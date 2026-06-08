#!/usr/bin/env bash
set -euo pipefail

CONFIG="${CONFIG:-examples/regtest.toml}"
BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-60}"
CORES="${CORES:-1}"
STOP_AFTER_ACCEPTED_BLOCKS="${STOP_AFTER_ACCEPTED_BLOCKS:-1}"
LINGER_AFTER_ACCEPTED_SECONDS="${LINGER_AFTER_ACCEPTED_SECONDS:-10}"

BEFORE="$(bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" getblockcount)"

cargo run --bin canary-mining-test-miner -- \
  --config "${CONFIG}" \
  --timeout-seconds "${TIMEOUT_SECONDS}" \
  --stop-after-accepted-blocks "${STOP_AFTER_ACCEPTED_BLOCKS}" \
  --linger-after-accepted-seconds "${LINGER_AFTER_ACCEPTED_SECONDS}" \
  --cores "${CORES}"

AFTER="$(bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" getblockcount)"

echo "before=${BEFORE} after=${AFTER}"

if [[ "${AFTER}" -le "${BEFORE}" ]]; then
  echo "native SV2 smoke test failed: block height did not increase" >&2
  exit 1
fi
