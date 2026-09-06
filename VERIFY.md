# Check every claim yourself

Every number in this repository is meant to be checked, not believed. Below are the exact
commands, in increasing order of effort. The first four need nothing but Python.

---

## 1. The transactions did what we say

Save this as `verify.py`:

```python
import json, sys, urllib.request
RPC = "https://api.devnet.solana.com"

def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    req = urllib.request.Request(RPC, body.encode(), {"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req))["result"]

d = rpc("getTransaction", [sys.argv[1], {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}])
m, msg = d["meta"], d["transaction"]["message"]

signers = [k["pubkey"] for k in msg["accountKeys"] if k.get("signer")]
print("signers :", len(signers), "->", signers[0])
print("CU      :", m["computeUnitsConsumed"], "  error:", m["err"])

pre = {b["accountIndex"]: float(b["uiTokenAmount"]["uiAmountString"]) for b in m["preTokenBalances"]}
print("token balance moves:")
for b in m["postTokenBalances"]:
    a = pre.get(b["accountIndex"], 0.0)
    p = float(b["uiTokenAmount"]["uiAmountString"])
    if a != p:
        print("   %s... %s -> %s (%+.5f)" % (b["mint"][:10], a, p, p - a))

print("accounts in the transaction:", len(msg["accountKeys"]))
for k in msg["accountKeys"]:
    print("   ", k["pubkey"])
```

Then:

```
python3 verify.py 5pERDUaLbB5UXKk6CcSnmAYUa1PgVWVAHnRitaeMuJAaGZUSXrRX4k91pbPQV6nQr6v94jgEPvAbjmZc6n1DUsFc
```

Output:

```
signers : 1 -> 2YWz9sLrFco2LSh9MMh4ae2XCSDQegmUjURYGAdy9JDw
CU      : 209706   error: None
token balance moves:
   HnpMSSfgBk... 1100.0 -> 1090.0 (-10.00000)          asset vault: the withdrawal
   xZ6gLRio6B... 1100.0 -> 1099.99999 (-0.00001)       NX vault: the fee, and nothing else
   xZ6gLRio6B... 0.0 -> 1e-05 (+0.00001)               the relayer, paid in NX
   HnpMSSfgBk... 369.570252 -> 379.570252 (+10.00000)  the recipient
accounts in the transaction: 9
```

Two things to look at.

**The asset vault was not touched for the fee.** That is the entire claim of this version,
and it is visible in four numbers.

**The sender is not among the nine accounts.** They are: the relayer, the pool, the two
vaults, the relayer's token account, the recipient, and three programs. The person who owned
the notes appears nowhere, and there is exactly one signer — the relayer, who also paid the
network fee.

## 2. It is not a one-shot demo

```
python3 verify.py 4jn34FnLDXjrDa4z5k5ABENVspuwDHLuA6koUFRBJP2t1gvDrACUfCuM1WThf3eh3C6tSRCdhQWJnDCwHAvtAweN
```

This one spends a note **created by the first transaction**. Both the commitment tree and the
nullifier tree were rebuilt from the chain alone, using only the public inputs of the
previous transaction. Before nullifiers were published, a second spend was impossible — a
flaw that stayed invisible until a client was written, because every demo restarted from a
fresh pool.

## 3. The deployed binary has not drifted

```
python3 -c "
import json, urllib.request, base64, hashlib
b = json.dumps({'jsonrpc':'2.0','id':1,'method':'getAccountInfo','params':['EbCn7j8eg72Ru8xjxMoDVQdY1LWtu6BVamDRQCQLYAET',{'encoding':'base64'}]})
r = urllib.request.Request('https://api.devnet.solana.com', b.encode(), {'Content-Type':'application/json'})
d = base64.b64decode(json.load(urllib.request.urlopen(r))['result']['value']['data'][0])
print(hashlib.sha256(d[45:45+67944]).hexdigest())"
```

Expected: `b8b569177b1f57d6cef9e2d8055aea8e005e3cf3be596429c576ed549635b76b`

## 4. The ceremony starting point is deterministic

```
for i in 00 01 02 03 04 05 06 07; do
  curl -s "https://bamiutggeezigmuauyhi.supabase.co/storage/v1/object/public/zk/v8/zkey.$i" -o p.$i
done
cat p.0* > start.zkey && sha256sum start.zkey
```

Expected: `bdf75a2e1a8cf307b8d8753d775ba97ea7328d043912f6c5a8a5371a85b8a788`

That file carries **zero contributions**. Recompile the circuit (step 6) and run
`snarkjs groth16 setup` against the public Hermez `powersOfTau28_hez_final_16.ptau`; you must
obtain exactly that file. `ceremony/verify.sh` then tells you where any key sits relative to
the deployed one — it distinguishes three cases, and only one of them is bad.

## 5. That binary is the published source (needs the Solana toolchain)

```
git clone https://github.com/ZNGLABS/confidential-swap-kit
cd confidential-swap-kit/solana-program
cargo-build-sbf --arch v3
sha256sum target/deploy/nexa_verifier.so
```

Same hash as step 3. Recompiling the six published files reproduces, byte for byte, the
program running on devnet — and therefore the verification key it applies and the NX mint it
enforces.

⚠️ `--arch v3` matters: the default architecture is rejected on chain by SIMD-0500.

## 6. The circuit is the one the key came from

```
npm install --no-save circom2 snarkjs@0.7.6 circomlib
mkdir lib && cp node_modules/circomlib/circuits/*.circom circuit/imt.circom lib/
node node_modules/circom2/cli.js circuit/pour.circom --r1cs --O2 -l lib
npx snarkjs r1cs info pour.r1cs
```

Expected: **55 496 constraints, 12 public inputs, 55 624 wires**.

⚠️ `--O2` matters. At `--O1` — the default of the `circom2` package — the same circuit yields
116 703 constraints, and `snarkjs groth16 setup` then refuses it against a 2^16 ptau. The
error text blames the ptau; the cause is the compiler flag.

## 7. The negative controls — what must FAIL

A system is worth what it refuses. These matter more than the successes.

| try | expected refusal |
|---|---|
| resubmit the first proof | `racine des nullifieurs perimee` — the nullifier root moved |
| a fee outside the tiers | `montant de frais 10 hors paliers autorises` |
| announce a token other than the one proved | `mintActif_ne_correspond_pas_a_la_preuve` |
| ask the relayer for less than its minimum | `frais_insuffisants` (HTTP 402) |

The out-of-tier proof we tested was **mathematically valid** — `snarkjs groth16 verify`
returns OK on it. What stops it is the rule, not a defect in the proof.

The relayer publishes its own tariff, so you can see what it accepts before building
anything: `GET` on `/functions/v1/relayeur-v8`. See `RELAYER.md`.

## What we are NOT claiming

- The trusted setup is **not finished**. The deployed program runs a single-machine test key.
  We need 3 to 5 independent contributors — see issue #1.
- There has been **no external audit**.
- Against a public AMM the **amount** of a swap remains deducible. We are selling
  fungibility, not perfect anonymity.
- Deposits are public by design, and deposit amounts are restricted to four denominations
  precisely because free amounts link withdrawals by simple equality.
