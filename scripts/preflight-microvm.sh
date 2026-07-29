#!/usr/bin/env bash
# Fail fast if this Linux host can't boot a Cloud Hypervisor guest for `make e2e-microvm`.
#
# Checks /dev/kvm access plus cloud-hypervisor and virtiofsd reachability
# (honoring the LNS_CLOUD_HYPERVISOR_BIN / LNS_VIRTIOFSD_BIN overrides the
# service itself reads). macOS uses the built-in Vz backend and skips this.

set -euo pipefail

fail=0

if [ ! -e /dev/kvm ]; then
	echo "preflight: /dev/kvm is missing — KVM is required to boot a guest." >&2
	echo "          load the module (modprobe kvm_intel | kvm_amd) and ensure your" >&2
	echo "          user can reach /dev/kvm (sudo usermod -aG kvm \$USER, then re-login)." >&2
	fail=1
elif [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
	echo "preflight: /dev/kvm is present but not read/writable by $(id -un)." >&2
	echo "          add yourself to the kvm group: sudo usermod -aG kvm \$USER (then re-login)." >&2
	fail=1
fi

if [ -z "${LNS_CLOUD_HYPERVISOR_BIN:-}" ] && ! command -v cloud-hypervisor >/dev/null 2>&1; then
	echo "preflight: cloud-hypervisor is not on PATH and LNS_CLOUD_HYPERVISOR_BIN is unset." >&2
	echo "          install it or point LNS_CLOUD_HYPERVISOR_BIN at the binary." >&2
	fail=1
fi

# Mirrors the service's resolution: override, then PATH, then the distro dirs (apt ships virtiofsd off PATH under /usr/libexec).
find_virtiofsd() {
	if [ -n "${LNS_VIRTIOFSD_BIN:-}" ] && [ -x "${LNS_VIRTIOFSD_BIN}" ]; then
		printf '%s\n' "${LNS_VIRTIOFSD_BIN}"
		return
	fi
	if command -v virtiofsd >/dev/null 2>&1; then
		command -v virtiofsd
		return
	fi
	for dir in /usr/libexec /usr/lib/virtiofsd /usr/lib/qemu; do
		if [ -x "$dir/virtiofsd" ]; then
			printf '%s\n' "$dir/virtiofsd"
			return
		fi
	done
	return 1
}
VIRTIOFSD_PATH="$(find_virtiofsd || true)"
if [ -z "$VIRTIOFSD_PATH" ]; then
	echo "preflight: virtiofsd not found on PATH, in /usr/libexec, /usr/lib/virtiofsd, /usr/lib/qemu, or via LNS_VIRTIOFSD_BIN." >&2
	echo "          install it: sudo apt-get install -y virtiofsd" >&2
	fail=1
elif ! "$VIRTIOFSD_PATH" --help 2>&1 | grep -q -- "--readonly"; then
	echo "preflight: virtiofsd at $VIRTIOFSD_PATH does not support read-only shares (--readonly)." >&2
	echo "          install a newer virtiofsd with --readonly support or set LNS_VIRTIOFSD_BIN=/path/to/virtiofsd." >&2
	fail=1
fi

if [ "$fail" -ne 0 ]; then
	echo "preflight: microVM host prerequisites are missing (see above)." >&2
	exit 1
fi
