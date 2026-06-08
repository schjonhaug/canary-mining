#!/usr/bin/env bash
set -euo pipefail

CONFIG="${CONFIG:-examples/regtest.toml}"
BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
SV2_TIMEOUT_SECONDS="${SV2_TIMEOUT_SECONDS:-60}"
SV2_CORES="${SV2_CORES:-1}"
STOP_AFTER_ACCEPTED_BLOCKS="${STOP_AFTER_ACCEPTED_BLOCKS:-1}"
LINGER_AFTER_ACCEPTED_SECONDS="${LINGER_AFTER_ACCEPTED_SECONDS:-0}"
COMPETING_MINERS="${COMPETING_MINERS:-1}"
COMPETING_MINER_INTERVAL_SECONDS="${COMPETING_MINER_INTERVAL_SECONDS:-5}"
COMPETING_MINER_STATE_DIR="${COMPETING_MINER_STATE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/canary-competing-miners.XXXXXX")}"

bitcoin_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" "$@"
}

sum_competitor_blocks() {
  local count_file total=0
  shopt -s nullglob
  for count_file in "${COMPETING_MINER_STATE_DIR}"/miner-*.count; do
    total=$((total + $(<"${count_file}")))
  done
  shopt -u nullglob
  echo "${total}"
}

extract_accepted_blocks() {
  sed -n 's/^native_sv2_test_miner_accepted_blocks=\([0-9][0-9]*\)$/\1/p' "$1" | tail -n 1
}

mkdir -p "${COMPETING_MINER_STATE_DIR}"

BEFORE="$(bitcoin_cli getblockcount)"
MINER_LOG="$(mktemp "${TMPDIR:-/tmp}/canary-mining-competition.XXXXXX")"
COMPETING_PID=""

cleanup() {
  if [[ -n "${COMPETING_PID}" ]]; then
    kill "${COMPETING_PID}" >/dev/null 2>&1 || true
    wait "${COMPETING_PID}" >/dev/null 2>&1 || true
  fi
}

abort() {
  cleanup
  exit 130
}

trap cleanup EXIT
trap abort INT TERM

COMPETING_MINERS="${COMPETING_MINERS}" \
COMPETING_MINER_INTERVAL_SECONDS="${COMPETING_MINER_INTERVAL_SECONDS}" \
COMPETING_MINER_STATE_DIR="${COMPETING_MINER_STATE_DIR}" \
BITCOIN_DATADIR="${BITCOIN_DATADIR}" \
  dev/regtest/scripts/competing-miners.sh &
COMPETING_PID="$!"

sleep 1

set +e
cargo run --bin canary-mining-test-miner -- \
  --config "${CONFIG}" \
  --timeout-seconds "${SV2_TIMEOUT_SECONDS}" \
  --stop-after-accepted-blocks "${STOP_AFTER_ACCEPTED_BLOCKS}" \
  --linger-after-accepted-seconds "${LINGER_AFTER_ACCEPTED_SECONDS}" \
  --cores "${SV2_CORES}" | tee "${MINER_LOG}"
MINER_STATUS="${PIPESTATUS[0]}"
set -e

cleanup
trap - EXIT INT TERM

AFTER="$(bitcoin_cli getblockcount)"
SV2_ACCEPTED_BLOCKS="$(extract_accepted_blocks "${MINER_LOG}")"
SV2_ACCEPTED_BLOCKS="${SV2_ACCEPTED_BLOCKS:-0}"
COMPETITOR_BLOCKS="$(sum_competitor_blocks)"
TOTAL_DELTA=$((AFTER - BEFORE))

echo "before=${BEFORE} after=${AFTER}"
echo "sv2_accepted_blocks=${SV2_ACCEPTED_BLOCKS}"
echo "competitor_blocks=${COMPETITOR_BLOCKS}"
echo "total_height_delta=${TOTAL_DELTA}"
echo "competing_miner_state_dir=${COMPETING_MINER_STATE_DIR}"

if [[ "${MINER_STATUS}" -ne 0 ]]; then
  echo "native SV2 miner failed while competing miners were running" >&2
  exit "${MINER_STATUS}"
fi

if [[ "${TOTAL_DELTA}" -le 0 ]]; then
  echo "competition scenario failed: block height did not increase" >&2
  exit 1
fi

if [[ "${COMPETITOR_BLOCKS}" -le 0 ]]; then
  echo "competition scenario failed: no competitor blocks were mined" >&2
  exit 1
fi

if [[ "${SV2_ACCEPTED_BLOCKS}" -lt "${STOP_AFTER_ACCEPTED_BLOCKS}" ]]; then
  echo "competition scenario failed: SV2 accepted block count was below target" >&2
  exit 1
fi
