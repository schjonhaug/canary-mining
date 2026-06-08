# Agent Notes

## cmux workspace

This repo is usually worked from a `cmux` workspace named `canary mining`.
Use it to keep the local regtest stack visible while editing.

Current workspace shape:

- `surface:8`: Codex or shell in this repo.
- `surface:6`: Bitcoin Core regtest node.
- `surface:5`: `canary-mining` mining server.
- `surface:9`: local UI browser at `http://127.0.0.1:8080`.
- `surface:7`: native SV2 miner loop terminal.
- `surface:10`: external competing miners.
- optional regtest mempool traffic runs as `dev/regtest/scripts/mempool-traffic.sh`,
  not inside the mining server.

Useful cmux commands:

```bash
cmux current-workspace
cmux tree
cmux read-screen --surface surface:6 --scrollback --lines 80
cmux read-screen --surface surface:5 --scrollback --lines 120
cmux read-screen --surface surface:7 --scrollback --lines 120
cmux read-screen --surface surface:10 --scrollback --lines 120
```

Do not stop or restart these panes casually. They are stateful and are useful
for checking the current regtest chain, pool logs, miner-loop wins, and
competitor history.

When making UI or server changes in this repo, restart the mining server before
handing back so the updated UI is immediately available for review.

## Local regtest stack

The active harness is documented in `dev/regtest/README.md`. The short version:

To create the full cmux layout from scratch:

```bash
dev/regtest/setup.sh
```

1. Bitcoin Core regtest runs with IPC and RPC enabled:

   ```bash
   bitcoin -m node -regtest -datadir=/tmp/bitcoin-regtest -server -ipcbind=unix
   ```

   Expected IPC socket:

   ```bash
   /tmp/bitcoin-regtest/regtest/node.sock
   ```

2. The mining server runs from the repo root with:

   ```bash
   cargo run -- run --config examples/regtest.toml
   ```

   Important endpoints from `examples/regtest.toml`:

   - SV2 listener: `0.0.0.0:3333`
   - UI: `http://127.0.0.1:8080`
   - SRI monitoring: `http://127.0.0.1:9090`
   - Monitoring cache refresh: `1s`

   Readiness log to look for:

   ```text
   Required template data received, ready to accept connections
   ```

3. Optional mempool traffic can run outside the app:

   ```bash
   dev/regtest/scripts/mempool-traffic.sh
   ```

   This repeatedly sends small self-pay transactions from the regtest `miner`
   wallet so the dashboard template transaction, fee, and weight cards have
   non-zero data between blocks.

4. The native SV2 miner loop runs in the `Our Miner Loop` pane:

   ```bash
   dev/regtest/scripts/native-sv2-miner-loop.sh
   ```

   This loop intentionally runs until stopped and repeatedly launches the
   bounded native SV2 test miner so the UI and logs show this server winning
   blocks while competitors run.

For a one-shot pass/fail check, run the bounded native SV2 smoke test manually:

   ```bash
   dev/regtest/scripts/native-sv2-smoke-test.sh
   ```

   Success means `after` is greater than `before`, and the miner prints:

   ```text
   native_sv2_test_miner_accepted_blocks=1
   ```

The mining server may log `SocketClosed` after the bounded test miner exits.
That is expected for this smoke test.

## Verification

For normal code/UI changes, run:

```bash
cargo test
```

For end-to-end SV2 behavior, use the cmux stack and either watch the `Our Miner
Loop` pane or rerun the native smoke test after the mining server is ready.
