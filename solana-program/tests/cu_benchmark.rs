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
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([7u8; 32]);

fn mollusk() -> Mollusk {
    Mollusk::new(&PROGRAM_ID, "nexa_verifier")
}

fn run(data: Vec<u8>) -> mollusk_svm::result::InstructionResult {
    let ix = Instruction::new_with_bytes(PROGRAM_ID, &data, vec![]);
    mollusk().process_instruction(&ix, &[])
}

#[test]
fn preuve_valide_acceptee_et_cout_mesure() {
    let data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    let res = run(data);

    println!("\n──────────────────────────────────────────────");
    println!("  COÛT MESURÉ : {} CU", res.compute_units_consumed);
    println!("  plafond par transaction : 1 400 000 CU");
    println!(
        "  part du plafond : {:.2} %",
        100.0 * res.compute_units_consumed as f64 / 1_400_000.0
    );
    println!("  entrées publiques : {}", NR_PUBLIC_INPUTS);
    println!("  (calcul théorique des opérations crypto : 97 771 CU)");
    println!("──────────────────────────────────────────────\n");

    assert!(
        res.program_result.is_err() == false,
        "la preuve honnête doit être acceptée : {:?}",
        res.program_result
    );
    assert!(
        res.compute_units_consumed < 1_400_000,
        "la vérification doit tenir dans une transaction"
    );
}

#[test]
fn entree_publique_modifiee_refusee() {
    let mut pubs = PUBLIC_INPUTS;
    pubs[0][31] ^= 1; // un seul bit
    let data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &pubs);
    let res = run(data);
    assert!(
        res.program_result.is_err(),
        "un énoncé public modifié doit être refusé"
    );
}

#[test]
fn preuve_modifiee_refusee() {
    let mut a = PROOF_A;
    a[0] ^= 1;
    let data = build_instruction_data(&a, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    let res = run(data);
    assert!(res.program_result.is_err(), "une preuve modifiée doit être refusée");
}

#[test]
fn taille_invalide_refusee() {
    let mut data = build_instruction_data(&PROOF_A, &PROOF_B, &PROOF_C, &PUBLIC_INPUTS);
    data.pop();
    let res = run(data);
    assert!(res.program_result.is_err(), "une longueur invalide doit être refusée");
}
