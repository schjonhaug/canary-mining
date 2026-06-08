#!/usr/bin/env bash
set -euo pipefail

BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
COMPETING_MINERS="${COMPETING_MINERS:-1}"
COMPETING_MINER_INTERVAL_SECONDS="${COMPETING_MINER_INTERVAL_SECONDS:-60}"
COMPETING_MINER_STATE_DIR="${COMPETING_MINER_STATE_DIR:-}"

if [[ -z "${COMPETING_MINER_STATE_DIR}" ]]; then
  COMPETING_MINER_STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/canary-competing-miners.XXXXXX")"
fi

mkdir -p "${COMPETING_MINER_STATE_DIR}"

bitcoin_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" "$@"
}

bitcoin_wallet_cli() {
  local wallet="$1"
  shift
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" -rpcwallet="${wallet}" "$@"
}

ensure_wallet() {
  local wallet="$1"

  bitcoin_cli createwallet "${wallet}" >/dev/null 2>&1 ||
    bitcoin_cli loadwallet "${wallet}" >/dev/null 2>&1 ||
    true

  bitcoin_wallet_cli "${wallet}" getwalletinfo >/dev/null
}

first_block_hash() {
  sed -n 's/.*"\([0-9a-fA-F]\{64\}\)".*/\1/p' | head -n 1
}

miner_loop() {
  local miner_id="$1"
  local wallet="canary_competitor_${miner_id}"
  local count_file="${COMPETING_MINER_STATE_DIR}/miner-${miner_id}.count"
  local log_file="${COMPETING_MINER_STATE_DIR}/miner-${miner_id}.log"
  local address before after block_hash

  ensure_wallet "${wallet}"
  address="$(bitcoin_wallet_cli "${wallet}" getnewaddress)"
  printf '0\n' >"${count_file}"

  echo "miner=${miner_id} wallet=${wallet} address=${address}" | tee -a "${log_file}"

  while true; do
    before="$(bitcoin_cli getblockcount)"
    block_hash="$(bitcoin_wallet_cli "${wallet}" generatetoaddress 1 "${address}" | first_block_hash)"
    after="$(bitcoin_cli getblockcount)"

    awk 'NF { print $1 + 1; next } { print 1 }' "${count_file}" >"${count_file}.tmp"
    mv "${count_file}.tmp" "${count_file}"

    printf 'ts=%s miner=%s before=%s after=%s block=%s\n' \
      "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
      "${miner_id}" \
      "${before}" \
      "${after}" \
      "${block_hash:-unknown}" | tee -a "${log_file}"

    sleep "${COMPETING_MINER_INTERVAL_SECONDS}"
  done
}

if ! [[ "${COMPETING_MINERS}" =~ ^[0-9]+$ ]] || [[ "${COMPETING_MINERS}" -lt 1 ]]; then
  echo "COMPETING_MINERS must be a positive integer" >&2
  exit 1
fi

echo "competing_miner_state_dir=${COMPETING_MINER_STATE_DIR}"
echo "competing_miners=${COMPETING_MINERS}"
echo "competing_miner_interval_seconds=${COMPETING_MINER_INTERVAL_SECONDS}"

pids=()
cleanup() {
  local pid
  for pid in "${pids[@]:-}"; do
    kill "${pid}" >/dev/null 2>&1 || true
  done
  wait "${pids[@]:-}" >/dev/null 2>&1 || true
}

abort() {
  cleanup
  exit 130
}

trap cleanup EXIT
trap abort INT TERM

for miner_id in $(seq 1 "${COMPETING_MINERS}"); do
  miner_loop "${miner_id}" &
  pids+=("$!")
done

wait "${pids[@]}"
