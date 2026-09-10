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
      echo "$REPOSITORY is new and private. Publish it at https://$REGISTRY_HOST/$REPOSITORY/settings"
    fi
  } >>"$GITHUB_STEP_SUMMARY"
}

push_one() {
  local tag=$1 output status pushed
  set +e
  output=$(lns artifact push "$tag" -f "$INPUT_FILE" --yes 2>&1)
  status=$?
  set -e
  printf '%s\n' "$output" >&2
  if [ "$status" -ne 0 ]; then
    echo "::error::pushing $tag failed." >&2
    exit "$status"
  fi
  pushed=$(printf '%s\n' "$output" | grep '^built and pushed ' | tail -1 || true)
  if [ -z "$pushed" ]; then
    echo "::error::pushing $tag reported no digest, so nothing here can say what it published." >&2
    exit 1
  fi
  printf '%s' "${pushed##*@}"
}

main() {
  local first_push digest first_ref refs refs_md tag tag_digest
  first_push=$(probe_first_push)

  digest=""
  first_ref=""
  refs=""
  refs_md=""
  while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    tag_digest=$(push_one "$tag")
    refs_md="$refs_md- \`$tag\` — \`$tag_digest\`"$'\n'
    if [ -n "$digest" ] && [ "$tag_digest" != "$digest" ]; then
      report_published "lns-push published two manifests" "" "$refs_md" "$first_push"
      refuse_second_manifest "$first_ref" "$digest" "$tag" "$tag_digest"
    fi
    digest=$tag_digest
    [ -n "$first_ref" ] || first_ref=$tag
    refs="$refs$tag"$'\n'
  done <"$TAGS_FILE"

  {
    echo "digest=$digest"
    echo "first-push=$first_push"
    echo 'refs<<LNS_REFS_EOF'
    printf '%s' "$refs"
    echo 'LNS_REFS_EOF'
  } >>"$GITHUB_OUTPUT"

  report_published "lns-push published" "$digest" "$refs_md" "$first_push"
}

main "$@"
