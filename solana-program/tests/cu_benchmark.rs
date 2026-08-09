//! Mesure du coût réel en unités de calcul, et contrôle que le programme
//! refuse bien une preuve fausse.
//!
//! C'est CE fichier qui doit remplacer le calcul de `circuit/cu_estimate.py`
//! par un chiffre mesuré. Le calcul annonce ~97 771 CU pour les opérations
//! cryptographiques seules ; ce test dira ce que coûte le programme complet.

use mollusk_svm::Mollusk;
use nexa_verifier::{
    build_instruction_data,
    verifying_key::{NR_PUBLIC_INPUTS, PROOF_A, PROOF_B, PROOF_C, PUBLIC_INPUTS},
};
use solana_account::Account;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([7u8; 32]);

/// Renvoie (le programme a-t-il refusé ?, unités de calcul consommées).
/// On ne nomme volontairement aucun type de Mollusk ici : leurs chemins
/// changent d'une version à l'autre, pas cette signature.
fn executer(data: Vec<u8>) -> (bool, u64) {
    let mollusk = Mollusk::new(&PROGRAM_ID, "nexa_verifier");
    let ix = Instruction::new_with_bytes(PROGRAM_ID, &data, vec![]);
    let comptes: Vec<(Pubkey, Account)> = vec![];
    let res = mollusk.process_instruction(&ix, &comptes);
    (res.program_result.is_err(), res.compute_units_consumed)
}

#[test]
fn preuve_valide_acceptee_et_cout_mesure() {
    let data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    let (refuse, cu) = executer(data);

    println!("\n──────────────────────────────────────────────");
    println!("  COÛT MESURÉ : {} CU", cu);
    println!("  plafond par transaction : 1 400 000 CU");
    println!("  part du plafond : {:.2} %", 100.0 * cu as f64 / 1_400_000.0);
    println!("  entrées publiques : {}", NR_PUBLIC_INPUTS);
    println!("  (calcul théorique des opérations crypto : 97 771 CU)");
    println!("  écart calcul → mesure : {} CU", cu as i64 - 97_771);
    println!("──────────────────────────────────────────────\n");

    assert!(!refuse, "la preuve honnête doit être acceptée");
    assert!(cu > 0, "le coût mesuré doit être non nul");
    assert!(cu < 1_400_000, "la vérification doit tenir dans une transaction");
}

#[test]
fn entree_publique_modifiee_refusee() {
    let mut pubs = PUBLIC_INPUTS;
    pubs[0][31] ^= 1; // un seul bit
    let data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &pubs);
    let (refuse, _) = executer(data);
    assert!(refuse, "un énoncé public modifié doit être refusé");
}

#[test]
fn preuve_modifiee_refusee() {
    let mut a = PROOF_A;
    a[0] ^= 1;
    let data = build_instruction_data(&a, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    let (refuse, _) = executer(data);
    assert!(refuse, "une preuve modifiée doit être refusée");
}

#[test]
fn taille_invalide_refusee() {
    let mut data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    data.pop();
    let (refuse, _) = executer(data);
    assert!(refuse, "une longueur invalide doit être refusée");
}
