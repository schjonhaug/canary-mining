#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BITCOIN_DATADIR="${BITCOIN_DATADIR:-/tmp/bitcoin-regtest}"
CONFIG="${CONFIG:-examples/regtest.toml}"
WORKSPACE_NAME="${WORKSPACE_NAME:-canary mining}"

bitcoin_cli() {
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" "$@"
}

bitcoin_wallet_cli() {
  local wallet="$1"
  shift
  bitcoin-cli -regtest -datadir="${BITCOIN_DATADIR}" -rpcwallet="${wallet}" "$@"
}

workspace_ref_by_name() {
  cmux list-workspaces |
    sed -n "s/.*\\(workspace:[0-9][0-9]*\\).*${WORKSPACE_NAME}.*/\\1/p" |
    head -n 1
}

parse_ref() {
  local kind="$1"
  sed -n "s/.*\\(${kind}:[0-9][0-9]*\\).*/\\1/p" | head -n 1
}

send_command() {
  local surface="$1"
  local command="$2"

  cmux send --surface "${surface}" "${command}"
  cmux send-key --surface "${surface}" enter
}

rename_surface() {
  local surface="$1"
  local title="$2"

  cmux rename-tab --surface "${surface}" "${title}" >/dev/null 2>&1 || true
}

new_terminal_pane() {
  local workspace="$1"
  local direction="$2"
  local title="$3"
  local command="$4"
  local output surface

  output="$(cmux new-pane --type terminal --direction "${direction}" --workspace "${workspace}" --focus false)"
  surface="$(printf '%s\n' "${output}" | parse_ref surface)"
  rename_surface "${surface}" "${title}"
  send_command "${surface}" "${command}"
  printf '%s\n' "${surface}"
}

wait_for_rpc() {
  until bitcoin_cli getblockcount >/dev/null 2>&1; do
    echo "Waiting for Bitcoin Core RPC at ${BITCOIN_DATADIR}..."
    sleep 1
  done
}

ensure_initial_blocks() {
  local height missing address

  wait_for_rpc

  bitcoin_cli createwallet miner >/dev/null 2>&1 ||
    bitcoin_cli loadwallet miner >/dev/null 2>&1 ||
    true

  bitcoin_wallet_cli miner getwalletinfo >/dev/null

  height="$(bitcoin_cli getblockcount)"
  if [[ "${height}" -lt 101 ]]; then
    missing=$((101 - height))
    address="$(bitcoin_wallet_cli miner getnewaddress)"
    echo "Mining ${missing} initial regtest blocks to ${address}..."
    bitcoin_wallet_cli miner generatetoaddress "${missing}" "${address}" >/dev/null
  fi

  echo "Bitcoin Core regtest height: $(bitcoin_cli getblockcount)"
}

run_role() {
  local role="$1"

  cd "${REPO_ROOT}"

  case "${role}" in
    bitcoin)
      mkdir -p "${BITCOIN_DATADIR}"
      exec bitcoin -m node -regtest -datadir="${BITCOIN_DATADIR}" -server -listen=0 -ipcbind=unix
      ;;
    server)
      ensure_initial_blocks
      exec cargo run -- run --config "${CONFIG}"
      ;;
    *)
      echo "unknown setup role: ${role}" >&2
      exit 2
      ;;
  esac
}

setup_workspace() {
  local workspace_name="${WORKSPACE_NAME}"
  local workspace output bitcoin_surface server_surface browser_surface
  local setup_cmd="${REPO_ROOT}/dev/regtest/setup.sh"

  workspace="$(workspace_ref_by_name || true)"
  if [[ -n "${workspace}" ]]; then
    cat >&2 <<EOF
Workspace '${WORKSPACE_NAME}' already exists as ${workspace}.
This setup script starts Bitcoin Core with ${BITCOIN_DATADIR}, so it avoids
creating duplicate panes that would fight over the same regtest datadir.

Use the existing workspace, close it first, or choose a different WORKSPACE_NAME
and matching CONFIG/BITCOIN_DATADIR for an independent stack.
EOF
    exit 1
  fi

  output="$(cmux new-workspace --name "${workspace_name}" --cwd "${REPO_ROOT}" --focus true)"
  workspace="$(printf '%s\n' "${output}" | parse_ref workspace)"
  bitcoin_surface="$(printf '%s\n' "${output}" | parse_ref surface)"

  rename_surface "${bitcoin_surface}" "Bitcoin Core"
  send_command "${bitcoin_surface}" "cd ${REPO_ROOT} && BITCOIN_DATADIR=${BITCOIN_DATADIR} ${setup_cmd} --role bitcoin"

  server_surface="$(new_terminal_pane "${workspace}" right "Mining Server" "cd ${REPO_ROOT} && BITCOIN_DATADIR=${BITCOIN_DATADIR} CONFIG=${CONFIG} ${setup_cmd} --role server")"

  output="$(cmux browser open http://127.0.0.1:8080 --workspace "${workspace}" --focus false)"
  browser_surface="$(printf '%s\n' "${output}" | parse_ref surface || true)"
  if [[ -n "${browser_surface}" ]]; then
    rename_surface "${browser_surface}" "Canary UI"
  fi

  cat <<EOF
workspace=${workspace}
bitcoin_core=${bitcoin_surface}
mining_server=${server_surface}
ui=${browser_surface:-opened}
EOF
}

case "${1:-}" in
  --role)
    run_role "${2:-}"
    ;;
  -h|--help)
    cat <<EOF
Usage: dev/regtest/setup.sh

Creates a cmux regtest workspace with:
- Bitcoin Core regtest with IPC and RPC
- canary-mining mining server
- local UI browser pane

Useful overrides:
  WORKSPACE_NAME="canary mining"
  BITCOIN_DATADIR=/tmp/bitcoin-regtest
  CONFIG=examples/regtest.toml
EOF
    ;;
  "")
    setup_workspace
    ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
esac
