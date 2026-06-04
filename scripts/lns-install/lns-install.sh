#!/usr/bin/env bash
# Install the Lens Sandbox CLI (lns).
#
#   curl -fsSL https://get.lns.run | bash
#   curl -fsSL https://get.lns.run | VERSION=0.1.0 bash
#
# INSTALL_DIR     target directory (default: ~/.local/bin)
# VERSION         specific release (default: latest)
# CDN_BASE        override CDN origin (default: https://get.lns.run)
# LNS_NO_SERVICE  set to 1 to skip starting + enabling the login auto-start service

set -euo pipefail

CDN_BASE="${CDN_BASE:-https://get.lns.run}"
BINARY_NAME="lns"
SERVICE_BINARY_NAME="lns-service"
MANIFEST_NAME="lns-latest.json"

LNS_INSTALL_VERSION="0.4.0" # x-release-please-version

# Sent on every CDN request to correlate install volume and detect stale curl|sh pipelines.
USER_AGENT="lns-install/${LNS_INSTALL_VERSION} (os=$(uname -s); arch=$(uname -m); kernel=$(uname -s)/$(uname -r); shell=$(basename "${SHELL:-unknown}"); method=install-script)"

if [ -t 1 ] && command -v tput >/dev/null 2>&1 && [ "$(tput colors 2>/dev/null || echo 0)" -ge 8 ]; then
  BOLD=$(tput bold)
  RED=$(tput setaf 1)
  GREEN=$(tput setaf 2)
  YELLOW=$(tput setaf 3)
  RESET=$(tput sgr0)
else
  BOLD="" RED="" GREEN="" YELLOW="" RESET=""
fi

info()  { printf "%s[info]%s  %s\n" "$GREEN" "$RESET" "$1"; }
warn()  { printf "%s[warn]%s  %s\n" "$YELLOW" "$RESET" "$1" >&2; }
error() { printf "%s[error]%s %s\n" "$RED" "$RESET" "$1" >&2; exit 1; }

# Service auto-start is on by default. LNS_NO_SERVICE=1 is the primary opt-out
# (the published path is curl|bash, which has no argv); --no-service works when
# args are passed. Under `set -u`, "$@" with zero args expands to nothing.
NO_SERVICE="${LNS_NO_SERVICE:-}"
for arg in "$@"; do
  case "$arg" in
    --no-service) NO_SERVICE=1 ;;
  esac
done

download() {
  local url="$1" dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -A "$USER_AGENT" -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --user-agent="$USER_AGENT" -q -O "$dest" "$url"
  else
    error "Either curl or wget is required."
  fi
}

fetch() {
  local url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -A "$USER_AGENT" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --user-agent="$USER_AGENT" -qO- "$url"
  else
    error "Either curl or wget is required."
  fi
}

detect_os() {
  local os
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  case "$os" in
    darwin) echo "darwin" ;;
    linux)  echo "linux" ;;
    *)      error "Unsupported operating system: $os" ;;
  esac
}

detect_arch() {
  # Darwin reports `arm64`; rewrite to `aarch64` to match Rust target_arch and tarball filenames.
  local arch
  arch=$(uname -m)
  case "$arch" in
    x86_64|amd64)  echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)             error "Unsupported architecture: $arch" ;;
  esac
}

OS=$(detect_os)
ARCH=$(detect_arch)
PLATFORM="${OS}-${ARCH}"

# Reject Intel Macs early — Vz requires Apple Silicon; the binary would install but fail on first run.
if [ "$OS" = "darwin" ] && [ "$ARCH" != "aarch64" ]; then
  error "lns requires Apple Silicon (M-series) on macOS — Intel Macs cannot host the guest VM."
fi

if [ -n "${VERSION:-}" ]; then
  VERSION="${VERSION#v}"
else
  info "Resolving latest version..."
  MANIFEST=$(fetch "${CDN_BASE}/${MANIFEST_NAME}") || \
    error "Failed to fetch ${CDN_BASE}/${MANIFEST_NAME}"
  # Prefer jq; the awk fallback guards against a value literally equal to "version"
  # being mistaken for the key, and strips \r in case a proxy rewrites line endings.
  if command -v jq >/dev/null 2>&1; then
    VERSION=$(printf '%s' "$MANIFEST" | jq -r '.version // empty')
  else
    VERSION=$(printf '%s\n' "$MANIFEST" | tr -d '\r' | \
      awk -F'"' '{ for (i=1; i<NF; i++) if ($i == "version" && $(i+1) ~ /^[[:space:]]*:[[:space:]]*$/) { print $(i+2); exit } }')
  fi
  [ -n "$VERSION" ] || error "Could not parse version from manifest."
fi

info "Installing ${BOLD}${BINARY_NAME} ${VERSION}${RESET} for ${PLATFORM}"

ASSET_NAME="lns-${VERSION}-${PLATFORM}.tar.gz"
ASSET_URL="${CDN_BASE}/${ASSET_NAME}"
CHECKSUM_URL="${ASSET_URL}.sha256"

TMPDIR_INSTALL=$(mktemp -d)
trap 'rm -rf "$TMPDIR_INSTALL"' EXIT

info "Downloading ${ASSET_NAME}..."
download "$ASSET_URL" "${TMPDIR_INSTALL}/${ASSET_NAME}" || \
  error "Failed to download ${ASSET_URL}"

info "Verifying checksum..."
download "$CHECKSUM_URL" "${TMPDIR_INSTALL}/${ASSET_NAME}.sha256" || \
  error "Failed to download ${CHECKSUM_URL}"

EXPECTED=$(awk '{print $1}' "${TMPDIR_INSTALL}/${ASSET_NAME}.sha256")
[ -n "$EXPECTED" ] || error "Checksum file at ${CHECKSUM_URL} is empty or malformed."

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL=$(sha256sum "${TMPDIR_INSTALL}/${ASSET_NAME}" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL=$(shasum -a 256 "${TMPDIR_INSTALL}/${ASSET_NAME}" | awk '{print $1}')
else
  error "Neither sha256sum nor shasum is available — cannot verify download integrity. Install GNU coreutils or perl-shasum and retry."
fi

[ "$ACTUAL" = "$EXPECTED" ] || error "Checksum mismatch!
  Expected: ${EXPECTED}
  Got:      ${ACTUAL}"
info "Checksum OK."

INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
INSTALL_DIR="${INSTALL_DIR%/}"

tar xzf "${TMPDIR_INSTALL}/${ASSET_NAME}" -C "${TMPDIR_INSTALL}"
[ -f "${TMPDIR_INSTALL}/${BINARY_NAME}" ] || \
  error "Tarball ${ASSET_NAME} did not contain ${BINARY_NAME} at the top level."

HAS_SERVICE=true
if [ ! -f "${TMPDIR_INSTALL}/${SERVICE_BINARY_NAME}" ]; then
  warn "Tarball does not contain ${SERVICE_BINARY_NAME} (older release?). Skipping service binary."
  HAS_SERVICE=false
fi

# Clear the quarantine bit so Gatekeeper doesn't block the binary; the Vz entitlement is in the release signature.
if [ "$OS" = "darwin" ]; then
  xattr -d com.apple.quarantine "${TMPDIR_INSTALL}/${BINARY_NAME}" 2>/dev/null || true
  if [ "$HAS_SERVICE" = true ]; then
    xattr -d com.apple.quarantine "${TMPDIR_INSTALL}/${SERVICE_BINARY_NAME}" 2>/dev/null || true
  fi
fi

if [ ! -d "${INSTALL_DIR}" ]; then
  if ! mkdir -p "${INSTALL_DIR}" 2>/dev/null; then
    info "Creating ${INSTALL_DIR} (requires sudo)..."
    sudo mkdir -p "${INSTALL_DIR}" || error "Failed to create install directory ${INSTALL_DIR} with sudo."
  fi
fi

if ! install -m 0755 "${TMPDIR_INSTALL}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}" 2>/dev/null; then
  info "Installing to ${INSTALL_DIR} (requires sudo)..."
  sudo install -m 0755 "${TMPDIR_INSTALL}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}" || \
    error "Failed to install ${BINARY_NAME} to ${INSTALL_DIR} with sudo."
fi

if [ "$HAS_SERVICE" = true ]; then
  if ! install -m 0755 "${TMPDIR_INSTALL}/${SERVICE_BINARY_NAME}" "${INSTALL_DIR}/${SERVICE_BINARY_NAME}" 2>/dev/null; then
    info "Installing ${SERVICE_BINARY_NAME} to ${INSTALL_DIR} (requires sudo)..."
    sudo install -m 0755 "${TMPDIR_INSTALL}/${SERVICE_BINARY_NAME}" "${INSTALL_DIR}/${SERVICE_BINARY_NAME}" || \
      error "Failed to install ${SERVICE_BINARY_NAME} to ${INSTALL_DIR} with sudo."
  fi
  info "Installed ${BOLD}${BINARY_NAME}${RESET} and ${BOLD}${SERVICE_BINARY_NAME}${RESET} to ${INSTALL_DIR}/"
else
  info "Installed ${BOLD}${BINARY_NAME}${RESET} to ${INSTALL_DIR}/${BINARY_NAME}"
fi

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    CURRENT_SHELL=$(basename "${SHELL:-/bin/sh}")
    case "$CURRENT_SHELL" in
      fish)
        SNIPPET="fish_add_path ${INSTALL_DIR}"
        RC_FILE="~/.config/fish/config.fish"
        ;;
      zsh)
        SNIPPET="export PATH=\"${INSTALL_DIR}:\$PATH\""
        RC_FILE="~/.zshrc"
        ;;
      *)
        SNIPPET="export PATH=\"${INSTALL_DIR}:\$PATH\""
        RC_FILE="~/.bashrc"
        ;;
    esac
    echo ""
    warn "${INSTALL_DIR} is not in your PATH. Add this to ${RC_FILE}:"
    echo ""
    echo "    ${SNIPPET}"
    echo ""
    echo "  Or to use ${BINARY_NAME} in this shell right now, run that same line."
    echo ""
    ;;
esac

# Warn (not error): the binary is still useful for `lns audit`, `lns --help`, etc. without KVM.
if [ "$OS" = "linux" ] && [ ! -e /dev/kvm ]; then
  warn "/dev/kvm is not available. \`${BINARY_NAME} run\` will fail to launch a guest VM."
  echo "  Check that:"
  echo "    - the kernel supports KVM (modprobe kvm_intel | kvm_amd)"
  echo "    - hardware virtualisation is enabled in BIOS/UEFI"
  echo "    - your user can access /dev/kvm (usually: sudo usermod -aG kvm \$USER)"
  echo ""
fi

# lns-service links against libgtk-3, libayatana-appindicator3, libxdo; probe via ldconfig so
# minimal Linux installs (server, NixOS, Alpine) get a friendly error instead of an opaque loader crash.
if [ "$OS" = "linux" ] && [ "$HAS_SERVICE" = true ] && command -v ldconfig >/dev/null 2>&1; then
  MISSING_LIBS=""
  for lib in libgtk-3.so.0 libayatana-appindicator3.so.1 libxdo.so.3; do
    if ! ldconfig -p 2>/dev/null | grep -q " ${lib} "; then
      MISSING_LIBS="${MISSING_LIBS} ${lib}"
    fi
  done
  if [ -n "$MISSING_LIBS" ]; then
    warn "Some shared libraries needed by ${SERVICE_BINARY_NAME} are not present. \`${BINARY_NAME} service start\` will fail with a loader error."
    echo "  Missing:${MISSING_LIBS}"
    echo "  Install (Debian/Ubuntu): sudo apt-get install -y libgtk-3-0 libayatana-appindicator3-1 libxdo3"
    echo "  Install (Fedora/RHEL):   sudo dnf install -y gtk3 libayatana-appindicator-gtk3 libxdo"
    echo ""
  fi
fi

if [ "$HAS_SERVICE" = true ] && [ -z "$NO_SERVICE" ]; then
  info "Starting the Lens Sandbox service and enabling login auto-start..."
  # Never let enable fail the install: `lns service enable` exits 0 even when it
  # degrades to start-for-session on a headless/no-bus host. The `if` guard keeps
  # `set -e` from aborting if the binary itself is unrunnable.
  if "${INSTALL_DIR}/${BINARY_NAME}" service enable; then
    SERVICE_ENABLED=true
  else
    SERVICE_ENABLED=false
    warn "Could not enable the background service automatically. Start it yourself with: ${BINARY_NAME} service enable"
  fi
else
  SERVICE_ENABLED=false
fi

"${INSTALL_DIR}/${BINARY_NAME}" --version

echo ""
if [ "$SERVICE_ENABLED" = true ]; then
  echo "${BOLD}▸ Service:${RESET}  running now and on every login (stop: ${BINARY_NAME} service stop · remove auto-start: ${BINARY_NAME} service disable)"
elif [ "$HAS_SERVICE" = true ] && [ -n "$NO_SERVICE" ]; then
  echo "${BOLD}▸ Service:${RESET}  skipped (LNS_NO_SERVICE/--no-service). Start it with: ${BINARY_NAME} service enable"
fi
echo "${BOLD}▸ Try it:${RESET}   ${BINARY_NAME} run -- echo hello from inside a microVM"
echo "${BOLD}▸ Help:${RESET}     ${BINARY_NAME} --help"
