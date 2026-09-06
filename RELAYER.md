# Running a relayer

A relayer submits somebody else's confidential spend and gets paid in NX. This document
specifies what it must check, what it cannot do, and what it costs — so that someone other
than us can run one.

Everything below is measured on public devnet, against program
`BxCp94G2PD6y1xgnLgFCs3T8MtnyGpQ5SU6cms1oBWs1`.

---

## 1. Why a relayer exists at all

Without one, the user signs and pays for their own transaction. They then appear as the fee
payer and the signer — **they identify themselves at the last step**, after having gone to
the trouble of proving in zero knowledge. The relayer advances the SOL, submits on their
behalf, and is reimbursed out of the shielded pool.

Note what this does **not** apply to. Deposits are public: the depositor signs and transfers
from their own account. Paying for your own deposit costs you nothing in privacy, and it is
why there is no bootstrap problem — you only need a relayer for **spends**.

## 2. What a relayer cannot do, by construction

These are not promises, they are enforced by the program. A dishonest relayer gains nothing.

| it cannot | because |
|---|---|
| redirect the withdrawal to itself | the recipient is sealed in public input **[9]** and checked on chain |
| award itself a larger fee | the fee is public input **[3]**, and the program transfers exactly that |
| be paid in something other than NX | the program rejects any `tokenFrais` that is not the NX mint |
| take a fee outside the tiers | the program rejects any amount not in `DENOMINATIONS_FRAIS` |
| replay a spend | the nullifier root must match the current one — `racine des nullifieurs perimee` |
| learn anything | it receives a proof and public inputs. Never a note, a seed, or a hidden amount |

The circuit cannot enforce the NX rule itself: it proves the fee leaves slot 1, but it has
no way to know *which* token sits there. That check has to live in the program, and it does.

## 3. Public inputs, in order

⚠️ Indices shift from the 7th onwards compared with the single-token version. An error here
raises **no alert** — the proof stays valid, only the value read is wrong.

```
[0] commitment root      [1][2] new commitments   [3] fee
[4][5] nullifier roots before / after              [6] asset token
[7] fee token (NX)       [8] withdrawn amount      [9] recipient
[10][11] nullifiers
```

## 4. Fee tiers

The program accepts five amounts, in NX base units (6 decimals):

```
0   ·   10 000   ·   100 000   ·   1 000 000   ·   10 000 000
0   ·   0.01 NX  ·   0.1 NX    ·   1 NX        ·   10 NX
```

**Why tiers and not a free amount.** A free-form fee identifies whoever pays it, exactly as
free-form deposit amounts did — our own measurement on free amounts linked 98 % of
withdrawals by simple equality. That is already why deposits are restricted to four
denominations. A relayer that accepted arbitrary values would destroy the anonymity it is
selling.

Zero is allowed on purpose: it is the case where the user submits their own transaction and
therefore identifies themselves anyway. A paid relayer will refuse it.

**The protocol does not set a price.** It constrains the shape of the fee, not its amount.
Which tiers you accept is your only commercial decision.

## 5. Interface

`GET` returns the tariff, so a client can read it *before* building a proof:

```json
{ "programme": "BxCp94G2…", "mint_frais": "xZ6gLRio…",
  "paliers_acceptes": ["0","10000","100000","1000000","10000000"],
  "frais_minimum": "10000", "entrees_publiques": 12 }
```

`POST` submits a spend:

```json
{ "proofA": "<64 bytes, base64>", "proofB": "<128 bytes>", "proofC": "<64 bytes>",
  "publicInputs": "<12 × 32 bytes>",
  "mintActif": "<base58>", "beneficiaire": "<base58 token account>",
  "memo": "<optional, ≤ 200 chars>" }
```

`mintActif` and `beneficiaire` must be supplied because the field encoding zeroes the top
byte of an address: the full address cannot be recovered from the proof. The relayer checks
them against public inputs [6] and [9] and refuses on mismatch.

**Checks a relayer should perform, cheapest first**, so that a bad request costs no network
call: sizes → fee is a tier → fee ≥ your minimum → fee token is NX → announced addresses
match the proof → simulate → send. Simulating before sending matters: an invalid proof would
otherwise cost you the transaction fee for nothing.

**Derive every account, copy none.** The pool is a PDA of the program; the vaults and the
relayer's own token account are associated token accounts. An early version of our relayer
carried an address copied from a *truncated* display and failed with `IllegalOwner` after
burning 138 463 compute units — the proof was fine, the account was not.

## 6. What it costs, measured

| | |
|---|---|
| base fee (1 signature) | 5 000 lamports ≈ $0.0005 |
| priority fee (400 000 CU, normal → congested) | ≈ $0.0004 → $0.004 |
| rent created | **0** |
| compute units for a full spend | ~209 700 of 1 400 000 |

**The marginal cost is a tenth of a cent. Fixed costs decide.** A reliable RPC endpoint and a
host are what you actually pay for. Against $50/month of fixed cost:

| fee charged | transactions/month to break even |
|---|---|
| $0.01 | ~5 500 |
| $0.05 | ~1 020 |
| $0.10 | ~505 |

For scale: Tornado Cash relayers charged 0.3–1 % of the amount moved — three to ten dollars
on a $1 000 transfer, a thousand times the network cost. Privacy is not priced off gas.

## 7. The honest risk

You advance SOL and are reimbursed in NX. **If NX is thinly traded you are accumulating an
illiquid asset against a liquid expense.** That inventory risk, not the network cost, is what
decides whether running a relayer makes sense. Three ways out, in order of realism: run it as
a loss while the pool is small; index your minimum to the NX/SOL rate with a liquidity
premium; or convert to SOL on every transaction, which needs a market with depth.

We currently run the only relayer, and absorb that loss. We would rather say so than imply a
market exists.

## 8. Status

The reference relayer runs on devnet with a minimum of 10 000 units (0.01 NX). Verified in
both directions: a valid tier is accepted and paid, an out-of-tier amount is refused before
any network call — on a proof that is otherwise mathematically valid.

Not solved: the price itself, and the inventory risk above.
