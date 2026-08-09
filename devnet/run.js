#!/usr/bin/env node
/*
 * run.js — le cycle complet sur devnet, en quatre transactions.
 *
 *   1. Initialize     l'arbre vide
 *   2. Shield(cm0)    dépôt de la première note
 *   3. Shield(cm1)    dépôt de la seconde   → l'arbre a maintenant 2 feuilles
 *   4. Pour(preuve)   dépense les deux notes, sans révéler quoi que ce soit
 *
 * La preuve de l'étape 4 a été produite HORS LIGNE, contre un arbre contenant
 * exactement ces deux feuilles. Si le programme calcule une racine différente
 * de celle du client, l'étape 4 est refusée — c'est le test qui compte.
 *
 * Usage : node run.js <PROGRAM_ID> [chemin/keypair.json]
 */

const fs = require("fs");
const path = require("path");
const {
  Connection, Keypair, PublicKey, Transaction, TransactionInstruction,
  SystemProgram, sendAndConfirmTransaction,
} = require("@solana/web3.js");

const RPC = process.env.RPC_URL || "https://api.devnet.solana.com";
const TAG = { VERIFY: 0, INIT: 1, POUR: 2, SHIELD: 3 };

const programId = new PublicKey(process.argv[2]);
const kpPath = process.argv[3] || path.join(process.env.HOME, ".config/solana/id.json");
const payeur = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(kpPath))));

const sc = JSON.parse(fs.readFileSync(path.join(__dirname, "scenario.json")));
const b64 = (s) => Buffer.from(s, "base64");
const hex = (s) => Buffer.from(s, "hex");

const cx = new Connection(RPC, "confirmed");
const lien = (s) => `https://explorer.solana.com/tx/${s}?cluster=devnet`;

async function envoyer(nom, ix) {
  const tx = new Transaction().add(ix);
  const sig = await sendAndConfirmTransaction(cx, tx, [payeur], {
    commitment: "confirmed",
    skipPreflight: false,
  });
  console.log(`  ✅ ${nom}`);
  console.log(`     ${sig}`);
  console.log(`     ${lien(sig)}`);
  return sig;
}

(async () => {
  console.log("═".repeat(66));
  console.log("  Cycle complet dépôt → dépense confidentielle, sur devnet");
  console.log("═".repeat(66));
  console.log(`  programme : ${programId.toBase58()}`);
  console.log(`  payeur    : ${payeur.publicKey.toBase58()}`);
  console.log(`  solde     : ${(await cx.getBalance(payeur.publicKey)) / 1e9} SOL`);

  const [pool] = PublicKey.findProgramAddressSync([Buffer.from("pool")], programId);
  console.log(`  pool (PDA): ${pool.toBase58()}\n`);

  const signatures = {};

  // ── 1. Initialize
  const dejaLa = await cx.getAccountInfo(pool);
  if (dejaLa) {
    console.log("  ℹ️  le pool existe déjà, initialisation sautée");
  } else {
    signatures.initialize = await envoyer("Initialize", new TransactionInstruction({
      programId,
      keys: [
        { pubkey: payeur.publicKey, isSigner: true, isWritable: true },
        { pubkey: pool, isSigner: false, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: Buffer.from([TAG.INIT]),
    }));
  }

  // ── 2 et 3. les deux dépôts
  for (let i = 0; i < sc.depots_hex.length; i++) {
    signatures[`shield_${i}`] = await envoyer(`Shield ${i}`, new TransactionInstruction({
      programId,
      keys: [{ pubkey: pool, isSigner: false, isWritable: true }],
      data: Buffer.concat([Buffer.from([TAG.SHIELD]), hex(sc.depots_hex[i])]),
    }));
  }

  // ── contrôle : la racine on-chain est-elle celle que le client attend ?
  const info = await cx.getAccountInfo(pool);
  const racineOnChain = info.data.subarray(0, 32).toString("hex");
  const attendue = sc.racine_attendue_hex;
  console.log(`\n  racine on-chain : ${racineOnChain}`);
  console.log(`  racine attendue : ${attendue}`);
  if (racineOnChain !== attendue) {
    console.error("\n  ❌ LES DEUX ARBRES DIVERGENT — la preuve sera refusée.");
    console.error("     Cause probable : DEPTH ou la fonction de hachage diffèrent");
    console.error("     entre pour.circom et pool.rs.");
    process.exit(1);
  }
  console.log("  ✅ le programme et le client calculent la MÊME racine\n");

  // ── 4. la dépense confidentielle
  const [nf0] = PublicKey.findProgramAddressSync(
    [Buffer.from("nf"), hex(sc.nullifieurs_hex[0])], programId);
  const [nf1] = PublicKey.findProgramAddressSync(
    [Buffer.from("nf"), hex(sc.nullifieurs_hex[1])], programId);

  const data = Buffer.concat([
    Buffer.from([TAG.POUR]),
    b64(sc.proof_a_b64), b64(sc.proof_b_b64), b64(sc.proof_c_b64),
    b64(sc.public_inputs_b64),
  ]);

  signatures.pour = await envoyer("Pour (dépense confidentielle)", new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payeur.publicKey, isSigner: true, isWritable: true },
      { pubkey: pool, isSigner: false, isWritable: true },
      { pubkey: nf0, isSigner: false, isWritable: true },
      { pubkey: nf1, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  }));

  // ── 5. rejouer la même preuve doit échouer (double dépense)
  console.log("\n  Contrôle final : rejouer la MÊME preuve doit être refusé.");
  try {
    await envoyer("Pour rejoué", new TransactionInstruction({
      programId,
      keys: [
        { pubkey: payeur.publicKey, isSigner: true, isWritable: true },
        { pubkey: pool, isSigner: false, isWritable: true },
        { pubkey: nf0, isSigner: false, isWritable: true },
        { pubkey: nf1, isSigner: false, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data,
    }));
    console.error("  ❌ LA DOUBLE DÉPENSE EST PASSÉE — le registre ne protège rien.");
    process.exit(1);
  } catch (e) {
    console.log("  ✅ refusé, comme attendu (le nullifieur existe déjà)");
  }

  const cu = await cx.getTransaction(signatures.pour, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  const consomme = cu?.meta?.computeUnitsConsumed;

  console.log("\n" + "═".repeat(66));
  console.log("  RÉSULTAT");
  console.log("═".repeat(66));
  console.log(`  swap confidentiel accepté sur devnet`);
  console.log(`  signature : ${signatures.pour}`);
  console.log(`  ${lien(signatures.pour)}`);
  if (consomme) console.log(`  unités de calcul consommées (Pour complet) : ${consomme}`);
  console.log("═".repeat(66));

  fs.writeFileSync(path.join(__dirname, "resultat.json"),
    JSON.stringify({ programId: programId.toBase58(), pool: pool.toBase58(), signatures, computeUnits: consomme ?? null }, null, 2));
})().catch((e) => {
  console.error("\n❌ échec :", e.message);
  if (e.logs) console.error(e.logs.join("\n"));
  process.exit(1);
});
