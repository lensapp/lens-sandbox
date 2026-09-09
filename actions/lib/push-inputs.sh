#!/usr/bin/env bash
# What lns-push publishes, read off its inputs: every reference fully
# qualified, in order, and the exact-tool-version switch in one of its two
# positions. Nothing here talks to a registry.
set -euo pipefail

HUB=hub.lns.run

refuse_tag() {
  echo "::error::tag '$1' $2. Use <namespace>/<name>:<tag> or <host>/<namespace>/<name>:<tag> — lns-push never assumes a namespace."
  exit 1
}

# Only what the workflow's line wrapping added: whitespace inside a reference
# is a malformed reference, and lns is the one that says so.
trim() {
  local value=$1
  value=${value#"${value%%[![:space:]]*}"}
  printf '%s' "${value%"${value##*[![:space:]]}"}"
}

host_of() {
  case "${1%%/*}" in
    *.* | *:* | localhost) printf '%s' "${1%%/*}" ;;
    *) printf '%s' "$HUB" ;;
  esac
}

repository_of() {
  local repository=$1
  if [ "$(host_of "$repository")" != "$HUB" ] || [ "${repository%%/*}" = "$HUB" ]; then
    repository=${repository#*/}
  fi
  printf '%s' "$repository"
}

exact_tool_versions() {
  case "${INPUT_EXACT_TOOLS-}" in
    true | false) EXACT_TOOLS=$INPUT_EXACT_TOOLS ;;
    *)
      echo "::error::require-exact-tool-versions must be 'true' or 'false', not '${INPUT_EXACT_TOOLS-}'."
      exit 1
      ;;
  esac
}

main() {
  local file first host repository raw tag tag_host tag_repository
  exact_tool_versions

  file="$RUNNER_TEMP/lns-push-tags.txt"
  : >"$file"
  first=""
  host=""
  repository=""
  while IFS= read -r raw; do
    tag=$(trim "$raw")
    [ -n "$tag" ] || continue
    case "${tag##*/}" in
      *:*) : ;;
      *) refuse_tag "$tag" "names no tag" ;;
    esac
    tag_host=$(host_of "${tag%:*}")
    tag_repository=$(repository_of "${tag%:*}")
    case "$tag_repository" in
      */*) : ;;
      *) refuse_tag "$tag" "has no namespace segment" ;;
    esac
    tag="$tag_host/$tag_repository:${tag##*:}"
    printf '%s\n' "$tag" >>"$file"
    if [ -z "$first" ]; then
      first=$tag
      host=$tag_host
      repository=$tag_repository
    fi
  done <<TAGS
$(printf '%s' "${INPUT_TAGS-}" | tr ',' '\n')
TAGS

  if [ -z "$first" ]; then
    echo "::error::tags is empty; give at least one <namespace>/<name>:<tag>."
    exit 1
  fi
  {
    echo "first=$first"
    echo "host=$host"
    echo "repository=$repository"
    echo "file=$file"
    echo "exact-tools=$EXACT_TOOLS"
  } >>"$GITHUB_OUTPUT"
}

main "$@"
