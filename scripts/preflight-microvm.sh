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

if [ -z "${LNS_VIRTIOFSD_BIN:-}" ] && ! command -v virtiofsd >/dev/null 2>&1; then
	echo "preflight: virtiofsd is not on PATH and LNS_VIRTIOFSD_BIN is unset." >&2
	echo "          install it or point LNS_VIRTIOFSD_BIN at the binary." >&2
	fail=1
fi

if [ "$fail" -ne 0 ]; then
	echo "preflight: microVM host prerequisites are missing (see above)." >&2
	exit 1
fi
