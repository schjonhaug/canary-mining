#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLAYWRIGHT_DIR="$SCRIPT_DIR/playwright"

(
  cd "$PLAYWRIGHT_DIR"
  if [[ ! -d node_modules/@playwright/test ]]; then
    npm ci
    npx playwright install chromium
  fi
  npm run gallery:umbrel
)
