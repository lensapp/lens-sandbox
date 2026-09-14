#!/usr/bin/env bash
# What lns-push publishes and what it then reports: one manifest under every
# tag. Each push rebuilds the document, and a fuzzy tool version resolves per
# push, so the tags are only interchangeable once their digests agree — and
# `digest` is only true of every ref in `refs` once that is checked.
set -euo pipefail

probe_first_push() {
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}' -m 30 \
    "https://$REGISTRY_HOST/v2/$REPOSITORY/tags/list" || echo 000)
  [ "$code" = 404 ] && printf 'true' || printf 'false'
}

refuse_second_manifest() {
  echo "::error::'$1' published $2 but '$3' published $4. Both are now in the registry pointing at different manifests, and a single digest describes neither — set require-exact-tool-versions: true, or pin the document's tool versions, so every tag builds the same manifest."
  exit 1
}

report_published() {
  local heading=$1 digest=$2 refs_md=$3 first_push=$4
  {
    echo "### $heading"
    echo
    if [ -n "$digest" ]; then
      echo "Digest \`$digest\`, from \`$INPUT_FILE\`:"
    else
      echo "From \`$INPUT_FILE\`:"
    fi
    echo
    printf '%s' "$refs_md"
    if [ "$first_push" = true ]; then
      echo
      case "$REGISTRY_HOST" in
        hub.lns.run | hub.staging.lns.run)
          echo "$REPOSITORY is new and private. Publish it at https://$REGISTRY_HOST/$REPOSITORY/settings"
          ;;
        *) echo "Anonymous repository probe returned HTTP 404 for $REGISTRY_HOST/$REPOSITORY; check your registry's visibility controls." ;;
      esac
    fi
  } >>"$GITHUB_STEP_SUMMARY"
}

push_one() {
  local tag=$1 output status pushed
  status=0
  output=$(lns artifact push "$tag" -f "$INPUT_FILE" --yes 2>&1) || status=$?
  printf '%s\n' "$output" >&2
  if [ "$status" -ne 0 ]; then
    echo "::error::pushing $tag failed." >&2
    return "$status"
  fi
  pushed=$(printf '%s\n' "$output" | grep '^built and pushed ' | tail -1 || true)
  if [ -z "$pushed" ]; then
    echo "::error::pushing $tag reported no digest, so nothing here can say what it published." >&2
    return 1
  fi
  printf '%s' "${pushed##*@}"
}

main() {
  local first_push digest first_ref refs refs_md tag tag_digest status=0
  first_push=$(probe_first_push)

  digest=""
  first_ref=""
  refs=""
  refs_md=""
  while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    if tag_digest=$(push_one "$tag"); then
      :
    else
      status=$?
      break
    fi
    refs_md="$refs_md- \`$tag\` — \`$tag_digest\`"$'\n'
    if [ -n "$digest" ] && [ "$tag_digest" != "$digest" ]; then
      report_published "lns-push published two manifests" "" "$refs_md" "$first_push"
      refuse_second_manifest "$first_ref" "$digest" "$tag" "$tag_digest"
    fi
    digest=$tag_digest
    [ -n "$first_ref" ] || first_ref=$tag
    refs="$refs$tag"$'\n'
  done <"$TAGS_FILE"

  if [ "$status" -ne 0 ] && [ -z "$refs" ]; then
    return "$status"
  fi

  {
    echo "digest=$digest"
    echo "first-push=$first_push"
    echo 'refs<<LNS_REFS_EOF'
    printf '%s' "$refs"
    echo 'LNS_REFS_EOF'
  } >>"$GITHUB_OUTPUT"

  if [ "$status" -ne 0 ]; then
    report_published "lns-push partially published" "$digest" "$refs_md" "$first_push" || return "$status"
  else
    report_published "lns-push published" "$digest" "$refs_md" "$first_push"
  fi
  return "$status"
}

main "$@"
