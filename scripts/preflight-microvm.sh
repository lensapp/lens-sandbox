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
virtiofsd_found() {
	if [ -n "${LNS_VIRTIOFSD_BIN:-}" ] && [ -x "${LNS_VIRTIOFSD_BIN}" ]; then
		return 0
	fi
	if command -v virtiofsd >/dev/null 2>&1; then
		return 0
	fi
	for dir in /usr/libexec /usr/lib/virtiofsd /usr/lib/qemu; do
		[ -x "$dir/virtiofsd" ] && return 0
	done
	return 1
}
if ! virtiofsd_found; then
	echo "preflight: virtiofsd not found on PATH, in /usr/libexec, /usr/lib/virtiofsd, /usr/lib/qemu, or via LNS_VIRTIOFSD_BIN." >&2
	echo "          install it: sudo apt-get install -y virtiofsd" >&2
	fail=1
fi

if [ "$fail" -ne 0 ]; then
	echo "preflight: microVM host prerequisites are missing (see above)." >&2
	exit 1
fi
