#!/usr/bin/env bash
# verify.sh — ANYONE can verify this ceremony, without trusting us.
#
# It checks three things:
#   1. that the starting point matches the published circuit;
#   2. that the whole chain of contributions is valid;
#   3. how the resulting key relates to the one the DEPLOYED program uses.
#
# On (3) there are three possible outcomes, and only one of them is bad. While
# a ceremony is still running, the key you hold is EXPECTED to differ from the
# deployed one — that is what an unfinished ceremony means. The script now says
# which case you are in instead of declaring everything void.
set -euo pipefail
R1CS="${1:?usage: ./verify.sh pour.r1cs powersOfTau28_hez_final_16.ptau some.zkey}"
PTAU="${2:?}" ; FINAL="${3:?}"

npm install --no-save snarkjs@0.7.6 >/dev/null 2>&1
SNARK="npx --no-install snarkjs"

echo "-- hashes of the inputs"
sha256sum "$R1CS" "$PTAU" "$FINAL" | cut -c1-72

echo "-- phase 1: is the ptau the public Hermez one?"
echo "   compare the hash above with the one published by the Hermez project"
echo "   for powersOfTau28_hez_final_16.ptau."

echo "-- chain of contributions (about 90 s)"
$SNARK zkey verify "$R1CS" "$PTAU" "$FINAL"

echo "-- verification key produced by this file"
$SNARK zkey export verificationkey "$FINAL" vkey-from-ceremony.json >/dev/null

set +e
python3 - vkey-from-ceremony.json circuit/verification_key.json <<'PY'
import json, sys
a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))
if a == b:
    sys.exit(0)
diff = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
if diff == ["vk_delta_2"]:
    sys.exit(1)
print("   fields that differ: " + ", ".join(diff))
sys.exit(2)
PY
CAS=$?
set -e

case $CAS in
  0)
    cat <<'END'
   IDENTICAL to circuit/verification_key.json.

   This file IS the key compiled into the deployed program
   (solana-program/src/verifying_key.rs). The chain of contributions above is
   therefore the chain that protects the code actually running on chain.
END
    ;;
  1)
    cat <<'END'
   SAME CIRCUIT, DIFFERENT POINT IN THE CEREMONY.

   Only vk_delta_2 differs. That single field is the one that accumulates
   phase-2 contributions; everything else comes from the circuit and from
   phase 1, and it matches exactly. So this key and the deployed one prove
   statements about the SAME circuit — they simply sit at different points
   of the ceremony.

   This is the expected result while the ceremony is still open: the deployed
   program currently runs a single-machine test key, and it will be replaced
   by the key this ceremony produces once enough people have contributed.
   Until then, what you can check is the starting point (above), not the
   deployed binary.
END
    ;;
  *)
    cat <<'END'
   DIFFERENT CIRCUIT  <-- stop here.

   Fields other than vk_delta_2 differ, which means this key does not even
   describe the same circuit as the published one. Nothing above matters.
   Please report it on issue #1.
END
    exit 1
    ;;
esac
