#!/usr/bin/env bash
# `::add-mask::` on a secret the runner will decode: `%`, CR and LF are the
# workflow command's own syntax, so an unescaped value ends the command early
# and is masked as something other than what was entered.
set -euo pipefail

secret=${LNS_ACTION_SECRET:-}
name=${LNS_ACTION_SECRET_NAME:-password}

if [ -z "$secret" ]; then
  echo "::error::$name is required."
  exit 1
fi

escaped=${secret//'%'/%25}
escaped=${escaped//$'\r'/%0D}
escaped=${escaped//$'\n'/%0A}

printf '::add-mask::%s\n' "$escaped"
