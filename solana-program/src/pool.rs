//! Le registre : arbre de Merkle incrémental + historique de racines.
//!
//! Transcription en Rust de la logique validée dans `registry/registry.js`
//! (10 tests, dont le seul qui compte : la racine calculée ici doit être
//! IDENTIQUE à celle que le client recalcule sur l'arbre complet, sinon
//! aucune preuve ne vérifiera jamais).
//!
//! ⚠️ `DEPTH` doit être IDENTIQUE à celui de `circuit/pour.circom`.
//!    Changer l'un sans l'autre casse silencieusement toutes les preuves.

use solana_poseidon::{hashv, Endianness, Parameters};
use solana_program::program_error::ProgramError;

/// Profondeur de l'arbre. 8 → 256 notes (taille de la preuve de concept).
pub const DEPTH: usize = 8;
/// Nombre de racines mémorisées. Sans cet historique, deux swaps simultanés
/// se cassent mutuellement : la racine bouge pendant qu'on prépare sa preuve.
pub const ROOT_HISTORY: usize = 32;

pub const OFF_ROOT: usize = 0;
pub const OFF_NEXT: usize = 32;
pub const OFF_RIDX: usize = 36;
pub const OFF_FILLED: usize = 40;
pub const OFF_ROOTS: usize = OFF_FILLED + 32 * DEPTH;
pub const POOL_LEN: usize = OFF_ROOTS + 32 * ROOT_HISTORY;

/// Poseidon sur deux éléments de corps, en big-endian — exactement la
/// convention de circomlib utilisée par le circuit.
pub fn h2(a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32], ProgramError> {
    hashv(Parameters::Bn254X5, Endianness::BigEndian, &[a, b])
        .map(|r| r.to_bytes())
        .map_err(|_| ProgramError::InvalidArgument)
}

/// zeros[i] = valeur d'un sous-arbre vide au niveau i.
pub fn zeros() -> Result<[[u8; 32]; DEPTH + 1], ProgramError> {
    let mut z = [[0u8; 32]; DEPTH + 1];
    for i in 1..=DEPTH {
        z[i] = h2(&z[i - 1], &z[i - 1])?;
    }
    Ok(z)
}

/// Vue en lecture/écriture sur les octets du compte.
pub struct Pool<'a> {
    pub data: &'a mut [u8],
}

impl<'a> Pool<'a> {
    pub fn new(data: &'a mut [u8]) -> Result<Self, ProgramError> {
        if data.len() < POOL_LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        Ok(Self { data })
    }

    fn get32(&self, off: usize) -> [u8; 32] {
        let mut o = [0u8; 32];
        o.copy_from_slice(&self.data[off..off + 32]);
        o
    }
    fn put32(&mut self, off: usize, v: &[u8; 32]) {
        self.data[off..off + 32].copy_from_slice(v);
    }
    fn get_u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.data[off..off + 4].try_into().unwrap())
    }
    fn put_u32(&mut self, off: usize, v: u32) {
        self.data[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn root(&self) -> [u8; 32] { self.get32(OFF_ROOT) }
    pub fn next_index(&self) -> u32 { self.get_u32(OFF_NEXT) }
    pub fn root_index(&self) -> u32 { self.get_u32(OFF_RIDX) }
    pub fn filled(&self, i: usize) -> [u8; 32] { self.get32(OFF_FILLED + 32 * i) }

    /// La racine fournie fait-elle partie de l'historique récent ?
    pub fn root_connue(&self, root: &[u8; 32]) -> bool {
        (0..ROOT_HISTORY).any(|i| &self.get32(OFF_ROOTS + 32 * i) == root)
    }

    fn pousser_racine(&mut self, root: &[u8; 32]) {
        let i = (self.root_index() as usize) % ROOT_HISTORY;
        self.put32(OFF_ROOTS + 32 * i, root);
        let n = self.root_index().wrapping_add(1);
        self.put_u32(OFF_RIDX, n);
        self.put32(OFF_ROOT, root);
    }

    /// Initialise un arbre vide. La racine de départ est zeros[DEPTH].
    pub fn initialiser(&mut self) -> Result<(), ProgramError> {
        let z = zeros()?;
        for i in 0..DEPTH {
            self.put32(OFF_FILLED + 32 * i, &[0u8; 32]);
        }
        self.put_u32(OFF_NEXT, 0);
        self.put_u32(OFF_RIDX, 0);
        for i in 0..ROOT_HISTORY {
            self.put32(OFF_ROOTS + 32 * i, &[0u8; 32]);
        }
        self.pousser_racine(&z[DEPTH]);
        Ok(())
    }

    /// Insère une feuille en O(DEPTH). C'est tout ce que la chaîne peut faire :
    /// recalculer 2^DEPTH feuilles à chaque insertion est hors de question.
    /// Le client, lui, reconstruit l'arbre complet depuis les engagements
    /// publiés — et doit retomber sur exactement la même racine.
    pub fn inserer(
        &mut self,
        feuille: &[u8; 32],
        z: &[[u8; 32]; DEPTH + 1],
    ) -> Result<[u8; 32], ProgramError> {
        let mut idx = self.next_index();
        if (idx as usize) >= (1usize << DEPTH) {
            return Err(ProgramError::InvalidAccountData);
        }
        let mut cur = *feuille;
        for i in 0..DEPTH {
            let (g, d) = if idx % 2 == 0 {
                self.put32(OFF_FILLED + 32 * i, &cur);
                (cur, z[i])
            } else {
                (self.filled(i), cur)
            };
            cur = h2(&g, &d)?;
            idx /= 2;
        }
        let n = self.next_index() + 1;
        self.put_u32(OFF_NEXT, n);
        self.pousser_racine(&cur);
        Ok(cur)
    }
}
