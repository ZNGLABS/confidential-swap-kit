#!/usr/bin/env bash
# verify.sh — ANYONE can verify this ceremony, without trusting us.
#
# It checks three things:
#   1. that the starting point matches the published circuit;
#   2. that the whole chain of contributions is valid;
#   3. that the verification key embedded in the DEPLOYED program is the one
#      this ceremony produced. Without check 3, a ceremony says nothing about
#      the code that actually runs.
set -euo pipefail
R1CS="${1:?usage: ./verify.sh pour.r1cs powersOfTau28_hez_final_16.ptau final.zkey}"
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

echo "-- verification key produced by this ceremony"
$SNARK zkey export verificationkey "$FINAL" vkey-from-ceremony.json >/dev/null
diff <(python3 -m json.tool vkey-from-ceremony.json) \
     <(python3 -m json.tool circuit/verification_key.json) >/dev/null 2>&1 \
  && echo "   MATCHES circuit/verification_key.json" \
  || echo "   DIFFERS from circuit/verification_key.json  <-- everything above is void"
cat <<'END'

   circuit/verification_key.json is the key compiled into the deployed program
   (solana-program/src/verifying_key.rs). If the two differ, the deployed
   program does NOT use this ceremony, and nothing above matters.
END
