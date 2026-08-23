# Confidential Swap Kit

An open-source toolkit for **confidential token transfers on Solana**: deposit SPL tokens into a shielded pool, move value inside it without revealing amounts or parties, withdraw to a public account — and pay the relayer in a token instead of SOL.

Everything below is **measured by CI on a real Solana validator**, at every commit. No estimates.

```
Groth16 verification on-chain          136 477 CU      9.8 % of the 1.4 M budget
Full spend: verify + registry + fee
+ withdrawal                           209 622 CU     15.0 %
Rent immobilised per swap                    0 SOL
Capacity                             1 048 576 notes / 1 048 576 nullifiers
Circuit                                 55 496 constraints, 11 public inputs
Proof generation (Node)                       7 s
Proof generation on a Solana Seeker          5.8 s     measured on the device
```

The nullifiers are public inputs. Without that, the nullifier tree cannot be rebuilt from
the chain and a **second** spend is impossible — the flaw stayed invisible until a client
was written, because every demo restarted from a fresh pool. Publishing them costs nothing
in privacy: `nf = Poseidon(ask, rho)` links to no note, no owner and no amount. It is the
Zcash and Tornado Cash choice.

MIT licensed. `ZNGLABS/confidential-swap-kit`.

## Live on public devnet

Program `6N5k5xnVU1gNxoydLKb7YprsuJYV7k9i1B3ojbyU7PsM`. Two transactions anyone can read:

A confidential swap **proved on a phone** and submitted by a relayer, so the sender's
address is nowhere in the transaction —
`3S6cAWomFKGiyR6HVG8zATiutaEkRuLWdQRJ8XTN8n6oazp3xdQVc5q779kchSTWDqWmQChXPfCVe2weQfTtx4CK`
(slot 484 786 576, 232 321 CU, one signer, fee paid by the relayer, program log
`pour accepte — index 28`).

A confidential swap signed from a **browser** with a consumer wallet —
`4uBrGwwzgS3Z7ALprydojfwrHtmbXSZmwZPFZqA6iLRRqxNFw3Ur8zxjAPoAhi5pfaYdJjGZahkPUVaPfY9Ve1tw`.

> **We need 3 to 5 independent contributors for the trusted setup ceremony.**
> About three minutes each. One honest participant is enough to make the key
> permanently safe — see [issue #1](https://github.com/ZNGLABS/confidential-swap-kit/issues/1).

---

## What the CI proves, on its own, at every commit

The workflow builds the program, deploys it to a local Solana validator and replays the full cycle. Ten properties are asserted — the run fails if any of them breaks:

1. deposited SPL tokens actually land in the pool's vault;
2. the commitment root computed **on-chain** equals the one the client computed off-chain;
3. the nullifier root starts from the empty-tree value;
4. the proof is accepted by the on-chain verifier;
5. the relayer is paid its fee, in tokens, out of the pool;
6. **the withdrawal reaches the exact account sealed in the proof**;
7. the vault decreases by exactly `fee + withdrawal` — nothing leaks, nothing is created;
8. **rent immobilised is zero** (only the 5 000-lamport network fee is spent);
9. replaying the same proof is **rejected by the chain**;
10. the "forged vault" attack is **rejected** — the vault address is recomputed, never trusted.

Reproduce it yourself: fork, push, read the job summary. No key, no SOL, no trust in us required.

---

## Three results worth knowing

**Nullifiers cost no rent.** The naive design stores one account per nullifier: 0.0009 SOL each, 0.0018 SOL per swap, immobilised forever — about $0.36 at $200/SOL, against a fee of 0.00001 tokens. No independent relayer survives that. Replacing the accounts with an **indexed Merkle tree**, whose root lives in 32 bytes of the pool account, brings rent to exactly zero. The circuit proves both that the nullifiers were absent and that they are now inserted; the chain only compares two roots.

**Enriching the circuit is free on-chain.** Going from depth 6 to depth 20 tripled the circuit — 15 712 to 55 496 constraints — and multiplied capacity by 16 384. Verification moved from 125 055 to **125 225 CU: +0.14 %**. The Groth16 pairing does not depend on constraint count, only on the number of public inputs. Going from 9 to 11 public inputs then moved verification from 125 225 to 136 477 CU — **5 626 CU per public input, measured**, not the 4 174 we first estimated. Sizing the tree is therefore a ceremony question; sizing the public interface is the one that costs.

**Sizing is arithmetic, not guesswork:**

```
constraints ≈ 15 712 + (depth − 6) × 2 426
```

Depth 20 → 55 496, fits a 2^16 ceremony. Depth 24 → ~65 200, still 2^16. Beyond that, 2^17 — a bigger machine, not a redesign.

---

## What is deliberately public, and why

| Public | Hidden |
|---|---|
| the asset being moved | the amount |
| the fee paid to the relayer | the sender |
| the withdrawal amount and destination | the recipient |
| deposit amounts (fixed denominations) | the link between a deposit and a withdrawal |

The fee must be public so a relayer can verify it is paid. The asset must be public so the chain knows which vault to draw from and can check that vault is its own — Tornado Cash separates its pools by asset, which leaks the asset anyway.

**The honest privacy claim.** Against a public AMM the amount *cannot* be hidden: we simulated 1 000 deposit/withdrawal pairs and recovered 5 out of 5 swap amounts by simple subtraction of pool reserves, even with a perfectly confidential transaction. What this design provides is **fungibility, not invisibility** — nobody can link an amount back to *you*. That is the Zcash/Tornado property, and it is the only one we claim.

And it depends on volume: an anonymity set of one is no anonymity at all. Shipping a privacy pool with two users would be worse than shipping nothing, because it grants false assurance.

---

## Architecture

```
circuit/          Circom: the `pour` of Zerocash + indexed nullifier tree
                  Groth16 over BN254, Poseidon hashing
solana-program/   Rust: 4 instructions — VerifyOnly, Initialize, Shield, Pour
                  registry = incremental Merkle tree + 32-root history
devnet/           Node client: replays the full cycle and asserts the 10 properties
.github/          the CI that measures everything above
```

The note commitment is two-layered, and the token sits in the **outer** layer:

```
k  = Poseidon(apk, ρ, r)            opaque: reveals neither owner nor randomness
cm = Poseidon(value, token, k)      the chain can recompute it
```

A deposit is public: the chain sees the amount and the mint. It must therefore be able to verify that the inserted commitment carries *that* amount and *that* token — otherwise you deposit a worthless token, declare a note denominated in a valuable one, and drain the vault on withdrawal. The depositor publishes only `k`; the program computes `cm` itself. What matters is proven, not declared.

Value conservation covers all three exits in a single equation over **private** signals:

```
Σ inputs = Σ outputs + fee + withdrawal
```

---

## What is NOT done — read this before using anything here

- **The phase-2 ceremony is a TEST** — and we are [looking for contributors to fix that](https://github.com/ZNGLABS/confidential-swap-kit/issues/1). Its entropy is known to a single machine. Anyone holding it can forge proofs and mint value from nothing. Phase 1 is the real public Hermez Perpetual Powers of Tau (2^16); phase 2 must be redone as a multi-party ceremony before any real value depends on this.
- **No audit.** A zk circuit plus a program that custodies other people's tokens. Non-negotiable before mainnet.
- **The client exists, note management does not.** Proofs are now generated in the browser and on-device (5.8 s on a Seeker), and a full swap has been signed by a consumer wallet. What is still missing is note management: notes are recovered by scanning the chain and reading an encrypted memo, and **losing a note means losing the funds**. Until that is a product rather than a script, this is not safe for non-technical users.
- **The fee is paid in the transferred token.** So "the fee is paid in $NX" holds only for $NX notes today. Paying an $NX fee on a USDC transfer requires per-token conservation and a second note: feasible, not built.
- **`Shield` is permissionless** by design, with fixed denominations (1 / 10 / 100 / 1 000 units) to avoid the amount-matching leak.
- **No cross-token swap at a constrained price.** `pour` moves value; it does not exchange one asset for another. Doing that confidentially needs either a counterparty inside the pool or an exit to a public AMM — and the latter reveals the amount. This is the genuinely hard, unsolved part.

---

## Development notes worth stealing

Portage of a Groth16 proof to Solana breaks silently in three places, all verified here rather than assumed (`circuit/to_solana.js`, 11 checks): everything is 32-byte **big-endian**; `pi_a` must be **negated** because the verification equation uses −A; and each Fq2 coordinate of G2 is written **(c1, c0)** — the reverse of snarkjs's JSON order. The test re-reads each 32-byte slice as a raw big-endian integer and compares it to the decimal in the original JSON, so the byte order is *observed*, not deduced. Re-reading with our own reader would have validated any self-consistent convention.

Two scaling traps that work perfectly at toy size and make growth impossible: computing the empty nullifier root by walking all 2^depth leaves (8 hashes at depth 3, 16 383 at depth 14 — `Initialize` would exceed the compute budget), and holding the nullifier tree as a dense array in the witness generator (7 million Poseidon hashes per scenario). Both are now O(depth): the empty tree has a single non-zero leaf, and the sparse tree only stores nodes that differ from "empty subtree".

One circom trap: **a public input used in no constraint is eliminated by the compiler**, and the guarantee it was supposed to seal disappears with it. The withdrawal recipient is anchored in a dummy constraint for exactly this reason — without it a relayer, who sees the proof first, could redirect the withdrawal to itself.

---

*ZNG Labs. Built in the open, with the numbers.*
