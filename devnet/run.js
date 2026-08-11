#!/usr/bin/env node
/*
 * run.js — le cycle complet sur une vraie chaine, AVEC de vrais jetons.
 *
 *   1. mint NX de test + comptes de jetons
 *   2. Initialize        l'arbre vide
 *   3. Shield(100 NX)    dépôt réel : les jetons entrent dans le coffre
 *   4. Shield(100 NX)    idem
 *   5. Pour(preuve)      dépense confidentielle + FRAIS PAYÉS EN NX au relayeur
 *   6. rejeu             doit échouer
 *
 * Ce que ce script vérifie, et qui n'avait jamais été vérifié :
 *   · le coffre contient bien 200 NX après les dépôts ;
 *   · le programme recalcule lui-même l'engagement à partir du montant
 *     (impossible de déposer 1 en déclarant 1 000) ;
 *   · après le Pour, le coffre a diminué EXACTEMENT des frais, et le relayeur
 *     les a reçus — sans que personne n'apprenne qui a payé.
 *
 * Usage : node run.js <PROGRAM_ID> [chemin/keypair.json]
 */

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const {
  Connection, Keypair, PublicKey, Transaction, TransactionInstruction,
  SystemProgram, sendAndConfirmTransaction,
} = require("@solana/web3.js");
const {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo,
  getAssociatedTokenAddressSync, createAssociatedTokenAccountInstruction,
  getAccount,
} = require("@solana/spl-token");

const RPC = process.env.RPC_URL || "https://api.devnet.solana.com";
const TAG = { VERIFY: 0, INIT: 1, POUR: 2, SHIELD: 3 };
const NX = 1_000_000;

const programId = new PublicKey(process.argv[2]);
// Sans second argument, on prend la clé par défaut de la CLI Solana — c'est
// celle que la CI provisionne, et celle qu'un développeur a déjà sous la main.
const kpPath = process.argv[3] || path.join(process.env.HOME, ".config/solana/id.json");
const dossier = process.argv[3] ? path.dirname(kpPath) : __dirname;
const payeur = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(kpPath))));
const sc = JSON.parse(fs.readFileSync(path.join(__dirname, "scenario.json")));

const cx = new Connection(RPC, "confirmed");
const hex = (s) => Buffer.from(s, "hex");
const b64 = (s) => Buffer.from(s, "base64");
// Un lien d'explorateur ne veut rien dire sur un validateur local : personne
// ne peut le suivre. On n'en affiche que si l'on parle vraiment à devnet.
const RESEAU = RPC.includes("devnet") ? "devnet" : RPC.includes("mainnet") ? "mainnet" : "validateur local";
const lien = (s) =>
  RESEAU === "devnet" ? `https://explorer.solana.com/tx/${s}?cluster=devnet` : s;
const nx = (n) => (Number(n) / NX).toFixed(5);
const meta = (p, s, w) => ({ pubkey: p, isSigner: s, isWritable: w });

async function envoyer(nom, ix, signataires = [payeur]) {
  const sig = await sendAndConfirmTransaction(cx, new Transaction().add(ix), signataires, { commitment: "confirmed" });
  console.log(`  ✅ ${nom}\n     ${lien(sig)}`);
  return sig;
}

(async () => {
  console.log("═".repeat(68));
  console.log(`  Cycle complet avec de vrais jetons — ${RESEAU}`);
  console.log("═".repeat(68));
  console.log(`  programme : ${programId.toBase58()}`);
  console.log(`  payeur    : ${payeur.publicKey.toBase58()}`);

  const [pool] = PublicKey.findProgramAddressSync([Buffer.from("pool")], programId);

  // ── 1. le jeton NX de test — adresse DÉTERMINISTE
  //
  // Depuis la v4, le jeton entre dans l'engagement et dans l'énoncé public.
  // La preuve est générée hors ligne, avant que ce script ne tourne : elle
  // doit donc porter sur un mint dont l'adresse est connue d'avance. On la
  // dérive d'une graine fixe, publiée dans le scénario — n'importe qui peut
  // la recalculer et rejouer le cycle à l'identique.
  const graineMint = crypto.createHash("sha256").update(sc.mint_test_graine).digest();
  const cleMint = Keypair.fromSeed(graineMint);
  const mint = cleMint.publicKey;
  if (mint.toBase58() !== sc.mint_test_base58) {
    throw new Error(`mint derive ${mint.toBase58()} != mint du scenario ${sc.mint_test_base58}`);
  }
  if (await cx.getAccountInfo(mint)) {
    console.log(`  mint NX   : ${mint.toBase58()} (deja en place)`);
  } else {
    await createMint(cx, payeur, payeur.publicKey, null, 6, cleMint);
    console.log(`  mint NX   : ${mint.toBase58()} (cree, 6 decimales)`);
  }
  fs.writeFileSync(path.join(dossier, "mint-nx.json"), JSON.stringify({ mint: mint.toBase58() }));

  // comptes de jetons : déposant, coffre (autorité = PDA du pool), relayeur
  const compteDeposant = await getOrCreateAssociatedTokenAccount(cx, payeur, mint, payeur.publicKey);
  const coffre = getAssociatedTokenAddressSync(mint, pool, true);
  if (!(await cx.getAccountInfo(coffre))) {
    await envoyer("coffre cree (autorite = PDA du pool)",
      createAssociatedTokenAccountInstruction(payeur.publicKey, coffre, pool, mint));
  }
  const compteRelayeur = getAssociatedTokenAddressSync(mint, payeur.publicKey, false);

  // Le bénéficiaire du retrait : déterministe lui aussi, car son compte est
  // scellé dans la preuve. On crée le compte s'il n'existe pas — n'importe qui
  // peut le faire, ça ne donne aucun droit dessus.
  const beneficiaire = Keypair.fromSeed(
    crypto.createHash("sha256").update(sc.beneficiaire_graine).digest()
  ).publicKey;
  const compteBenef = getAssociatedTokenAddressSync(mint, beneficiaire, false);
  if (compteBenef.toBase58() !== sc.beneficiaire_compte_base58) {
    throw new Error(`compte beneficiaire ${compteBenef.toBase58()} != scenario ${sc.beneficiaire_compte_base58}`);
  }
  if (!(await cx.getAccountInfo(compteBenef))) {
    await envoyer("compte du beneficiaire cree",
      createAssociatedTokenAccountInstruction(payeur.publicKey, compteBenef, beneficiaire, mint));
  }

  const totalDepose = Number(sc.total_depose);
  const soldeDeposant = Number((await getAccount(cx, compteDeposant.address)).amount);
  if (soldeDeposant < totalDepose) {
    await mintTo(cx, payeur, mint, compteDeposant.address, payeur, totalDepose - soldeDeposant);
    console.log(`  ${nx(totalDepose - soldeDeposant)} NX frappes pour le deposant`);
  }
  console.log(`  coffre    : ${coffre.toBase58()}\n`);

  // ── 2. Initialize
  if (!(await cx.getAccountInfo(pool))) {
    await envoyer("Initialize", new TransactionInstruction({
      programId,
      keys: [meta(payeur.publicKey, true, true), meta(pool, false, true), meta(SystemProgram.programId, false, false)],
      data: Buffer.from([TAG.INIT]),
    }));
  } else console.log("  ℹ️  pool deja initialise");

  // ── 3 et 4. les dépôts, avec de vrais jetons
  const avantCoffre = Number((await getAccount(cx, coffre)).amount);
  for (let i = 0; i < sc.depots.length; i++) {
    const d = sc.depots[i];
    const montant = Buffer.alloc(8);
    montant.writeBigUInt64LE(BigInt(d.montant));
    await envoyer(`Shield ${nx(d.montant)} NX`, new TransactionInstruction({
      programId,
      keys: [
        meta(payeur.publicKey, true, true), meta(pool, false, true), meta(coffre, false, true),
        meta(compteDeposant.address, false, true), meta(TOKEN_PROGRAM_ID, false, false),
      ],
      data: Buffer.concat([Buffer.from([TAG.SHIELD]), montant, hex(d.k_hex)]),
    }));
  }

  const apresDepots = Number((await getAccount(cx, coffre)).amount);
  console.log(`\n  coffre : ${nx(avantCoffre)} -> ${nx(apresDepots)} NX`);
  if (apresDepots - avantCoffre !== totalDepose) throw new Error("le coffre n'a pas recu le bon montant");
  console.log("  ✅ les jetons sont bien entres dans le coffre");

  // la racine doit correspondre à celle du client
  const racine = (await cx.getAccountInfo(pool)).data.subarray(0, 32).toString("hex");
  console.log(`\n  racine on-chain : ${racine}`);
  console.log(`  racine attendue : ${sc.racine_attendue_hex}`);
  if (racine !== sc.racine_attendue_hex) throw new Error("les deux arbres divergent");
  console.log("  ✅ meme racine — le programme a recalcule les engagements lui-meme");

  // la racine des nullifieurs doit être celle de l'arbre vide
  const nfAvant = (await cx.getAccountInfo(pool)).data.subarray(32, 64).toString("hex");
  console.log(`\n  racine nullifieurs on-chain : ${nfAvant}`);
  console.log(`  racine nullifieurs attendue : ${sc.nf_racine_avant_hex}`);
  if (nfAvant !== sc.nf_racine_avant_hex) throw new Error("l'arbre des nullifieurs ne part pas du bon etat");
  console.log("  ✅ meme point de depart pour les nullifieurs");

  // ── 5. la dépense confidentielle, frais payés en NX
  //
  // Cinq comptes, plus aucun pour les nullifieurs : ils vivent maintenant
  // dans 32 octets du compte du pool, réécrits à chaque swap. C'est toute la
  // différence entre un relayeur qui perd 0,36 $ par transaction et un
  // relayeur qui n'immobilise rien.
  const comptes = [
    meta(payeur.publicKey, true, true), meta(pool, false, true),
    meta(coffre, false, true), meta(compteRelayeur, false, true),
    meta(compteBenef, false, true), meta(TOKEN_PROGRAM_ID, false, false),
  ];
  const data = Buffer.concat([
    Buffer.from([TAG.POUR]),
    b64(sc.proof_a_b64), b64(sc.proof_b_b64), b64(sc.proof_c_b64), b64(sc.public_inputs_b64),
  ]);

  const relAvant = Number((await getAccount(cx, compteRelayeur)).amount);
  const benefAvant = Number((await getAccount(cx, compteBenef)).amount);
  const solAvant = await cx.getBalance(payeur.publicKey);
  const sigPour = await envoyer("Pour — depense confidentielle", new TransactionInstruction({ programId, keys: comptes, data }));
  const solApres = await cx.getBalance(payeur.publicKey);

  const coffreApres = Number((await getAccount(cx, coffre)).amount);
  const relApres = Number((await getAccount(cx, compteRelayeur)).amount);
  const frais = Number(sc.frais);

  const benefApres = Number((await getAccount(cx, compteBenef)).amount);
  const retrait = Number(sc.montant_retrait);

  console.log(`\n  coffre       : ${nx(apresDepots)} -> ${nx(coffreApres)} NX  (-${nx(apresDepots - coffreApres)})`);
  console.log(`  relayeur     : ${nx(relAvant)} -> ${nx(relApres)} NX  (+${nx(relApres - relAvant)})`);
  console.log(`  beneficiaire : ${nx(benefAvant)} -> ${nx(benefApres)} NX  (+${nx(benefApres - benefAvant)})`);
  if (relApres - relAvant !== frais) throw new Error("le relayeur n'a pas recu les frais");
  if (benefApres - benefAvant !== retrait) throw new Error("le beneficiaire n'a pas recu le retrait");
  if (apresDepots - coffreApres !== frais + retrait) throw new Error("le coffre n'a pas diminue de frais + retrait");
  console.log(`  ✅ ${nx(frais)} NX au relayeur — $NX a servi`);
  console.log(`  ✅ ${nx(retrait)} NX SORTIS du pool vers un compte public — le retrait fonctionne`);
  console.log(`  ✅ le coffre a diminue d'exactement frais + retrait : rien ne fuit, rien ne se cree`);
  console.log(`     (les 149,99999 NX restants sont deux notes dont personne ne connait les montants)`);

  // ── 6. LE CHIFFRE DE CETTE PHASE : le rent par swap
  //
  // Avant, chaque swap créait deux comptes de nullifieur : 0,0018 SOL que le
  // relayeur ne revoyait jamais. Ici il ne doit rester que les frais de
  // réseau, 5 000 lamports par signature — récurrents, pas immobilisés.
  const coutSol = solAvant - solApres;
  const FRAIS_RESEAU = 5000;
  const rent = coutSol - FRAIS_RESEAU;
  console.log(`\n  SOL du relayeur : ${(coutSol / 1e9).toFixed(9)} depense au total`);
  console.log(`    dont frais de reseau : ${(FRAIS_RESEAU / 1e9).toFixed(9)} SOL`);
  console.log(`    dont rent immobilise : ${(rent / 1e9).toFixed(9)} SOL`);
  if (rent > 0) throw new Error(`il reste ${rent} lamports de rent par swap`);
  console.log("  ✅ RENT PAR SWAP = 0 — un relayeur independant devient tenable");

  // la racine des nullifieurs doit avoir avancé, exactement là où le circuit l'a dit
  const nfApres = (await cx.getAccountInfo(pool)).data.subarray(32, 64).toString("hex");
  console.log(`\n  racine nullifieurs : ${nfAvant.slice(0, 16)}… -> ${nfApres.slice(0, 16)}…`);
  if (nfApres !== sc.nf_racine_apres_hex) throw new Error("la racine des nullifieurs n'est pas celle prouvee");
  console.log("  ✅ la chaine a enregistre exactement la racine que le circuit a prouvee");

  // ── 7. rejeu
  console.log("\n  Rejouer la meme preuve doit echouer.");
  try {
    await envoyer("Pour rejoue", new TransactionInstruction({ programId, keys: comptes, data }));
    throw new Error("LA DOUBLE DEPENSE EST PASSEE");
  } catch (e) {
    if (String(e.message).includes("DOUBLE DEPENSE EST PASSEE")) throw e;
    console.log("  ✅ refuse (la preuve part d'une racine de nullifieurs perimee)");
  }

  // ── 8. LA FAILLE DOIT ÊTRE FERMÉE, ET ÇA SE DÉMONTRE
  //
  // Écrire un contrôle ne prouve rien : ce qui prouve, c'est de rejouer
  // l'attaque et de la voir refusée. Ici on tente exactement ce qui marchait
  // avant le 10 août 2026 — déposer en désignant son PROPRE compte de jetons
  // comme coffre. Le programme recalcule l'adresse associée du pool ; le
  // compte fourni n'y correspond pas ; la transaction doit échouer.
  console.log("\n  L attaque du faux coffre doit echouer.");
  {
    const d = sc.depots[0];
    const montant = Buffer.alloc(8);
    montant.writeBigUInt64LE(BigInt(d.montant));
    try {
      await envoyer("Shield vers un faux coffre", new TransactionInstruction({
        programId,
        keys: [
          meta(payeur.publicKey, true, true), meta(pool, false, true),
          meta(compteDeposant.address, false, true),   // ← le coffre de l attaquant
          meta(compteDeposant.address, false, true), meta(TOKEN_PROGRAM_ID, false, false),
        ],
        data: Buffer.concat([Buffer.from([TAG.SHIELD]), montant, hex(d.k_hex)]),
      }));
      throw new Error("LE FAUX COFFRE EST PASSE");
    } catch (e) {
      if (String(e.message).includes("LE FAUX COFFRE EST PASSE")) throw e;
      console.log("  ✅ refuse — le coffre est recalcule, pas accepte sur parole");
    }
  }

  const tx = await cx.getTransaction(sigPour, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  console.log("\n" + "═".repeat(68));
  console.log(`  swap confidentiel + frais en NX, sur ${RESEAU}`);
  console.log(`  signature : ${sigPour}`);
  console.log(`  ${lien(sigPour)}`);
  console.log(`  unites de calcul : ${tx?.meta?.computeUnitsConsumed}`);
  console.log("═".repeat(68));

  fs.writeFileSync(path.join(dossier, "resultat-jetons.json"), JSON.stringify({
    programId: programId.toBase58(), pool: pool.toBase58(), mint: mint.toBase58(),
    coffre: coffre.toBase58(), signaturePour: sigPour,
    coffreFinal: coffreApres, fraisVersesAuRelayeur: frais,
    computeUnits: tx?.meta?.computeUnitsConsumed ?? null,
    solDepenseParSwap: coutSol, rentImmobiliseParSwap: rent,
    nfRacineAvant: nfAvant, nfRacineApres: nfApres,
  }, null, 2));
})().catch((e) => {
  console.error("\n❌ echec :", e.message);
  if (e.logs) console.error(e.logs.join("\n"));
  process.exit(1);
});
