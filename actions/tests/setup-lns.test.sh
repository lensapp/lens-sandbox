#!/bin/sh
set -eu
setup_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
setup_tmp=$(mktemp -d)
trap 'rm -rf "$setup_tmp"' EXIT HUP INT TERM
mkdir -p "$setup_tmp/bin" "$setup_tmp/source/Contents/Helpers"
printf '#!/bin/sh\necho "lns 0.26.0"\n' > "$setup_tmp/source/Contents/Helpers/lns"
chmod +x "$setup_tmp/source/Contents/Helpers/lns"
awk '/^      run: \|$/ { body=1; next } body { sub(/^        /, ""); print }' "$setup_root/actions/setup-lns/action.yml" > "$setup_tmp/install.sh"
cat > "$setup_tmp/bin/uname" <<'MOCK'
#!/bin/sh
set -eu
case "$1" in -s) echo "${SETUP_OS:-Darwin}";; -m) echo arm64;; esac
MOCK
cat > "$setup_tmp/bin/gh" <<'MOCK'
#!/bin/sh
set -eu
if [ "$1 $2" = 'release view' ]; then
  echo "lns-0.26.0-darwin-aarch64.${SETUP_EXTENSION:-zip}"
  exit
fi
[ "$1 $2" = 'release download' ] || exit 1
shift 3
while [ "$#" -gt 0 ]; do
  case "$1" in --dir) directory=$2;; --pattern) asset=$2;; esac
  shift 2
done
asset=${asset%.sha256}
case "$asset" in
  *.zip) [ "${SETUP_EXTENSION:-zip}" = zip ]; touch "$directory/$asset";;
  *.tar.gz) [ "${SETUP_EXTENSION:-zip}" = tar.gz ] || { echo "FAIL: requested a tarball from a ZIP release" >&2; exit 1; }; tar czf "$directory/$asset" -C "$SETUP_SOURCE/Contents/Helpers" lns;;
  *) exit 1;;
esac
(cd "$directory" && /usr/bin/shasum -a 256 "$asset" > "$asset.sha256")
MOCK
cat > "$setup_tmp/bin/ditto" <<'MOCK'
#!/bin/sh
set -eu
[ "$1 $2" = '-x -k' ]
mkdir -p "$4"
cp -R "$SETUP_SOURCE" "$4/LNS.app"
MOCK
cat > "$setup_tmp/bin/shasum" <<'MOCK'
#!/bin/sh
set -eu
[ "${SETUP_BAD_CHECKSUM:-0}" != 1 ] || exit 1
exec /usr/bin/shasum "$@"
MOCK
chmod +x "$setup_tmp/bin/"*
export PATH="$setup_tmp/bin:$PATH" SETUP_SOURCE="$setup_tmp/source"
export INPUT_VERSION=0.26.0 INPUT_BINARY='' RELEASE_REPO=lensapp/lens-sandbox
export GITHUB_PATH="$setup_tmp/path" GITHUB_OUTPUT="$setup_tmp/output"
export RUNNER_TOOL_CACHE="$setup_tmp/cache"
export TMPDIR="$setup_tmp"
bash "$setup_tmp/install.sh"
[ "$("$RUNNER_TOOL_CACHE/lns/0.26.0/aarch64/lns" --version)" = 'lns 0.26.0' ]
[ ! -e "$RUNNER_TOOL_CACHE/lns/0.26.0/aarch64/lns-service" ]
echo 'PASS: the macOS action installs the CLI from the native release ZIP'
rm -rf "$RUNNER_TOOL_CACHE"
SETUP_EXTENSION=tar.gz bash "$setup_tmp/install.sh"
[ -x "$RUNNER_TOOL_CACHE/lns/0.26.0/aarch64/lns" ]
echo 'PASS: pinned historical macOS tarball releases remain installable'
rm -rf "$RUNNER_TOOL_CACHE"
if SETUP_BAD_CHECKSUM=1 bash "$setup_tmp/install.sh"; then
  echo 'FAIL: invalid checksum was installed' >&2; exit 1
fi
[ ! -e "$RUNNER_TOOL_CACHE/lns/0.26.0/aarch64/lns" ]
echo 'PASS: checksum failure stops installation'

SETUP_OS=Linux SETUP_EXTENSION=tar.gz bash "$setup_tmp/install.sh"
[ "$("$RUNNER_TOOL_CACHE/lns/0.26.0/aarch64/lns" --version)" = 'lns 0.26.0' ]
echo 'PASS: Linux installs the existing CLI tarball'

echo "Results: 4 passed, 0 failed"
