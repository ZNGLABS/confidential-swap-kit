#!/usr/bin/env node
/*
 * to_solana.js — convertit une preuve snarkjs vers la disposition d'octets
 * attendue par `groth16-solana` (Light Protocol), et PROUVE que la conversion
 * est correcte.
 *
 * Pourquoi ce fichier existe : c'est ici que le portage casse, presque toujours.
 * Trois pièges, tous silencieux :
 *   1. snarkjs sort des entiers décimaux ; Solana veut 32 octets BIG-ENDIAN ;
 *   2. l'équation de vérification utilise −A, pas A → il faut NÉGATIVER pi_a ;
 *   3. pour G2, l'ordre des deux composantes de chaque coordonnée Fq2 n'est pas
 *      le même dans le JSON de snarkjs et dans la convention Ethereum/arkworks
 *      que suivent les syscalls alt_bn128 de Solana.
 *
 * Le test anti-circularité : je ne me contente pas de relire mes propres octets
 * avec ma propre fonction (ça validerait n'importe quelle convention pourvu
 * qu'elle soit cohérente). Je relis chaque tranche de 32 octets comme un simple
 * entier big-endian et je la compare aux valeurs décimales du JSON d'origine.
 * L'ordre réellement écrit est donc constaté, pas supposé.
 */

const fs = require("fs");
const path = require("path");
const { buildBn128, Scalar } = require("ffjavascript");

const DIR = process.argv[2] || "build";
const OUT = process.argv[3] || path.join(DIR, "solana");

const rd = (f) => JSON.parse(fs.readFileSync(path.join(DIR, f), "utf8"));
const beBytes = (x) => {                       // BigInt -> 32 octets big-endian
  const b = Buffer.alloc(32);
  let v = BigInt(x);
  for (let i = 31; i >= 0; i--) { b[i] = Number(v & 0xffn); v >>= 8n; }
  return b;
};
const beRead = (buf, off) =>                   // 32 octets big-endian -> BigInt
  BigInt("0x" + buf.subarray(off, off + 32).toString("hex"));

let pass = 0, fail = 0;
const ok = (m) => { console.log(`  ✅ ${m}`); pass++; };
const ko = (m) => { console.log(`  ❌ ${m}`); fail++; };

(async () => {
  const curve = await buildBn128(true);
  const G1 = curve.G1, G2 = curve.G2, Fr = curve.Fr;

  const vk = rd("vkey.json");
  const pr = rd("proof.json");
  const pub = rd("public.json");

  // ───────────────────────────── points de courbe depuis le JSON
  const g1 = (a) => G1.fromObject([BigInt(a[0]), BigInt(a[1]), BigInt(a[2] ?? 1)]);
  const g2 = (a) => G2.fromObject([
    [BigInt(a[0][0]), BigInt(a[0][1])],
    [BigInt(a[1][0]), BigInt(a[1][1])],
    [BigInt(a[2]?.[0] ?? 1), BigInt(a[2]?.[1] ?? 0)],
  ]);

  const A = g1(pr.pi_a), B = g2(pr.pi_b), C = g1(pr.pi_c);
  const negA = G1.neg(A);
  const alpha = g1(vk.vk_alpha_1), beta = g2(vk.vk_beta_2);
  const gamma = g2(vk.vk_gamma_2), delta = g2(vk.vk_delta_2);
  const IC = vk.IC.map(g1);

  console.log("=".repeat(62));
  console.log("  Conversion snarkjs → groth16-solana");
  console.log("=".repeat(62));
  console.log(`\n  entrées publiques : ${pub.length}   ·   IC : ${IC.length} points`);

  // ───────────────────────────── 1. l'équation de Groth16 tient-elle ?
  console.log("\n[1] L'équation de vérification, calculée ici");
  let vkx = IC[0];
  pub.forEach((p, i) => { vkx = G1.add(vkx, G1.timesScalar(IC[i + 1], Scalar.e(p))); });

  const good = await curve.pairingEq(negA, B, alpha, beta, vkx, gamma, C, delta);
  good ? ok("e(−A,B)·e(α,β)·e(vk_x,γ)·e(C,δ) = 1") : ko("l'équation est fausse");

  // contrôle négatif : sans la négation de A, ça doit échouer
  const withoutNeg = await curve.pairingEq(A, B, alpha, beta, vkx, gamma, C, delta);
  withoutNeg ? ko("l'équation passe SANS négation de A (impossible)")
             : ok("sans négation de A → échoue (la négation est bien nécessaire)");

  // contrôle négatif : une entrée publique modifiée
  let vkxBad = IC[0];
  pub.forEach((p, i) => {
    const v = i === 0 ? Scalar.e(BigInt(p) + 1n) : Scalar.e(p);
    vkxBad = G1.add(vkxBad, G1.timesScalar(IC[i + 1], v));
  });
  const bad = await curve.pairingEq(negA, B, alpha, beta, vkxBad, gamma, C, delta);
  bad ? ko("une entrée publique modifiée est acceptée") : ok("entrée publique modifiée → rejetée");

  // ───────────────────────────── 2. sérialisation, et constat de l'ordre écrit
  console.log("\n[2] Disposition des octets, CONSTATÉE (relecture en big-endian brut)");

  const serG1 = (P) => {
    const o = G1.toObject(G1.toAffine(P));
    return Buffer.concat([beBytes(o[0]), beBytes(o[1])]);      // x ‖ y
  };
  // convention Ethereum / arkworks, celle des syscalls alt_bn128 :
  // x = (c1, c0) puis y = (c1, c0) — l'INVERSE de l'ordre du JSON snarkjs
  const serG2 = (P) => {
    const o = G2.toObject(G2.toAffine(P));
    return Buffer.concat([
      beBytes(o[0][1]), beBytes(o[0][0]),
      beBytes(o[1][1]), beBytes(o[1][0]),
    ]);
  };

  const bA = serG1(negA), bB = serG2(B), bC = serG1(C);

  // le constat : que vaut réellement chaque tranche de 32 octets ?
  const jb = pr.pi_b;
  const chunks = [
    ["proof_b[  0.. 32]", beRead(bB, 0),  { "pi_b[0][0] (c0)": BigInt(jb[0][0]), "pi_b[0][1] (c1)": BigInt(jb[0][1]) }],
    ["proof_b[ 32.. 64]", beRead(bB, 32), { "pi_b[0][0] (c0)": BigInt(jb[0][0]), "pi_b[0][1] (c1)": BigInt(jb[0][1]) }],
  ];
  for (const [label, val, cand] of chunks) {
    const hit = Object.entries(cand).find(([, v]) => v === val);
    console.log(`  ${label} = ${hit ? hit[0] : "??? aucune correspondance"}`);
  }
  const orderOK = beRead(bB, 0) === BigInt(jb[0][1]) && beRead(bB, 32) === BigInt(jb[0][0]);
  orderOK ? ok("G2 écrit bien (c1, c0) — convention Ethereum/alt_bn128")
          : ko("l'ordre G2 écrit ne correspond pas à la convention attendue");

  const negOK = beRead(bA, 0) === BigInt(pr.pi_a[0]);
  negOK ? ok("proof_a garde x inchangé (seul y est négativé)")
        : ko("x de proof_a a changé — négation incorrecte");
  const yNeg = beRead(bA, 32) !== BigInt(pr.pi_a[1]);
  yNeg ? ok("proof_a a bien un y différent de l'original") : ko("y de proof_a n'a pas été négativé");

  // ───────────────────────────── 3. tailles
  console.log("\n[3] Tailles attendues par groth16-solana");
  const sizes = [["proof_a", bA, 64], ["proof_b", bB, 128], ["proof_c", bC, 64]];
  for (const [nm, buf, want] of sizes)
    buf.length === want ? ok(`${nm} : ${buf.length} octets`) : ko(`${nm} : ${buf.length} ≠ ${want}`);

  const bIC = IC.map(serG1);
  bIC.every((b) => b.length === 64) ? ok(`vk_ic : ${bIC.length} × 64 octets`) : ko("vk_ic mal dimensionné");
  const bPub = pub.map((p) => beBytes(p));
  bPub.every((b) => b.length === 32) ? ok(`entrées publiques : ${bPub.length} × 32 octets`) : ko("entrées mal dimensionnées");

  // ───────────────────────────── 4. écriture des artefacts
  fs.mkdirSync(OUT, { recursive: true });
  fs.writeFileSync(path.join(OUT, "proof_a.bin"), bA);
  fs.writeFileSync(path.join(OUT, "proof_b.bin"), bB);
  fs.writeFileSync(path.join(OUT, "proof_c.bin"), bC);
  fs.writeFileSync(path.join(OUT, "public_inputs.bin"), Buffer.concat(bPub));

  const rustArr = (b) => "[" + Array.from(b).join(", ") + "]";
  const rs = `// Généré par to_solana.js — ne pas éditer à la main.
// Clé de vérification et preuve, au format attendu par groth16-solana.
// ⚠️ Cette clé provient d'une CÉRÉMONIE DE TEST. Ne jamais l'utiliser en production.

use groth16_solana::groth16::Groth16Verifyingkey;

pub const NR_PUBLIC_INPUTS: usize = ${pub.length};

pub const VERIFYINGKEY: Groth16Verifyingkey = Groth16Verifyingkey {
    nr_pubinputs: ${pub.length},
    vk_alpha_g1: ${rustArr(serG1(alpha))},
    vk_beta_g2: ${rustArr(serG2(beta))},
    vk_gamme_g2: ${rustArr(serG2(gamma))},
    vk_delta_g2: ${rustArr(serG2(delta))},
    vk_ic: &[
${bIC.map((b) => "        " + rustArr(b) + ",").join("\n")}
    ],
};

// Un exemplaire de preuve valide, pour les tests et la mesure du coût.
pub const PROOF_A: [u8; 64] = ${rustArr(bA)};
pub const PROOF_B: [u8; 128] = ${rustArr(bB)};
pub const PROOF_C: [u8; 64] = ${rustArr(bC)};
pub const PUBLIC_INPUTS: [[u8; 32]; ${pub.length}] = [
${bPub.map((b) => "    " + rustArr(b) + ",").join("\n")}
];
`;
  fs.writeFileSync(path.join(OUT, "verifying_key.rs"), rs);

  console.log(`\n[4] Écrit dans ${OUT}/ : proof_a.bin, proof_b.bin, proof_c.bin,`);
  console.log(`    public_inputs.bin, verifying_key.rs (${rs.length} octets)`);

  console.log("\n" + "=".repeat(62));
  console.log(`  RÉSULTAT : ${pass} réussis, ${fail} échoués`);
  console.log("=".repeat(62));

  await curve.terminate();
  process.exit(fail ? 1 : 0);
})();
