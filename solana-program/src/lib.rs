//! nexa-verifier — vérifie on-chain une preuve du circuit `pour.circom`.
//!
//! Ce programme ne fait qu'UNE chose : recevoir une preuve Groth16 et l'énoncé
//! public, et refuser la transaction si la preuve est fausse. Le registre
//! (arbre de Merkle, ensemble des nullifieurs) viendra ensuite ; l'objectif ici
//! est d'obtenir la MESURE du coût en unités de calcul, puis un swap
//! confidentiel vérifiable sur devnet.
//!
//! ⚠️ La clé de vérification embarquée vient d'une cérémonie de TEST.
//!    Aucun argent réel ne doit dépendre de ce programme.

use groth16_solana::groth16::Groth16Verifier;
use solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    msg,
    program_error::ProgramError,
    pubkey::Pubkey,
};

pub mod verifying_key;
use verifying_key::{NR_PUBLIC_INPUTS, VERIFYINGKEY};

/// proof_a (64) ‖ proof_b (128) ‖ proof_c (64)
const PROOF_LEN: usize = 64 + 128 + 64;
/// … ‖ entrées publiques (32 × N)
const DATA_LEN: usize = PROOF_LEN + 32 * NR_PUBLIC_INPUTS;

entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    if data.len() != DATA_LEN {
        msg!("taille attendue {} octets, reçu {}", DATA_LEN, data.len());
        return Err(ProgramError::InvalidInstructionData);
    }

    // Découpage. Les `unwrap` sont sûrs : la longueur vient d'être vérifiée.
    let proof_a: [u8; 64] = data[0..64].try_into().unwrap();
    let proof_b: [u8; 128] = data[64..192].try_into().unwrap();
    let proof_c: [u8; 64] = data[192..256].try_into().unwrap();

    let mut public_inputs = [[0u8; 32]; NR_PUBLIC_INPUTS];
    for (i, slot) in public_inputs.iter_mut().enumerate() {
        let off = PROOF_LEN + 32 * i;
        slot.copy_from_slice(&data[off..off + 32]);
    }

    // Rappel du format, produit et vérifié par `circuit/to_solana.js` :
    //   · tout en 32 octets BIG-ENDIAN ;
    //   · proof_a est déjà NÉGATIVÉ (l'équation utilise −A) ;
    //   · pour G2, chaque coordonnée Fq2 est écrite (c1, c0), convention
    //     Ethereum/alt_bn128 — l'inverse de l'ordre du JSON snarkjs.
    let mut verifier =
        Groth16Verifier::new(&proof_a, &proof_b, &proof_c, &public_inputs, &VERIFYINGKEY)
            .map_err(|_| {
                msg!("preuve malformée");
                ProgramError::InvalidArgument
            })?;

    verifier.verify().map_err(|_| {
        msg!("PREUVE REFUSÉE");
        ProgramError::InvalidArgument
    })?;

    msg!("preuve valide — {} entrees publiques", NR_PUBLIC_INPUTS);
    Ok(())
}

/// Assemble les données d'instruction dans l'ordre attendu.
/// Exposé pour que les tests et le client utilisent exactement le même code.
pub fn build_instruction_data(
    proof_a: &[u8; 64],
    proof_b: &[u8; 128],
    proof_c: &[u8; 64],
    public_inputs: &[[u8; 32]; NR_PUBLIC_INPUTS],
) -> Vec<u8> {
    let mut v = Vec::with_capacity(DATA_LEN);
    v.extend_from_slice(proof_a);
    v.extend_from_slice(proof_b);
    v.extend_from_slice(proof_c);
    for pi in public_inputs {
        v.extend_from_slice(pi);
    }
    v
}
