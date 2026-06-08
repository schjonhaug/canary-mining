# Changelog

## 1.0.0 - 2026-06-08

First public release of Canary Mining.

- Native Stratum V2 solo mining server backed by Bitcoin Core IPC.
- StartOS package support with Bitcoin Core dependency integration.
- Local operator dashboard for template, chain, miner, and recent-block status.
- Miner payout validation through SV2 `user_identity`.
- Regtest harness and native SV2 smoke-test miner.
- Bounded dashboard/status behavior for slow optional Bitcoin Core RPC calls.
- Bounded recent-template/runtime state to avoid unbounded memory growth.

