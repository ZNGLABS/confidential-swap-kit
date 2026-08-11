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

/// Profondeur de l'arbre des engagements. 20 → 1 048 576 notes.
/// ⚠️ Doit être IDENTIQUE à celle de `circuit/pour.circom`.
pub const DEPTH: usize = 20;
/// Profondeur de l'arbre des nullifieurs. 20 → 1 048 576 nullifieurs.
/// ⚠️ Doit rester ≤ DEPTH : `racine_nf_vide()` réutilise le tableau `zeros`.
pub const NFDEPTH: usize = 20;
/// Nombre de racines mémorisées. Sans cet historique, deux swaps simultanés
/// se cassent mutuellement : la racine bouge pendant qu'on prépare sa preuve.
pub const ROOT_HISTORY: usize = 32;

// Disposition du compte, en octets :
//   [0..32)    racine des engagements
//   [32..64)   racine des NULLIFIEURS — remplace à elle seule tous les
//              comptes de nullifieurs, et leur rent de 0,0018 SOL par swap
//   [64..68)   next_index (u32, little-endian)
//   [68..72)   root_index (u32, little-endian)
//   [72..72+32*DEPTH)                  filled_subtrees
//   [.. + 32*ROOT_HISTORY)             historique des racines
pub const OFF_ROOT: usize = 0;
pub const OFF_NFROOT: usize = 32;
pub const OFF_NEXT: usize = 64;
pub const OFF_RIDX: usize = 68;
pub const OFF_FILLED: usize = 72;
pub const OFF_ROOTS: usize = OFF_FILLED + 32 * DEPTH;
pub const POOL_LEN: usize = OFF_ROOTS + 32 * ROOT_HISTORY;

/// Poseidon sur trois entrées — le hachage d'une feuille de l'arbre indexé.
pub fn h3(a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) -> Result<[u8; 32], ProgramError> {
    hashv(Parameters::Bn254X5, Endianness::BigEndian, &[a, b, c])
        .map(|r| r.to_bytes())
        .map_err(|_| ProgramError::InvalidArgument)
}

/// Racine de l'arbre des nullifieurs vide : une sentinelle `(0, 0, 0)` à
/// l'index 0, tous les autres emplacements à zéro. Tout nullifieur non nul
/// est supérieur à la sentinelle, donc l'arbre vide sait déjà répondre
/// « absent » — c'est ce qui amorce la chaîne.
/// ⚠️ Calculé en O(profondeur), pas en O(2^profondeur).
///
/// La première version parcourait les 2^NFDEPTH feuilles. À NFDEPTH = 3 cela
/// faisait 8 hachages ; à NFDEPTH = 14 cela en ferait 16 383, et
/// `Initialize` dépasserait le plafond de calcul avant d'avoir commencé.
/// Or l'arbre vide n'a qu'une feuille non nulle — la sentinelle, tout à
/// gauche : il suffit de remonter son chemin en pairant à chaque niveau avec
/// un sous-arbre vide, dont la valeur est précisément `zeros[i]`.
pub fn racine_nf_vide() -> Result<[u8; 32], ProgramError> {
    let zero = [0u8; 32];
    let z = zeros()?;
    let mut cur = h3(&zero, &zero, &zero)?;   // la sentinelle (0, 0, 0)
    for i in 0..NFDEPTH {
        cur = h2(&cur, &z[i])?;               // frère de droite : sous-arbre vide
    }
    Ok(cur)
}

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
    pub fn nf_root(&self) -> [u8; 32] { self.get32(OFF_NFROOT) }
    pub fn set_nf_root(&mut self, r: &[u8; 32]) { self.put32(OFF_NFROOT, r); }
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
        self.put_u32(OFF_RIDX, self.root_index().wrapping_add(1));
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
        // l'arbre des nullifieurs démarre avec sa seule sentinelle
        let nf = racine_nf_vide()?;
        self.set_nf_root(&nf);
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
            return Err(ProgramError::InvalidAccountData); // arbre plein
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
        self.put_u32(OFF_NEXT, self.next_index() + 1);
        self.pousser_racine(&cur);
        Ok(cur)
    }
}
