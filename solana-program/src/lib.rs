//! nexa-shielded-pool — vérification de preuve + registre confidentiel.
//!
//! Quatre instructions, distinguées par le premier octet :
//!   0 = VerifyOnly   vérifie une preuve et rien d'autre (108 304 CU mesurés)
//!   1 = Initialize   crée l'arbre vide
//!   2 = Pour         preuve + racine connue + nullifieurs neufs + insertion
//!   3 = Shield       dépose une note (insère un engagement, sans preuve)
//!
//! ⚠️ La clé de vérification embarquée vient d'une cérémonie de TEST.

use groth16_solana::groth16::Groth16Verifier;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

pub mod jetons;
pub mod pool;
pub mod verifying_key;

use jetons::{denomination_valide, ix_transfert, TOKEN_PROGRAM_ID};
use pool::{h2, zeros, Pool, POOL_LEN};

/// Convertit un element de corps 32 octets big-endian en u64.
/// Refuse ce qui depasse : un montant de 2^200 n'a pas de sens ici.
fn champ_vers_u64(f: &[u8; 32]) -> Result<u64, ProgramError> {
    if f[..24].iter().any(|b| *b != 0) {
        msg!("champ trop grand pour un montant");
        return Err(ProgramError::InvalidArgument);
    }
    Ok(u64::from_be_bytes(f[24..32].try_into().unwrap()))
}

/// Le meme entier vu comme element de corps sur 32 octets big-endian,
/// exactement ce que le circuit met dans Poseidon.
fn u64_vers_champ(v: u64) -> [u8; 32] {
    let mut f = [0u8; 32];
    f[24..32].copy_from_slice(&v.to_be_bytes());
    f
}
use verifying_key::{NR_PUBLIC_INPUTS, VERIFYINGKEY};

pub const PROOF_LEN: usize = 64 + 128 + 64;
pub const PAYLOAD_LEN: usize = PROOF_LEN + 32 * NR_PUBLIC_INPUTS;

pub const TAG_VERIFY: u8 = 0;
pub const TAG_INIT: u8 = 1;
pub const TAG_POUR: u8 = 2;
pub const TAG_SHIELD: u8 = 3;

pub const SEED_POOL: &[u8] = b"pool";
pub const SEED_NF: &[u8] = b"nf";

/// L'adresse du System Program est 11111111111111111111111111111111,
/// c'est-à-dire 32 octets nuls.
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([0u8; 32]);

/// Construit à la main l'instruction CreateAccount du System Program.
/// solana_program::system_instruction a disparu en 4.x, et les constructeurs
/// de solana-system-interface sont derrière une dépendance optionnelle. Le
/// format binaire, lui, est figé :
///   [0..4) u32=0 | [4..12) u64 lamports | [12..20) u64 espace | [20..52) owner
fn ix_creer_compte(
    depuis: &Pubkey,
    vers: &Pubkey,
    lamports: u64,
    espace: u64,
    proprietaire: &Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(52);
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&espace.to_le_bytes());
    data.extend_from_slice(proprietaire.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*depuis, true),
            AccountMeta::new(*vers, true),
        ],
        data,
    }
}

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
        TAG_SHIELD => shield(program_id, accounts, corps),
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
        &ix_creer_compte(payeur.key, compte_pool.key, rent, POOL_LEN as u64, program_id),
        &[payeur.clone(), compte_pool.clone(), systeme.clone()],
        &[&[SEED_POOL, &[bump]]],
    )?;

    let mut data = compte_pool.try_borrow_mut_data()?;
    Pool::new(&mut data)?.initialiser()?;
    msg!("arbre initialise, profondeur {}", pool::DEPTH);
    Ok(())
}

/// Dépose une note dans l'arbre : insère son engagement, sans preuve.
///
/// Dans un système complet, cette instruction serait couplée à un transfert de
/// tokens vers le pool — c'est l'entrée du circuit blindé. Ici elle se contente
/// d'insérer, ce qui suffit à faire exister des notes dépensables et donc à
/// démontrer le cycle complet dépôt → dépense sur devnet.
///
/// ⚠️ En l'état, n'importe qui peut créer des notes sans rien déposer.
///    Acceptable pour une preuve de concept, pas pour de l'argent réel.
fn shield(program_id: &Pubkey, accounts: &[AccountInfo], corps: &[u8]) -> ProgramResult {
    // Donnees : u64 montant (little-endian) + [u8; 32] k
    //
    // Le deposant ne fournit PAS l'engagement : le programme le recalcule,
    // cm = Poseidon(montant, k). C'est ce qui empeche de deposer 1 NX en
    // declarant une note de 1000. k = Poseidon(token, apk, rho, r) ne revele
    // ni le proprietaire ni l'alea.
    if corps.len() != 40 {
        msg!("attendu 8 octets de montant + 32 de k");
        return Err(ProgramError::InvalidInstructionData);
    }
    let montant = u64::from_le_bytes(corps[0..8].try_into().unwrap());
    let mut k = [0u8; 32];
    k.copy_from_slice(&corps[8..40]);

    // Un depot est PUBLIC. A montants libres il devient une empreinte :
    // voir swap-design/, 98 % de retraits relies par simple egalite.
    if !denomination_valide(montant) {
        msg!("montant {} hors denominations autorisees", montant);
        return Err(ProgramError::InvalidArgument);
    }

    let it = &mut accounts.iter();
    let deposant = next_account_info(it)?;
    let compte_pool = next_account_info(it)?;
    let coffre = next_account_info(it)?;
    let jetons_deposant = next_account_info(it)?;
    let prog_jetons = next_account_info(it)?;

    if !deposant.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if compte_pool.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    if *prog_jetons.key != TOKEN_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    // 1. les jetons entrent reellement dans le coffre
    invoke(
        &ix_transfert(jetons_deposant.key, coffre.key, deposant.key, montant),
        &[jetons_deposant.clone(), coffre.clone(), deposant.clone(), prog_jetons.clone()],
    )?;

    // 2. l'engagement est recalcule ici, pas accepte sur parole
    let cm = h2(&u64_vers_champ(montant), &k)?;
    let z = zeros()?;
    let mut data = compte_pool.try_borrow_mut_data()?;
    let mut p = Pool::new(&mut data)?;
    p.inserer(&cm, &z)?;
    msg!("depot de {} unites, index {}", montant, p.next_index());
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
    let coffre = next_account_info(it)?;
    let jetons_relayeur = next_account_info(it)?;
    let prog_jetons = next_account_info(it)?;

    if !payeur.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if compte_pool.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    if *prog_jetons.key != TOKEN_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
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
    msg!("pour accepte, index {}", p.next_index());
    drop(data);

    // 5. LES FRAIS EN $NX : c'est ici que le token gagne son travail.
    // Le circuit a deja impose somme(entrees) = somme(sorties) + frais, donc
    // la valeur versee au relayeur a ete retiree du total blinde. Le coffre et
    // la comptabilite cachee restent alignes, sans que personne n'apprenne qui
    // a paye. Le relayeur a avance le SOL, il se rembourse en NX.
    let frais = champ_vers_u64(&pubs[5])?;
    if frais > 0 {
        let (_, bump) = Pubkey::find_program_address(&[SEED_POOL], program_id);
        invoke_signed(
            &ix_transfert(coffre.key, jetons_relayeur.key, compte_pool.key, frais),
            &[coffre.clone(), jetons_relayeur.clone(), compte_pool.clone(), prog_jetons.clone()],
            &[&[SEED_POOL, &[bump]]],
        )?;
        msg!("frais payes au relayeur : {} unites", frais);
    }
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
    // 1 octet : seule l'EXISTENCE du compte porte l'information.
    // Coût permanent ~0,0009 SOL (voir registry/README).
    let rent = Rent::get()?.minimum_balance(1);
    invoke_signed(
        &ix_creer_compte(payeur.key, compte.key, rent, 1, program_id),
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
