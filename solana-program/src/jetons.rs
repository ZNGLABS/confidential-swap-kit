//! La couche jetons : le pool détient réellement des tokens, et les frais
//! se paient en $NX.
//!
//! ## Pourquoi les frais sont PUBLICS alors que tout le reste est caché
//!
//! Un validateur doit pouvoir vérifier qu'il est payé. Le circuit expose donc
//! `fee` parmi ses six entrées publiques — dès la première version, cette
//! place était réservée. Le montant échangé reste secret ; le prix du service,
//! non. C'est le compromis de Zerocash, et il est nécessaire.
//!
//! ## Pourquoi un relayeur
//!
//! Si l'utilisateur payait les frais Solana depuis son propre compte, il se
//! désignerait lui-même — et toute la confidentialité tomberait. Un tiers
//! (le relayeur) soumet donc la transaction, avance le SOL, et se rembourse
//! en $NX pris **à l'intérieur du pool**. L'utilisateur n'apparaît nulle part.
//!
//! C'est exactement pour ça que le circuit impose
//!     Σ entrées = Σ sorties + frais
//! La valeur qui sort du pool vers le relayeur est retirée du total blindé.
//! Les deux comptabilités restent alignées sans jamais révéler qui paie.
//!
//! ## Pourquoi des dénominations imposées au dépôt
//!
//! Un dépôt est PUBLIC — c'est un transfert de tokens visible. La simulation
//! de `swap-design/` a montré qu'à montants libres, 98 % des retraits se
//! relient à leur dépôt par simple égalité. Des dénominations fixes
//! suppriment cette fuite, et surtout elles ne dépendent pas de la discipline
//! de l'utilisateur.

use solana_program::{
    instruction::{AccountMeta, Instruction},
    program_error::ProgramError,
    pubkey::Pubkey,
};

/// TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
pub const TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172,
    28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

/// Dénominations acceptées au dépôt, en unités de base du jeton.
/// Avec 6 décimales : 1, 10, 100 et 1 000 NX.
pub const DENOMINATIONS: [u64; 4] = [1_000_000, 10_000_000, 100_000_000, 1_000_000_000];

pub fn denomination_valide(montant: u64) -> bool {
    DENOMINATIONS.contains(&montant)
}

/// Instruction `Transfer` du programme SPL Token, encodée à la main.
/// Format figé : `[3u8]` suivi du montant sur 8 octets little-endian.
pub fn ix_transfert(
    source: &Pubkey,
    destination: &Pubkey,
    autorite: &Pubkey,
    montant: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&montant.to_le_bytes());
    Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*source, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*autorite, true),
        ],
        data,
    }
}

/// Lit le solde d'un compte de jetons SPL sans désérialiser toute la
/// structure : le champ `amount` occupe les octets 64 à 72.
pub fn solde_compte_jetons(donnees: &[u8]) -> Result<u64, ProgramError> {
    if donnees.len() < 72 {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(u64::from_le_bytes(donnees[64..72].try_into().unwrap()))
}

/// Le mint d'un compte de jetons SPL occupe les 32 premiers octets.
pub fn mint_compte_jetons(donnees: &[u8]) -> Result<Pubkey, ProgramError> {
    if donnees.len() < 32 {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(Pubkey::new_from_array(donnees[0..32].try_into().unwrap()))
}
