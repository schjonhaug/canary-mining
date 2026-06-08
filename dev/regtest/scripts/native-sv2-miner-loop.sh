#!/usr/bin/env bash
set -euo pipefail

CONFIG="${CONFIG:-examples/regtest.toml}"
BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
SV2_TIMEOUT_SECONDS="${SV2_TIMEOUT_SECONDS:-30}"
SV2_CORES="${SV2_CORES:-1}"
STOP_AFTER_ACCEPTED_BLOCKS="${STOP_AFTER_ACCEPTED_BLOCKS:-1}"
LINGER_AFTER_ACCEPTED_SECONDS="${LINGER_AFTER_ACCEPTED_SECONDS:-0}"
SV2_LOOP_DELAY_SECONDS="${SV2_LOOP_DELAY_SECONDS:-60}"
SV2_RETRY_DELAY_SECONDS="${SV2_RETRY_DELAY_SECONDS:-5}"

bitcoin_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" "$@"
}

extract_accepted_blocks() {
  sed -n 's/^native_sv2_test_miner_accepted_blocks=\([0-9][0-9]*\)$/\1/p' "$1" | tail -n 1
}

run_once() {
  local before after accepted_blocks miner_log miner_status total_wins

  before="$(bitcoin_cli getblockcount)"
  miner_log="$(mktemp "${TMPDIR:-/tmp}/canary-mining-loop.XXXXXX")"

  set +e
  cargo run --bin canary-mining-test-miner -- \
    --config "${CONFIG}" \
    --timeout-seconds "${SV2_TIMEOUT_SECONDS}" \
    --stop-after-accepted-blocks "${STOP_AFTER_ACCEPTED_BLOCKS}" \
    --linger-after-accepted-seconds "${LINGER_AFTER_ACCEPTED_SECONDS}" \
    --cores "${SV2_CORES}" | tee "${miner_log}"
  miner_status="${PIPESTATUS[0]}"
  set -e

  after="$(bitcoin_cli getblockcount)"
  accepted_blocks="$(extract_accepted_blocks "${miner_log}")"
  accepted_blocks="${accepted_blocks:-0}"
  rm -f "${miner_log}"

  total_wins="$(<"${wins_file}")"
  total_wins=$((total_wins + accepted_blocks))
  printf '%s\n' "${total_wins}" >"${wins_file}"

  printf 'ts=%s before=%s after=%s height_delta=%s sv2_accepted_blocks=%s sv2_total_wins=%s status=%s\n' \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
    "${before}" \
    "${after}" \
    "$((after - before))" \
    "${accepted_blocks}" \
    "${total_wins}" \
    "${miner_status}"

  return "${miner_status}"
}

if ! [[ "${STOP_AFTER_ACCEPTED_BLOCKS}" =~ ^[0-9]+$ ]] || [[ "${STOP_AFTER_ACCEPTED_BLOCKS}" -lt 1 ]]; then
  echo "STOP_AFTER_ACCEPTED_BLOCKS must be a positive integer" >&2
  exit 1
fi

wins_file="$(mktemp "${TMPDIR:-/tmp}/canary-mining-loop-wins.XXXXXX")"
printf '0\n' >"${wins_file}"

cleanup() {
  rm -f "${wins_file}"
}
trap cleanup EXIT

echo "sv2_miner_loop_config=${CONFIG}"
echo "sv2_timeout_seconds=${SV2_TIMEOUT_SECONDS}"
echo "sv2_cores=${SV2_CORES}"
echo "sv2_loop_delay_seconds=${SV2_LOOP_DELAY_SECONDS}"

while true; do
  if run_once; then
    sleep "${SV2_LOOP_DELAY_SECONDS}"
  else
    echo "SV2 miner loop iteration failed; retrying in ${SV2_RETRY_DELAY_SECONDS}s" >&2
    sleep "${SV2_RETRY_DELAY_SECONDS}"
  fi
done
