#!/usr/bin/env bash
# One registry entry in ~/.lns/registry-auth.json, added or removed. The CLI
# owns that store; writing the file here is the fallback for a runner with no
# lns-service, and it goes away when work item 4 of #398 ships headless login.
set -euo pipefail

NO_SERVICE='background service must be running'

usage() {
  {
    echo "usage: registry-auth.sh add|remove"
    echo "  env: LNS_ACTION_REGISTRY, LNS_ACTION_USERNAME (add), LNS_ACTION_PASSWORD (add)"
  } >&2
}

canonical_registry() {
  local host
  host=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
  host=${host%/}
  case "$host" in
    docker.io | index.docker.io | registry-1.docker.io | registry.hub.docker.com) echo docker.io ;;
    *) echo "$host" ;;
  esac
}

require_bare_host() {
  case "$1" in
    '')
      echo "::error::registry must not be empty."
      exit 1
      ;;
    *://*)
      echo "::error::registry '$1' must be a bare host, not a URL (drop the scheme)."
      exit 1
      ;;
    */*)
      echo "::error::registry '$1' must be a bare host[:port], not a path."
      exit 1
      ;;
  esac
}

auth_file() {
  local home=${LNS_HOME:-}
  if [ -z "$home" ]; then
    home="$HOME/.lns"
  fi
  echo "$home/registry-auth.json"
}

require_lns() {
  if ! command -v lns >/dev/null 2>&1; then
    echo "::error::lns is not on PATH; run lensapp/lens-sandbox/actions/setup-lns first."
    exit 1
  fi
}

require_jq() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "::error::registry-auth needs jq, which is not on PATH."
    exit 1
  fi
}

# Rewrites the auth file through a jq program that reads the current entries,
# keeping every other host and the 0600 the CLI writes.
rewrite_auth_file() {
  local program=$1
  shift
  local file current tmp
  file=$(auth_file)
  require_jq
  mkdir -p "$(dirname "$file")"
  current='{}'
  if [ -s "$file" ]; then
    if ! current=$(jq -e . "$file" 2>/dev/null); then
      echo "::error::$file is not valid JSON; lns cannot read it either."
      exit 1
    fi
  fi
  tmp="$file.$$.tmp"
  (
    umask 077
    printf '%s' "$current" | jq "$@" "$program" >"$tmp"
  )
  chmod 0600 "$tmp"
  mv -f "$tmp" "$file"
  printf 'wrote %s\n' "$file"
}

# The CLI owns the store, so only "no service" sends us to the file; every
# other failure is the CLI's answer and stands, bar the one a caller names as
# the state it asked for.
fallback_after_cli() {
  local what=$1 output=$2 status=$3 already=${4:-}
  if [ "$status" -eq 0 ]; then
    printf '%s\n' "$output"
    return 1
  fi
  if [ -n "$already" ]; then
    case "$output" in
      *"$already"*)
        printf '%s\n' "$output"
        return 1
        ;;
    esac
  fi
  case "$output" in
    *"$NO_SERVICE"*)
      printf 'lns-service is not running on this runner; %s writes %s directly.\n' \
        "$what" "$(auth_file)" >&2
      return 0
      ;;
    *)
      printf '%s\n' "$output" >&2
      echo "::error::lns $what failed."
      exit 1
      ;;
  esac
}

add() {
  local registry=$1 username=$2 output status
  if [ -z "$username" ]; then
    echo "::error::username is required."
    exit 1
  fi
  if [ -z "${LNS_ACTION_PASSWORD:-}" ]; then
    echo "::error::password is required."
    exit 1
  fi
  require_lns
  set +e
  output=$(printf '%s' "$LNS_ACTION_PASSWORD" |
    lns login "$registry" -u "$username" --password-stdin 2>&1)
  status=$?
  set -e
  if fallback_after_cli login "$output" "$status"; then
    # shellcheck disable=SC2016 # a jq program, not shell expansion
    rewrite_auth_file '.[$registry] = {username: $username, secret: $ENV.LNS_ACTION_PASSWORD}' \
      --arg registry "$registry" --arg username "$username"
  fi
  printf 'Logged in to %s as %s.\n' "$registry" "$username"
}

remove() {
  local registry=$1 output status
  require_lns
  set +e
  output=$(lns logout "$registry" 2>&1)
  status=$?
  set -e
  if fallback_after_cli logout "$output" "$status" 'not logged in'; then
    # shellcheck disable=SC2016 # a jq program, not shell expansion
    rewrite_auth_file 'del(.[$registry])' --arg registry "$registry"
  fi
  printf 'Logged out of %s.\n' "$registry"
}

main() {
  local mode=${1:-} registry
  registry=$(canonical_registry "${LNS_ACTION_REGISTRY:-}")
  case "$mode" in
    add)
      require_bare_host "$registry"
      add "$registry" "${LNS_ACTION_USERNAME:-}"
      ;;
    remove)
      require_bare_host "$registry"
      remove "$registry"
      ;;
    *)
      usage
      exit 1
      ;;
  esac
}

main "$@"
