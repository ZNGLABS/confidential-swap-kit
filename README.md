# Confidential Swap Kit

Open-source reference implementation for adding **confidential swaps** (via [Arcium](https://docs.arcium.com/developers)) and **SPL-token fee payments** (via [Kora](https://launch.solana.com/docs/kora/getting-started)) to any existing Solana dApp.

## Why

Privacy is becoming Solana's next frontier: Arcium's MPC network is live on mainnet, C-SPL brings encrypted balances to any SPL token, and Kora (the Solana Foundation's fee relayer) lets users pay network fees in any SPL token. But integrating these primitives into an existing dApp still means piecing together scattered documentation. This kit turns that integration into a documented, reusable, audited path — days instead of months.

## What's inside (planned structure)

- `program/` — Anchor/Arcis confidential swap instruction, computed in an Arcium MXE
- `client/` — TypeScript integration: encrypt inputs client-side, build and send the confidential instruction
- `kora/` — fee-abstraction configuration: let users pay network fees in any SPL token
- `docs/` — step-by-step developer guide: add confidential swaps to your dApp

## Roadmap

| Milestone | Deliverable | Status |
|---|---|---|
| M1 | Devnet PoC: end-to-end confidential swap + SPL-fee payment demo (video + docs) | In preparation |
| M2 | Reusable kit + developer guide + production demonstration deployment | Planned |
| M3 | External security audit — full report published in this repo — + capped mainnet beta | Planned |

## Production demonstration

The kit is developed and demonstrated in production on [NEXA EXCHANGE](https://nexa-exchange.fr) — a Solana DEX live with Jupiter-routed swaps across 600+ tokens, vault-based on-chain limit orders, and a mobile app published on the Solana dApp Store.

## License

MIT — see [LICENSE](LICENSE).

Maintained by **ZNG Labs** · [@nexa_exchange](https://x.com/nexa_exchange) · contact@nexa-exchange.fr
