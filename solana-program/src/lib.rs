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

use jetons::{
    coffre_attendu, denomination_valide, ix_transfert, mint_compte_jetons, cle_vers_champ,
    TOKEN_PROGRAM_ID,
};
use pool::{h3, zeros, Pool, POOL_LEN};
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

/// v8 — LA MONNAIE NX, EN DUR.
///
/// Le circuit garantit la cohérence : les frais sortent bien de l'emplacement 1.
/// Il n'a AUCUN moyen de savoir quel jeton occupe cet emplacement. Sans la
/// vérification ci-dessous, n'importe qui déclarerait sa propre monnaie sans
/// valeur comme jeton de frais et paierait le relayeur en monnaie de singe.
///
/// Pourquoi une constante et non un champ modifiable du pool : un jeton de frais
/// qu'une autorité peut changer est un levier, et quelqu'un finira par l'actionner.
/// Le protocole annonce « aucun admin » — autant que ce soit vrai.
///
/// ⚠️ Mint de TEST, dérivé de la graine « nexa-NX-test-mint-v8 »
/// (xZ6gLRio6BLoehB73TRTGiAPWhV9X1vZ9dzAawKgpMU). À remplacer par le vrai
/// mint NX avant tout déploiement en production.
pub const MINT_NX: Pubkey = Pubkey::new_from_array([
    14, 59, 60, 218, 146, 85, 248, 255, 93, 54, 202, 8, 71, 149, 230, 95, 167, 67, 81, 225, 188, 70, 190, 99, 159, 244, 124, 146, 195, 211, 226, 247,
]);

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

    // 1. LE COFFRE N'EST PAS CELUI QU'ON NOUS DÉSIGNE, C'EST CELUI QU'ON
    //    RECALCULE.
    //
    // Faille corrigée le 10 août 2026 : `coffre` arrivait de l'appelant sans
    // aucun contrôle. On pouvait passer son propre compte, s'y virer 100
    // jetons, et repartir avec un engagement valide dans l'arbre — puis
    // retirer depuis le vrai coffre. Ici on lit le mint sur le compte, on
    // dérive l'adresse associée du PDA du pool, et on refuse tout le reste.
    if coffre.owner != &TOKEN_PROGRAM_ID {
        msg!("le coffre n est pas un compte de jetons");
        return Err(ProgramError::IllegalOwner);
    }
    let mint = mint_compte_jetons(&coffre.try_borrow_data()?)?;
    if *coffre.key != coffre_attendu(compte_pool.key, &mint) {
        msg!("ce compte n est pas le coffre du pool pour ce jeton");
        return Err(ProgramError::InvalidSeeds);
    }

    // 2. les jetons entrent réellement dans le coffre
    invoke(
        &ix_transfert(jetons_deposant.key, coffre.key, deposant.key, montant),
        &[jetons_deposant.clone(), coffre.clone(), deposant.clone(), prog_jetons.clone()],
    )?;

    // 3. l'engagement est recalculé ici, pas accepté sur parole — et il lie
    //    désormais LE JETON, pas seulement le montant. Sans ce troisième
    //    terme, déposer un jeton sans valeur en déclarant une note en NX
    //    resterait possible.
    let cm = h3(&u64_vers_champ(montant), &cle_vers_champ(&mint), &k)?;
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
    //   6 le JETON concerné
    //
    // Les nullifieurs eux-mêmes ne sont plus publics : ils vivent dans le
    // circuit, qui prouve qu'ils étaient absents et les insère. Plus aucun
    // compte à créer, donc plus de rent — 0,0018 SOL économisés par swap.
    let pubs = verifier(corps)?;
    let (racine, cm0, cm1) = (pubs[0], pubs[1], pubs[2]);
    let (nf_avant, nf_apres) = (pubs[4], pubs[5]);
    let jeton = pubs[6];
    // v8 — LA 12e ENTRÉE PUBLIQUE s'insère ICI, entre le jeton de l'actif et le
    // montant retiré. Tous les index suivants se décalent donc d'un cran, et une
    // erreur là-dessus ne lèverait aucune alerte : le programme lirait simplement
    // la mauvaise valeur, et la preuve resterait valide.
    let jeton_frais = pubs[7];
    let destinataire = pubs[9];
    // pubs[3] = les frais, réglés au relayeur à l'étape 5
    // pubs[8] = le montant retiré, envoyé au bénéficiaire à l'étape 6

    let it = &mut accounts.iter();
    let payeur = next_account_info(it)?;          // le RELAYEUR : il avance le SOL
    let compte_pool = next_account_info(it)?;
    let coffre = next_account_info(it)?;          // coffre de l'ACTIF transféré
    let coffre_frais = next_account_info(it)?;    // v8 — coffre NX : les frais sortent d'ICI
    let jetons_relayeur = next_account_info(it)?; // et se rembourse ici, en NX
    let beneficiaire = next_account_info(it)?;    // là où sortent les jetons retirés
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

    // 1bis. LE COFFRE, ENCORE — et cette fois il doit correspondre au jeton
    //       que la preuve annonce.
    //
    // Les frais sortent de ce coffre. Sans ce contrôle, un appelant pourrait
    // présenter une preuve portant sur le jeton A et se faire payer depuis le
    // coffre du jeton B : la comptabilité cachée de B ne correspondrait plus
    // à ce que le coffre de B détient. Deux vérifications, indissociables :
    // le coffre est bien le nôtre pour ce mint, et ce mint est bien celui de
    // la preuve.
    if coffre.owner != &TOKEN_PROGRAM_ID {
        return Err(ProgramError::IllegalOwner);
    }
    let mint = mint_compte_jetons(&coffre.try_borrow_data()?)?;
    if *coffre.key != coffre_attendu(compte_pool.key, &mint) {
        msg!("ce compte n est pas le coffre du pool pour ce jeton");
        return Err(ProgramError::InvalidSeeds);
    }
    if cle_vers_champ(&mint) != jeton {
        msg!("le coffre ne correspond pas au jeton prouve");
        return Err(ProgramError::InvalidArgument);
    }

    // 1ter. LE COFFRE DES FRAIS — c'est ici que NX devient le jeton de frais.
    //
    // Trois contrôles, et aucun n'est superflu :
    //   a) le jeton de frais annoncé par la preuve EST la monnaie NX. Le circuit
    //      ne peut pas l'imposer : il ignore quel jeton occupe l'emplacement 1.
    //      Sans (a), le relayeur se fait payer en monnaie sans valeur.
    //   b) ce compte est bien NOTRE coffre pour ce mint,
    //   c) et son mint est bien celui que la preuve désigne — sinon un appelant
    //      annoncerait NX et se ferait payer depuis le coffre d'un autre actif.
    if jeton_frais != cle_vers_champ(&MINT_NX) {
        msg!("le jeton de frais n est pas NX");
        return Err(ProgramError::InvalidArgument);
    }
    if coffre_frais.owner != &TOKEN_PROGRAM_ID {
        msg!("le coffre des frais n est pas un compte de jetons");
        return Err(ProgramError::IllegalOwner);
    }
    let mint_frais = mint_compte_jetons(&coffre_frais.try_borrow_data()?)?;
    if *coffre_frais.key != coffre_attendu(compte_pool.key, &mint_frais) {
        msg!("ce compte n est pas le coffre du pool pour le jeton de frais");
        return Err(ProgramError::InvalidArgument);
    }
    if cle_vers_champ(&mint_frais) != jeton_frais {
        msg!("le coffre des frais ne correspond pas au jeton de frais prouve");
        return Err(ProgramError::InvalidArgument);
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
    // v8 : le circuit impose désormais DEUX conservations séparées,
    //        actif : entrée = sortie + retrait      (jamais de frais)
    //        NX    : entrée = sortie + frais        (jamais de retrait)
    // La valeur versée au relayeur a donc été retirée du total NX blindé, et
    // l'actif transféré n'a pas été entamé d'un centime. Chaque coffre reste
    // aligné sur SA comptabilité cachée, sans que personne n'apprenne qui a payé.
    // Le relayeur a avancé le SOL ; il se rembourse en NX, quel que soit l'actif
    // que l'utilisateur faisait circuler. C'est tout l'objet de cette version.
    let frais = champ_vers_u64(&pubs[3])?;
    let (_, bump) = Pubkey::find_program_address(&[SEED_POOL], program_id);
    if frais > 0 {
        invoke_signed(
            &ix_transfert(coffre_frais.key, jetons_relayeur.key, compte_pool.key, frais),
            &[coffre_frais.clone(), jetons_relayeur.clone(), compte_pool.clone(), prog_jetons.clone()],
            &[&[SEED_POOL, &[bump]]],
        )?;
        msg!("frais payes au relayeur : {} unites", frais);
    }

    // 6. LE RETRAIT — la valeur quitte le pool et redevient visible.
    //
    // C'est le chemin de sortie qui manquait : jusqu'ici on pouvait entrer et
    // circuler à l'intérieur, pas sortir. Le circuit a intégré `montantRetrait`
    // dans sa conservation, donc ce qui part d'ici a bien été retiré du total
    // blindé — le coffre et la comptabilité cachée restent alignés.
    //
    // ⚠️ Le destinataire est SCELLÉ dans la preuve. Sans cette vérification, le
    // relayeur — qui voit la preuve avant tout le monde — n'aurait qu'à changer
    // le compte d'arrivée pour encaisser le retrait à la place du bénéficiaire.
    // La preuve dit où va l'argent ; le programme ne fait qu'obéir.
    let retrait = champ_vers_u64(&pubs[8])?;
    if retrait > 0 {
        if beneficiaire.owner != &TOKEN_PROGRAM_ID {
            msg!("le beneficiaire n est pas un compte de jetons");
            return Err(ProgramError::IllegalOwner);
        }
        if cle_vers_champ(beneficiaire.key) != destinataire {
            msg!("ce beneficiaire n est pas celui que la preuve designe");
            return Err(ProgramError::InvalidArgument);
        }
        // Le programme de jetons refusera lui-même un compte d'un autre mint :
        // c'est sa vérification, elle est plus sûre que la nôtre.
        invoke_signed(
            &ix_transfert(coffre.key, beneficiaire.key, compte_pool.key, retrait),
            &[coffre.clone(), beneficiaire.clone(), compte_pool.clone(), prog_jetons.clone()],
            &[&[SEED_POOL, &[bump]]],
        )?;
        msg!("retrait de {} unites vers le beneficiaire", retrait);
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
