//! nexa-shielded-pool — vérification de preuve + registre confidentiel.
//!
//! Trois instructions, distinguées par le premier octet :
//!   0 = VerifyOnly   vérifie une preuve et rien d'autre (sert à mesurer le
//!                    coût de la seule vérification : 108 580 CU mesurés)
//!   1 = Initialize   crée l'arbre vide
//!   2 = Pour         le vrai chemin : preuve + racine connue + nullifieurs
//!                    neufs + insertion des nouveaux engagements
//!
//! ⚠️ La clé de vérification embarquée vient d'une cérémonie de TEST.

use groth16_solana::groth16::Groth16Verifier;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    sysvar::Sysvar,
};

pub mod pool;
pub mod verifying_key;

use pool::{zeros, Pool, POOL_LEN};
use verifying_key::{NR_PUBLIC_INPUTS, VERIFYINGKEY};

pub const PROOF_LEN: usize = 64 + 128 + 64;
pub const PAYLOAD_LEN: usize = PROOF_LEN + 32 * NR_PUBLIC_INPUTS;

pub const TAG_VERIFY: u8 = 0;
pub const TAG_INIT: u8 = 1;
pub const TAG_POUR: u8 = 2;

pub const SEED_POOL: &[u8] = b"pool";
pub const SEED_NF: &[u8] = b"nf";

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (tag, corps) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        TAG_VERIFY => verifier_seulement(corps),
        TAG_INIT => initialiser(program_id, accounts),
        TAG_POUR => pour(program_id, accounts, corps),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

/// Découpe le payload et vérifie la preuve. Renvoie les 6 entrées publiques.
fn verifier(corps: &[u8]) -> Result<[[u8; 32]; NR_PUBLIC_INPUTS], ProgramError> {
    if corps.len() != PAYLOAD_LEN {
        msg!("taille attendue {} octets, recu {}", PAYLOAD_LEN, corps.len());
        return Err(ProgramError::InvalidInstructionData);
    }
    let proof_a: [u8; 64] = corps[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = corps[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = corps[192..256].try_into().unwrap();

    let mut pubs = [[0u8; 32]; NR_PUBLIC_INPUTS];
    for (i, slot) in pubs.iter_mut().enumerate() {
        let o = PROOF_LEN + 32 * i;
        slot.copy_from_slice(&corps[o..o + 32]);
    }

    // Format produit et démontré par circuit/to_solana.js :
    //   32 octets BIG-ENDIAN ; proof_a déjà NÉGATIVÉ ; G2 en (c1, c0).
    let mut v = Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &pubs, &VERIFYINGKEY)
        .map_err(|_| {
            msg!("preuve malformee");
            ProgramError::InvalidArgument
        })?;
    v.verify().map_err(|_| {
        msg!("PREUVE REFUSEE");
        ProgramError::InvalidArgument
    })?;
    Ok(pubs)
}

fn verifier_seulement(corps: &[u8]) -> ProgramResult {
    verifier(corps)?;
    msg!("preuve valide");
    Ok(())
}

fn initialiser(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let payeur = next_account_info(it)?;
    let compte_pool = next_account_info(it)?;
    let systeme = next_account_info(it)?;

    if !payeur.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (attendu, bump) = Pubkey::find_program_address(&[SEED_POOL], program_id);
    if attendu != *compte_pool.key {
        return Err(ProgramError::InvalidSeeds);
    }

    let rent = Rent::get()?.minimum_balance(POOL_LEN);
    invoke_signed(
        &system_instruction::create_account(
            payeur.key,
            compte_pool.key,
            rent,
            POOL_LEN as u64,
            program_id,
        ),
        &[payeur.clone(), compte_pool.clone(), systeme.clone()],
        &[&[SEED_POOL, &[bump]]],
    )?;

    let mut data = compte_pool.try_borrow_mut_data()?;
    Pool::new(&mut data)?.initialiser()?;
    msg!("arbre initialise, profondeur {}", pool::DEPTH);
    Ok(())
}

fn pour(program_id: &Pubkey, accounts: &[AccountInfo], corps: &[u8]) -> ProgramResult {
    // 1. la preuve d'abord : inutile de toucher au registre si elle est fausse
    let pubs = verifier(corps)?;
    let (racine, nf0, nf1, cm0, cm1) = (pubs[0], pubs[1], pubs[2], pubs[3], pubs[4]);

    let it = &mut accounts.iter();
    let payeur = next_account_info(it)?;
    let compte_pool = next_account_info(it)?;
    let compte_nf0 = next_account_info(it)?;
    let compte_nf1 = next_account_info(it)?;
    let systeme = next_account_info(it)?;

    if !payeur.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if compte_pool.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }

    // 2. la racine doit être CONNUE, pas seulement être la courante : sinon
    //    toute transaction concurrente ferait échouer les autres.
    {
        let data = compte_pool.try_borrow_data()?;
        let mut copie = data.to_vec();
        let p = Pool::new(&mut copie)?;
        if !p.root_connue(&racine) {
            msg!("racine inconnue ou trop ancienne");
            return Err(ProgramError::InvalidArgument);
        }
    }

    // 3. les nullifieurs : un compte par nullifieur, dont la création échoue
    //    s'il existe déjà. Le circuit prouve que le nullifieur est bien
    //    dérivé, PAS qu'il est neuf — c'est le registre qui le sait.
    creer_nullifieur(program_id, payeur, compte_nf0, systeme, &nf0)?;
    creer_nullifieur(program_id, payeur, compte_nf1, systeme, &nf1)?;

    // 4. insertion des nouveaux engagements
    let z = zeros()?;
    let mut data = compte_pool.try_borrow_mut_data()?;
    let mut p = Pool::new(&mut data)?;
    p.inserer(&cm0, &z)?;
    p.inserer(&cm1, &z)?;

    msg!("pour accepte — index {}", p.next_index());
    Ok(())
}

fn creer_nullifieur<'a>(
    program_id: &Pubkey,
    payeur: &AccountInfo<'a>,
    compte: &AccountInfo<'a>,
    systeme: &AccountInfo<'a>,
    nf: &[u8; 32],
) -> ProgramResult {
    let (attendu, bump) = Pubkey::find_program_address(&[SEED_NF, nf], program_id);
    if attendu != *compte.key {
        return Err(ProgramError::InvalidSeeds);
    }
    if compte.lamports() > 0 || !compte.data_is_empty() {
        msg!("DOUBLE DEPENSE : nullifieur deja vu");
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    // 1 octet : on ne stocke rien, seule l'EXISTENCE du compte porte
    // l'information. Coût permanent ~0,0009 SOL (voir registry/README).
    let rent = Rent::get()?.minimum_balance(1);
    invoke_signed(
        &system_instruction::create_account(payeur.key, compte.key, rent, 1, program_id),
        &[payeur.clone(), compte.clone(), systeme.clone()],
        &[&[SEED_NF, nf, &[bump]]],
    )
}

/// Assemble les données d'une instruction. Exposé pour que les tests et le
/// client utilisent exactement le même code que le programme.
pub fn build_instruction_data(
    tag: u8,
    proof_a: &[u8; 64],
    proof_b: &[u8; 128],
    proof_c: &[u8; 64],
    public_inputs: &[[u8; 32]; NR_PUBLIC_INPUTS],
) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + PAYLOAD_LEN);
    v.push(tag);
    v.extend_from_slice(proof_a);
    v.extend_from_slice(proof_b);
    v.extend_from_slice(proof_c);
    for pi in public_inputs {
        v.extend_from_slice(pi);
    }
    v
}
