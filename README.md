# Canary Mining

Self-hosted native Stratum V2 solo mining server for Bitcoin Core IPC.

Canary Mining lets native SV2 miners mine directly against your own Bitcoin
Core node:

```text
native Stratum V2 miner -> Canary Mining -> your Bitcoin Core
```

It accepts native Stratum V2 miner connections, receives block templates from
Bitcoin Core over IPC, submits solved blocks back through Bitcoin Core, and
serves a small local dashboard for operators.

Canary Mining is not a public pool, payout service, hosted mining platform, or
SV1 translator. There are no accounts, balances, pooled payouts, withdrawals,
or custody flows. If a miner finds a block, the reward goes directly to the
Bitcoin address supplied by that miner in its SV2 `user_identity`.

## Features

- Native Stratum V2 solo mining server.
- Bitcoin Core IPC template delivery and solved-block submission.
- Bitcoin Core RPC dashboard data for chain status, recent blocks, difficulty,
  and sync state.
- Miner identity validation against the active Bitcoin network.
- Local dashboard with template, miner, and recent-block visibility.
- Bounded template/history memory use.
- Regtest harness and native SV2 smoke-test miner for local verification.
- StartOS packaging support.

## Requirements

- Bitcoin Core 30.2+ with IPC enabled.
- A native Stratum V2 miner, or the included regtest-only
  `canary-mining-test-miner`.
- Rust 2024-compatible toolchain for source builds.

## Install On StartOS

The recommended production path is the StartOS package:

1. Install the Canary Mining StartOS package.
2. Enable IPC in Bitcoin Core:

   ```text
   Bitcoin Core -> Actions -> Other -> Enable IPC
   ```

3. Restart Bitcoin Core after enabling IPC. The setting is saved immediately,
   but the IPC socket is created only after Bitcoin Core restarts.
4. Open:

   ```text
   Bitcoin Core -> Actions -> Runtime Information
   ```

5. Copy the `IPC Socket Path` into the Canary Mining config if the package does
   not fill it automatically.
6. Open the Canary Mining dashboard and copy the SV2 endpoint and authority
   public key into your native SV2 miner configuration.

For package-to-package traffic on StartOS, Canary Mining expects Bitcoin Core
RPC and IPC to be reachable inside the service container. A typical internal
configuration looks like:

```toml
[bitcoin_core]
ipc_socket_path = "unix:/root/.bitcoin/ipc/bitcoin-core.sock"
rpc_url = "http://bitcoind.startos:8332"
rpc_cookie_path = "/root/.bitcoin/.cookie"
```

RPC health alone is not enough. Mining-critical template delivery and block
submission use IPC.

## Build From Source

Generate an example config:

```bash
cargo run -- example-config --network regtest
```

Check a config and create the SV2 authority key:

```bash
cargo run -- check --config examples/regtest.toml
```

Run the server:

```bash
cargo run -- run --config examples/regtest.toml
```

The `check` and `run` commands print the SV2 authority public key. Configure
your miner to use that key when connecting to the server.

Example configs are available for `mainnet`, `testnet4`, `signet`, and
`regtest` under `examples/`.

## Miner Identity

Each miner must provide the payout address in its SV2 `user_identity`. Accepted
formats are:

```text
<bitcoin-address>
<bitcoin-address>.<worker-label>
sri/solo/<bitcoin-address>/<worker-label>
```

The server validates the address for the active Bitcoin network before
accepting the miner. A bare worker name such as `garage-s19` is rejected because
it does not specify where the block reward should go.

## Dashboard And Template Timing

Mining templates are pushed from Bitcoin Core over IPC. The dashboard is
display-only and does not call Bitcoin Core's RPC `getblocktemplate`.

The browser polls Canary Mining's `/api/status` endpoint. Mining-critical work
uses IPC, while read-only dashboard data such as sync state, network hashrate,
difficulty, and recent blocks comes from Bitcoin Core RPC. If optional RPC data
is slow or unavailable, the server keeps mining state available and returns
bounded cached dashboard data where possible.

Important config knobs:

- `bitcoin_core.fee_threshold`: minimum mempool fee delta, in sats, that can
  trigger a new mempool-driven template.
- `bitcoin_core.min_interval`: minimum seconds between mempool-driven template
  updates sent to miners. Chain-tip changes still publish immediately.
- `[metrics].cache_refresh_secs`: SRI monitoring cache refresh interval for
  dashboard statistics.

## Regtest Smoke Test

The easiest end-to-end development test is:

```text
canary-mining-test-miner --SV2--> canary-mining --IPC--> Bitcoin Core regtest
```

Start Bitcoin Core regtest with IPC:

```bash
mkdir -p /tmp/bitcoin-regtest
bitcoin -m node -regtest -datadir=/tmp/bitcoin-regtest -server -ipcbind=unix
```

Mine initial regtest blocks before starting Canary Mining:

```bash
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest createwallet miner
ADDR=$(bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest -rpcwallet=miner getnewaddress)
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest generatetoaddress 101 "$ADDR"
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest getblockcount
```

Start Canary Mining:

```bash
cargo run -- run --config examples/regtest.toml
```

Open the local dashboard:

```text
http://127.0.0.1:8080
```

Run the native SV2 smoke test:

```bash
dev/regtest/scripts/native-sv2-smoke-test.sh
```

The smoke test succeeds when the Bitcoin Core block count increases and the
miner prints:

```text
native_sv2_test_miner_accepted_blocks=1
```

The full local harness is documented in `dev/regtest/README.md`.

## Project Boundary

The runtime product is the `canary-mining` binary:

```bash
canary-mining run --config <path>
```

That binary starts the SV2 solo mining server, optional local dashboard,
optional SRI monitoring proxy, authority-key storage, and local miner metadata.
The config file is the public operator interface for Bitcoin Core IPC/RPC,
listen addresses, metrics, UI enablement, data directory, and template refresh
thresholds.

Out of scope for 1.0:

- SV1 mining server support.
- Translator proxy operation as part of the packaged runtime.
- Public pool accounts, pooled mining, share accounting, balances,
  withdrawals, or custody flows.
- JDS/JDC wiring.
- Server-wide miner payout editing from the dashboard.

## SRI Pin

SRI crates are pinned to `stratum-mining/sv2-apps` release `v0.4.0`, commit
`de60df95245a4a6127cb5ece120d58433cbe823b`. `pool_sv2` is vendored under
`vendor/pool_sv2` with Canary patches for solo-mining identity validation and
dashboard/runtime integration.

## Security

Canary Mining handles block templates, miner connections, and Bitcoin Core
access. Run it only on infrastructure you control, keep Bitcoin Core RPC
private, and expose the SV2 listener only to miners you intend to use.

Report security issues privately using the guidance in `SECURITY.md`.

## License

MIT
