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
//!    Aucun argent réel ne doit dépendre de ce programme.

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

/// L'adresse du System Program est `11111111111111111111111111111111`,
/// c'est-à-dire 32 octets nuls.
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([0u8; 32]);

/// Construit à la main l'instruction `CreateAccount` du System Program.
///
/// Pourquoi à la main : `solana_program::system_instruction` a disparu de
/// solana-program 4.x, et dans `solana-system-interface` les constructeurs
/// sont derrière une dépendance optionnelle dont le nom de feature varie.
/// Le format binaire, lui, est figé depuis toujours :
///   [0..4)   u32 = 0  (variante CreateAccount)
///   [4..12)  u64 lamports
///   [12..20) u64 espace
///   [20..52) propriétaire
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

pub mod jetons;
pub mod pool;
pub mod verifying_key;

use jetons::{denomination_valide, ix_transfert, TOKEN_PROGRAM_ID};
use pool::{h2, zeros, Pool, POOL_LEN};
use verifying_key::{NR_PUBLIC_INPUTS, VERIFYINGKEY};

/// Convertit un élément de corps 32 octets big-endian en u64.
/// Refuse tout ce qui dépasse — un « montant » de 2^200 n'a pas de sens ici.
fn champ_vers_u64(f: &[u8; 32]) -> Result<u64, ProgramError> {
    if f[..24].iter().any(|b| *b != 0) {
        msg!("champ trop grand pour un montant");
        return Err(ProgramError::InvalidArgument);
    }
    Ok(u64::from_be_bytes(f[24..32].try_into().unwrap()))
}

/// Le même entier, vu comme élément de corps sur 32 octets big-endian —
/// exactement ce que le circuit met dans Poseidon.
fn u64_vers_champ(v: u64) -> [u8; 32] {
    let mut f = [0u8; 32];
    f[24..32].copy_from_slice(&v.to_be_bytes());
    f
}

/// proof_a (64) ‖ proof_b (128) ‖ proof_c (64)
pub const PROOF_LEN: usize = 64 + 128 + 64;
/// … ‖ entrées publiques (32 × N)
pub const PAYLOAD_LEN: usize = PROOF_LEN + 32 * NR_PUBLIC_INPUTS;

pub const TAG_VERIFY: u8 = 0;
pub const TAG_INIT: u8 = 1;
pub const TAG_POUR: u8 = 2;
pub const TAG_SHIELD: u8 = 3;

pub const SEED_POOL: &[u8] = b"pool";
// SEED_NF n'existe plus : il n'y a plus un seul compte de nullifieur.

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

// ───────────────────────────────────────────────── vérification de la preuve

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

    // Format produit et démontré par `circuit/to_solana.js` :
    //   · 32 octets BIG-ENDIAN partout ;
    //   · proof_a déjà NÉGATIVÉ (l'équation utilise −A) ;
    //   · G2 en (c1, c0), convention Ethereum/alt_bn128.
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

// ───────────────────────────────────────────────── initialisation

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

// ───────────────────────────────────────────────── le vrai chemin

/// Dépose une note dans l'arbre : insère son engagement, sans preuve.
///
/// Dans un système complet, cette instruction serait couplée à un transfert de
/// tokens vers le pool — c'est l'entrée du circuit blindé. Ici elle se contente
/// d'insérer, ce qui suffit à faire exister des notes dépensables et donc à
/// démontrer le cycle complet dépôt → dépense sur devnet.
///
/// ⚠️ En l'état, n'importe qui peut créer des notes sans rien déposer.
///    C'est acceptable pour une preuve de concept, pas pour de l'argent réel.
fn shield(program_id: &Pubkey, accounts: &[AccountInfo], corps: &[u8]) -> ProgramResult {
    // Données : u64 montant (little-endian) ‖ [u8; 32] k
    //
    // Le déposant ne fournit PAS l'engagement : le programme le recalcule,
    // cm = Poseidon(montant, k). C'est ce qui empêche de déposer 1 NX en
    // déclarant une note de 1 000. `k = Poseidon(token, apk, rho, r)` ne
    // révèle ni le propriétaire ni l'aléa.
    if corps.len() != 40 {
        msg!("attendu 8 octets de montant + 32 de k");
        return Err(ProgramError::InvalidInstructionData);
    }
    let montant = u64::from_le_bytes(corps[0..8].try_into().unwrap());
    let mut k = [0u8; 32];
    k.copy_from_slice(&corps[8..40]);

    // Un dépôt est PUBLIC. À montants libres il devient une empreinte :
    // voir swap-design/, 98 % de retraits reliés par simple égalité.
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

    // 1. les jetons entrent réellement dans le coffre
    invoke(
        &ix_transfert(jetons_deposant.key, coffre.key, deposant.key, montant),
        &[jetons_deposant.clone(), coffre.clone(), deposant.clone(), prog_jetons.clone()],
    )?;

    // 2. l'engagement est recalculé ici, pas accepté sur parole
    let cm = h2(&u64_vers_champ(montant), &k)?;
    let z = zeros()?;
    let mut data = compte_pool.try_borrow_mut_data()?;
    let mut p = Pool::new(&mut data)?;
    p.inserer(&cm, &z)?;
    msg!("depot de {} unites — index {}", montant, p.next_index());
    Ok(())
}

fn pour(program_id: &Pubkey, accounts: &[AccountInfo], corps: &[u8]) -> ProgramResult {
    // 1. la preuve d'abord : inutile de toucher au registre si elle est fausse
    //
    // Entrées publiques, dans l'ordre du circuit :
    //   0 racine des engagements
    //   1,2 nouveaux engagements
    //   3 frais
    //   4,5 racine des nullifieurs AVANT et APRÈS
    //
    // Les nullifieurs eux-mêmes ne sont plus publics : ils vivent dans le
    // circuit, qui prouve qu'ils étaient absents et les insère. Plus aucun
    // compte à créer, donc plus de rent — 0,0018 SOL économisés par swap.
    let pubs = verifier(corps)?;
    let (racine, cm0, cm1) = (pubs[0], pubs[1], pubs[2]);
    let (nf_avant, nf_apres) = (pubs[4], pubs[5]);
    // pubs[3] = les frais, réglés au relayeur à l'étape 5

    let it = &mut accounts.iter();
    let payeur = next_account_info(it)?;          // le RELAYEUR : il avance le SOL
    let compte_pool = next_account_info(it)?;
    let coffre = next_account_info(it)?;
    let jetons_relayeur = next_account_info(it)?; // et se rembourse ici, en NX
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

    // 2 et 3. les deux racines, lues d'un seul coup.
    //
    // La racine des engagements doit être CONNUE, pas forcément courante :
    // sinon deux swaps concurrents se feraient échouer l'un l'autre.
    //
    // La racine des nullifieurs, elle, doit être EXACTEMENT la courante. Le
    // circuit a prouvé que les deux nullifieurs en étaient absents, puis les y
    // a insérés : il part de `nf_avant` et aboutit à `nf_apres`. Le programme
    // n'a qu'à vérifier le point de départ, et enregistrer l'arrivée.
    //
    // Ces trois lignes remplacent deux créations de compte à 0,0009 SOL pièce,
    // définitivement perdus. C'est ce qui rend un relayeur viable.
    {
        let data = compte_pool.try_borrow_data()?;
        let mut copie = data.to_vec();
        let p = Pool::new(&mut copie)?;
        if !p.root_connue(&racine) {
            msg!("racine inconnue ou trop ancienne");
            return Err(ProgramError::InvalidArgument);
        }
        if p.nf_root() != nf_avant {
            msg!("racine des nullifieurs perimee");
            return Err(ProgramError::InvalidArgument);
        }
    }

    // 4. insertion des nouveaux engagements
    {
        let z = zeros()?;
        let mut data = compte_pool.try_borrow_mut_data()?;
        let mut p = Pool::new(&mut data)?;
        p.inserer(&cm0, &z)?;
        p.inserer(&cm1, &z)?;
        // la nouvelle racine des nullifieurs, telle que le circuit l'a prouvée
        p.set_nf_root(&nf_apres);
        msg!("pour accepte — index {}", p.next_index());
    }

    // 5. LES FRAIS EN $NX — c'est ici que le token gagne son travail.
    //
    // Le circuit a déjà imposé Σ entrées = Σ sorties + frais : la valeur
    // versée au relayeur a donc été retirée du total blindé. Le coffre et la
    // comptabilité cachée restent alignés, sans que personne n'apprenne qui
    // a payé. Le relayeur a avancé le SOL, il se rembourse en NX.
    let frais = champ_vers_u64(&pubs[3])?;
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

// `creer_nullifieur` a été supprimée le 9 août 2026.
//
// Elle créait un compte par nullifieur : deux comptes par swap, 0,0018 SOL
// immobilisés à jamais, soit 0,36 $ que personne ne récupère. L'arbre indexé
// fait le même travail — refuser la double dépense — pour 32 octets réutilisés
// dans le compte du pool. Le circuit prouve l'absence puis l'insertion ; la
// chaîne se contente de comparer deux racines.

// ───────────────────────────────────────────────── aide au client

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
