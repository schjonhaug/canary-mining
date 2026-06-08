# Native SV2 Regtest Smoke Test

This harness keeps Bitcoin Core and `canary-mining` local, then runs the
included native SV2 test miner:

```text
canary-mining-test-miner --SV2--> canary-mining --IPC--> Bitcoin Core regtest
```

## Requirements

- Bitcoin Core 30+ with IPC enabled
- Rust toolchain
- `capnp` installed locally for building the app

## 1. Start Bitcoin Core Regtest

Use a local datadir that matches `examples/regtest.toml`:

```bash
mkdir -p /tmp/bitcoin-regtest
bitcoin -m node -regtest -datadir=/tmp/bitcoin-regtest -server -ipcbind=unix
```

The expected socket is:

```text
/tmp/bitcoin-regtest/regtest/node.sock
```

Check it:

```bash
ls -l /tmp/bitcoin-regtest/regtest/node.sock
```

## 2. Mine Initial Blocks

On a fresh regtest chain, mine blocks before starting `canary-mining`.

```bash
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest createwallet miner
ADDR=$(bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest -rpcwallet=miner getnewaddress)
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest generatetoaddress 101 "$ADDR"
bitcoin-cli -regtest -datadir=/tmp/bitcoin-regtest getblockcount
```

If the wallet already exists, use `loadwallet miner` instead of `createwallet
miner`.

## 3. Start `canary-mining`

From the repo root:

```bash
cargo run -- run --config examples/regtest.toml
```

Confirm it logs:

```text
SV2 listen address: 0.0.0.0:3333
Bitcoin Core IPC socket: /tmp/bitcoin-regtest/regtest/node.sock
Required template data received, ready to accept connections
```

## 4. Run Native SV2 Miner Smoke Test

In another terminal:

```bash
dev/regtest/scripts/native-sv2-smoke-test.sh
```

The smoke test succeeds when `after` is greater than `before`.
The bundled test miner defaults to
`<default regtest address>.regtest_sv2_test_miner`, so the dashboard can show
both the worker label and the payout address.

You can run the miner directly:

```bash
cargo run --bin canary-mining-test-miner -- \
  --config examples/regtest.toml \
  --timeout-seconds 60 \
  --stop-after-accepted-blocks 1 \
  --cores 1
```

Expected pool-side flow:

- native miner opens an SV2 standard mining channel.
- pool receives one `SubmitSharesStandard`.
- pool logs `Block Found`.
- Bitcoin Core receives `SubmitSolution`.
- Bitcoin Core chain tip changes.

The pool can log `SocketClosed` when the test miner exits after the accepted
block candidate. That is expected for this bounded smoke test.

## cmux Setup

To create a full interactive `cmux` workspace with Bitcoin Core, the mining
server, and the local UI browser:

```bash
dev/regtest/setup.sh
```

If a `canary mining` workspace already exists, the script exits instead of
stopping or duplicating existing stateful panes. Use the existing workspace or
close it first. For an independent second stack, use a separate `WORKSPACE_NAME`
plus a matching `BITCOIN_DATADIR` and config file.

## 5. Run Native SV2 With Competing Miners

The smoke test above is intentionally short and deterministic. To exercise tip
changes from outside `canary-mining`, run the competition scenario while
Bitcoin Core and the mining server are already running:

```bash
dev/regtest/scripts/native-sv2-with-competition.sh
```

This starts background regtest miners that mine directly through Bitcoin Core
RPC with `generatetoaddress`, then runs the native SV2 test miner through
`canary-mining` at the same time:

```text
external regtest miners --RPC--> Bitcoin Core regtest
canary-mining-test-miner --SV2--> canary-mining --IPC--> Bitcoin Core regtest
```

Expected output includes a summary:

```text
before=...
after=...
sv2_accepted_blocks=...
competitor_blocks=...
total_height_delta=...
competing_miner_state_dir=...
```

Useful overrides:

```bash
COMPETING_MINERS=4 \
COMPETING_MINER_INTERVAL_SECONDS=2 \
SV2_TIMEOUT_SECONDS=120 \
SV2_CORES=2 \
dev/regtest/scripts/native-sv2-with-competition.sh
```

Use `native-sv2-smoke-test.sh` for a fast pass/fail check. Use
`native-sv2-with-competition.sh` when you want to confirm that the server and
native miner continue to receive usable work while unrelated miners advance the
regtest chain tip.

## 6. Watch Continuous Competition

For an interactive `cmux` workflow, keep Bitcoin Core and `canary-mining`
running in their own panes, then use separate panes for any extra long-running
actors.

Run external competitors in one pane:

```bash
COMPETING_MINERS=2 \
COMPETING_MINER_INTERVAL_SECONDS=60 \
dev/regtest/scripts/competing-miners.sh
```

By default this starts one competitor loop. It mines one block, then sleeps for
60 seconds before trying again:

```text
COMPETING_MINERS=1
COMPETING_MINER_INTERVAL_SECONDS=60
```

These competitors use Bitcoin Core regtest `generatetoaddress`, so they are not
actually hashing. Treat `COMPETING_MINER_INTERVAL_SECONDS` as the main knob for
how often the outside network advances the tip.

Run optional mempool traffic in another pane when you want non-empty templates:

```bash
dev/regtest/scripts/mempool-traffic.sh
```

This traffic is deliberately outside `canary-mining`. It uses Bitcoin Core RPC
wallet calls, so dashboard template weight, transaction count, and block value
can vary without adding regtest-only behavior to the app runtime.

Run the native SV2 miner loop in another pane:

```bash
SV2_LOOP_DELAY_SECONDS=60 \
SV2_TIMEOUT_SECONDS=30 \
dev/regtest/scripts/native-sv2-miner-loop.sh
```

The competitor pane prints each directly mined regtest block. The optional
mempool traffic script logs each randomized transaction batch, and the SV2 loop
pane prints each block candidate accepted through `canary-mining` plus a
running `sv2_total_wins` counter.

## 7. Run cgminer Through SRI Translator Proxy

`canary-mining` remains native SV2-only. To test an SV1 miner locally, run
cgminer through SRI translator proxy as an external harness:

```bash
dev/regtest/scripts/cgminer-through-tproxy.sh
```

The script generates `dev/regtest/generated/tproxy.toml` from
`examples/regtest.toml` and `data/regtest/authority-keys.toml`, starts
translator proxy on `127.0.0.1:34255`, then runs cgminer against:

```text
stratum+tcp://127.0.0.1:34255
```

Required external commands:

- `bitcoin-cli`
- `translator-proxy`, or set `TPROXY_CMD=/path/to/translator-proxy`
- `cgminer`, or set `CGMINER_CMD=/path/to/cgminer`

Useful overrides:

```bash
CONFIG=examples/regtest.toml \
BITCOIN_DATADIR=/tmp/bitcoin-regtest \
SV2_HOST=127.0.0.1 \
SV2_PORT=3333 \
TPROXY_PORT=34255 \
CGMINER_USER=regtest_cpu \
CGMINER_PASSWORD=x \
dev/regtest/scripts/cgminer-through-tproxy.sh
```

Use this for local SV1-through-SV2 experiments only. Start9/mainnet sideload
testing with native SV2 miners is the primary integration path.

This is intentionally separate from `native-sv2-smoke-test.sh`. The smoke test
should remain bounded; the loop scripts are for watching live competition.
