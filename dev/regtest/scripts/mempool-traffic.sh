#!/usr/bin/env bash
set -euo pipefail

BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
TRAFFIC_WALLET="${TRAFFIC_WALLET:-miner}"
TRAFFIC_INTERVAL_SECONDS="${TRAFFIC_INTERVAL_SECONDS:-1}"
TRAFFIC_MIN_TXS_PER_INTERVAL="${TRAFFIC_MIN_TXS_PER_INTERVAL:-1}"
TRAFFIC_MAX_TXS_PER_INTERVAL="${TRAFFIC_MAX_TXS_PER_INTERVAL:-5}"
TRAFFIC_AMOUNT_BTC="${TRAFFIC_AMOUNT_BTC:-0.00010000}"
TRAFFIC_FEE_RATE_SATS_VB="${TRAFFIC_FEE_RATE_SATS_VB:-1}"
TRAFFIC_MAX_FAILURES="${TRAFFIC_MAX_FAILURES:-5}"

bitcoin_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" "$@"
}

bitcoin_wallet_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" -rpcwallet="${TRAFFIC_WALLET}" "$@"
}

ensure_wallet() {
  bitcoin_cli createwallet "${TRAFFIC_WALLET}" >/dev/null 2>&1 ||
    bitcoin_cli loadwallet "${TRAFFIC_WALLET}" >/dev/null 2>&1 ||
    true

  bitcoin_wallet_cli getwalletinfo >/dev/null
}

send_batch() {
  local batch_count mempool_count target_count tx_span

  tx_span=$((TRAFFIC_MAX_TXS_PER_INTERVAL - TRAFFIC_MIN_TXS_PER_INTERVAL + 1))
  target_count=$((TRAFFIC_MIN_TXS_PER_INTERVAL + RANDOM % tx_span))
  batch_count=0
  while [[ "${batch_count}" -lt "${target_count}" ]]; do
    bitcoin_wallet_cli -named sendtoaddress \
      address="$(bitcoin_wallet_cli getnewaddress)" \
      amount="${TRAFFIC_AMOUNT_BTC}" \
      fee_rate="${TRAFFIC_FEE_RATE_SATS_VB}" \
      verbose=false \
    >/dev/null
    batch_count=$((batch_count + 1))
  done

  mempool_count="$(bitcoin_cli getmempoolinfo | sed -n 's/.*"size": \([0-9][0-9]*\).*/\1/p')"

  printf 'ts=%s wallet=%s target_txs=%s sent_txs=%s amount_btc=%s fee_rate_sats_vb=%s mempool_txs=%s\n' \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
    "${TRAFFIC_WALLET}" \
    "${target_count}" \
    "${batch_count}" \
    "${TRAFFIC_AMOUNT_BTC}" \
    "${TRAFFIC_FEE_RATE_SATS_VB}" \
    "${mempool_count:-unknown}"
}

if ! [[ "${TRAFFIC_MIN_TXS_PER_INTERVAL}" =~ ^[0-9]+$ ]]; then
  echo "TRAFFIC_MIN_TXS_PER_INTERVAL must be zero or a positive integer" >&2
  exit 1
fi

if ! [[ "${TRAFFIC_MAX_TXS_PER_INTERVAL}" =~ ^[0-9]+$ ]] || [[ "${TRAFFIC_MAX_TXS_PER_INTERVAL}" -lt "${TRAFFIC_MIN_TXS_PER_INTERVAL}" ]]; then
  echo "TRAFFIC_MAX_TXS_PER_INTERVAL must be greater than or equal to TRAFFIC_MIN_TXS_PER_INTERVAL" >&2
  exit 1
fi

if ! [[ "${TRAFFIC_MAX_FAILURES}" =~ ^[0-9]+$ ]] || [[ "${TRAFFIC_MAX_FAILURES}" -lt 1 ]]; then
  echo "TRAFFIC_MAX_FAILURES must be a positive integer" >&2
  exit 1
fi

ensure_wallet

echo "traffic_wallet=${TRAFFIC_WALLET}"
echo "traffic_interval_seconds=${TRAFFIC_INTERVAL_SECONDS}"
echo "traffic_min_txs_per_interval=${TRAFFIC_MIN_TXS_PER_INTERVAL}"
echo "traffic_max_txs_per_interval=${TRAFFIC_MAX_TXS_PER_INTERVAL}"
echo "traffic_amount_btc=${TRAFFIC_AMOUNT_BTC}"
echo "traffic_fee_rate_sats_vb=${TRAFFIC_FEE_RATE_SATS_VB}"

failures=0
while true; do
  if send_batch; then
    failures=0
  else
    failures=$((failures + 1))
    echo "mempool traffic transaction failed (${failures}/${TRAFFIC_MAX_FAILURES})" >&2
    if [[ "${failures}" -ge "${TRAFFIC_MAX_FAILURES}" ]]; then
      echo "mempool traffic stopped after too many failures" >&2
      exit 1
    fi
  fi

  sleep "${TRAFFIC_INTERVAL_SECONDS}"
done
