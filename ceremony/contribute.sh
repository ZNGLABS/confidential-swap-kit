#!/usr/bin/env bash
# contribute.sh — your contribution to the NEXA phase-2 trusted setup.
#
# What you are doing: you add your own randomness to the proving key, then you
# DESTROY it. It is enough that ONE participant genuinely destroyed theirs for
# nobody to ever forge a proof. You do not have to trust the other participants
# — your own honesty alone protects everyone.
#
# Requirements: node >= 18, ~1 GB free disk, about 3 minutes.
# Call for participants: https://github.com/ZNGLABS/confidential-swap-kit/issues/1
set -euo pipefail

YOUR_NAME="${1:?usage: ./contribute.sh \"your name or handle\" previous.zkey}"
PREVIOUS="${2:?give the path to the .zkey you received}"

echo "-- 1/4  tooling"
npm install --no-save snarkjs@0.7.6 >/dev/null 2>&1
SNARK="npx --no-install snarkjs"

echo "-- 2/4  what you received"
echo "   sha256 of the file you received:"
sha256sum "$PREVIOUS" | cut -c1-64

echo "-- 3/4  your contribution (about 20 s)"
echo "   Move the mouse and type randomly when asked."
$SNARK zkey contribute "$PREVIOUS" "contribution-$YOUR_NAME.zkey" \
  --name="$YOUR_NAME" -v

echo "-- 4/4  what to publish"
echo "   file  : contribution-$YOUR_NAME.zkey"
echo "   sha256:"
sha256sum "contribution-$YOUR_NAME.zkey" | cut -c1-64
cat <<'END'

   Publish that hash somewhere public and timestamped (a comment on issue #1, a
   commit, a post) BEFORE you send the file. That is what proves you did not
   change your contribution afterwards.

   >> NOW DESTROY YOUR RANDOMNESS:
      - close this terminal (the entropy never touched the disk);
      - keep no note of what you typed.
   If you keep it AND every other participant keeps theirs, someone could forge
   proofs. Your forgetting is the guarantee.
END
