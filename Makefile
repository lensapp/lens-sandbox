.PHONY: dev build build-lns build-lns-service test lint fmt complexity complexity-all clean coverage coverage-data coverage-affected coverage-lcov e2e e2e-microvm preflight-microvm audit install-hooks

CARGO ?= cargo

# `CARGO_LOCKED=--locked make <step>` from CI; empty locally.
CARGO_LOCKED ?=

WORKSPACE_ROOT = $(shell $(CARGO) metadata --format-version 1 --no-deps | \
	(jq -r .workspace_root 2>/dev/null || \
	 sed -n 's/.*"workspace_root":"\([^"]*\)".*/\1/p'))
WORKSPACE_MANIFEST = $(WORKSPACE_ROOT)/Cargo.toml
CARGO_TARGET_DIR = $(shell $(CARGO) metadata --format-version 1 --no-deps | \
	(jq -r .target_directory 2>/dev/null || \
	 sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'))

# Coverage isolates instrumented builds from `target/`: cargo-llvm-cov
# injects -Cinstrument-coverage via RUSTC_WRAPPER which cargo's
# fingerprint doesn't track. Sharing target/ with `make test`'s
# non-instrumented artifacts silently reuses them, emits no profraw,
# and the report step fails with "not found *.profraw files".
COVERAGE_TARGET_DIR = $(WORKSPACE_ROOT)/target-cov

# Crates whose code is enforced by the per-crate `complexity` gate,
# which iterates this list with `cd crates/<crate> && cargo …` so each
# invocation runs with that crate's own feature set instead of the
# workspace-unified one (see the comment on the `complexity` target
# below for why that matters). Decoupled from the shipping-artifact
# list above (`build-lns` / `build-lns-service`) — a crate can be in
# this gate without producing a binary (e.g. `lns-ipc` is a pure
# library).
GATE_CRATES := lns-cli lns-service lns-ipc lns-audit lns-ocsf lns-policy

# Every crate under crates/ EXCEPT e2e-tests. Used by `make coverage`
# iteration. e2e-tests is excluded because Layer 1 tests don't
# contribute to the coverage gate (see CLAUDE.md "Test layers").
ALL_CRATES := $(filter-out crates/e2e-tests,$(patsubst %/,%,$(wildcard crates/*/)))

# When COVERAGE_CRATES is set (space- or newline-separated crate names),
# the coverage recipe narrows to just those crates. Unset = all crates.
COVERAGE_CRATES_LIST := $(if $(strip $(COVERAGE_CRATES)),$(foreach c,$(COVERAGE_CRATES),crates/$(c)),$(ALL_CRATES))
COVERAGE_CARGO_SCOPE := $(if $(strip $(COVERAGE_CRATES)),$(foreach c,$(COVERAGE_CRATES),-p $(c)),--workspace --exclude e2e-tests)

# ── Dev loop ──────────────────────────────────────────────────────────

# Inner dev loop: debug build of the two user-facing crates. Skips
# the lns-init + lns-session-broker aarch64-musl cross-builds (~40x
# faster incremental than `make build`) and the static-nft embed.
# Set LNS_INIT_BIN / LNS_SESSION_BROKER_BIN / LNS_NFT_BIN to `<path>`
# at runtime to use a real pre-built guest binary instead.
dev: export LNS_INIT_BIN := skip
dev: export LNS_SESSION_BROKER_BIN := skip
dev: export LNS_NFT_BIN := skip
dev: export LNS_SUPERVISOR_BIN := skip
dev:
	$(CARGO) build -p lns-cli -p lns-service

# ── Shipping artifacts (not part of the gate) ─────────────────────────
# The kernel caches a vnode's code signature, so cp over an already-executed
# binary gets the next exec SIGKILLed on macOS; unlink before copying.

build: build-lns build-lns-service

build-lns:
	$(CARGO) build --release $(CARGO_LOCKED) -p lns-cli
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		./crates/lns-cli/scripts/codesign-macos.sh "$(CARGO_TARGET_DIR)/release/lns" >/dev/null; \
	fi
	@mkdir -p bin
	@rm -f bin/lns
	@cp $(CARGO_TARGET_DIR)/release/lns bin/lns

build-lns-service:
	$(CARGO) build --release $(CARGO_LOCKED) -p lns-service
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		./crates/lns-cli/scripts/codesign-macos.sh "$(CARGO_TARGET_DIR)/release/lns-service" >/dev/null; \
	fi
	@mkdir -p bin
	@rm -f bin/lns-service
	@cp $(CARGO_TARGET_DIR)/release/lns-service bin/lns-service

# ── Gate steps ────────────────────────────────────────────────────────
# `lint` and `test` run workspace-wide (one cargo invocation each).
# `complexity` iterates GATE_CRATES per-crate — workspace clippy with
# `-D cognitive_complexity` and per-crate clippy disagree because of
# feature unification; see the comment on the `complexity` target.
# `coverage` is its own multi-stage pipeline (see the Coverage section
# below). CI invokes these targets as parallel jobs, with
# `CARGO_LOCKED=--locked make <step>` for strictness on lint/test.

lint: export LNS_INIT_BIN := skip
lint: export LNS_SESSION_BROKER_BIN := skip
lint: export LNS_NFT_BIN := skip
lint: export LNS_SUPERVISOR_BIN := skip
lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets $(CARGO_LOCKED) -- -D warnings -D clippy::undocumented_unsafe_blocks

# `--exclude e2e-tests`: the Layer 1 cucumber harness spawns real
# binaries — owned by `make e2e`, not the fast in-process test gate.
test: export LNS_INIT_BIN := skip
test: export LNS_SESSION_BROKER_BIN := skip
test: export LNS_NFT_BIN := skip
test: export LNS_SUPERVISOR_BIN := skip
test:
	$(CARGO) test --workspace --exclude e2e-tests --all-targets $(CARGO_LOCKED)

# Per-crate cargo invocations (not workspace-wide): workspace feature
# unification expands shared deps differently than per-crate clippy,
# and `cognitive_complexity` workspace-wide flags functions that pass
# the per-crate gate. Keep complexity per-crate on both sides for
# parity.
complexity: export LNS_INIT_BIN := skip
complexity: export LNS_SESSION_BROKER_BIN := skip
complexity: export LNS_NFT_BIN := skip
complexity: export LNS_SUPERVISOR_BIN := skip
complexity:
	@status=0; for crate in $(GATE_CRATES); do \
		(cd crates/$$crate && $(CARGO) clippy --all-targets -- -D clippy::cognitive_complexity) || status=$$?; \
	done; exit $$status

complexity-all: complexity

fmt:
	$(CARGO) fmt --all

# ── Coverage ──────────────────────────────────────────────────────────
# Two phases:
#   1. `coverage-data` (workspace one-shot) — cargo-llvm-cov produces
#      the AST-stripped lcov.info in COVERAGE_TARGET_DIR.
#   2. `coverage` — iterates ALL_CRATES running scripts/coverage-floor.sh
#      against the lcov, filtered to each crate's subtree.
#
# `show-env` emits RUSTFLAGS / LLVM_PROFILE_FILE so the test step runs
# fully instrumented (cargo-llvm-cov has no `build` subcommand — only
# test/run/nextest).
coverage-data: export CARGO_TARGET_DIR := $(COVERAGE_TARGET_DIR)
coverage-data: export LNS_INIT_BIN := skip
coverage-data: export LNS_SESSION_BROKER_BIN := skip
coverage-data: export LNS_NFT_BIN := skip
coverage-data: export LNS_SUPERVISOR_BIN := skip
coverage-data:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { \
		echo "cargo-llvm-cov not installed. Install with: cargo install cargo-llvm-cov"; \
		exit 1; \
	}
	# Strip tool built OUTSIDE the cargo-llvm-cov env so it isn't
	# instrumented. Its output (the stripped lcov) is what the gate measures.
	# Debug mode: `syn` (heavy dep) compiles ~3x faster in debug than release;
	# runtime cost over ~100 source files is sub-second either way.
	$(CARGO) build -p coverage-strip-ast
	$(CARGO) llvm-cov clean --workspace
	@set -e; \
		eval "$$($(CARGO) llvm-cov show-env --export-prefix)"; \
		$(if $(strip $(COVERAGE_CRATES)),$(if $(filter lns-service,$(COVERAGE_CRATES)),$(CARGO) build -p lns-service;,),$(CARGO) build -p lns-service;) \
		$(CARGO) test $(COVERAGE_CARGO_SCOPE) --all-targets; \
		$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST); \
		$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST) --html; \
		$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST) --lcov \
			--output-path $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info; \
		$(COVERAGE_TARGET_DIR)/debug/coverage-strip-ast $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info; \
		echo "HTML report: $(COVERAGE_TARGET_DIR)/llvm-cov/html/index.html"

coverage: coverage-data
	@status=0; \
		for pkg in $(COVERAGE_CRATES_LIST); do \
			echo ""; \
			echo "── $$pkg ──"; \
			./scripts/coverage-floor.sh $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info "$$pkg/" || status=$$?; \
		done; \
		exit $$status

BASE_REF ?= origin/main
coverage-affected:
	@out=$$(./scripts/affected-crates.sh $(BASE_REF)); \
		case "$$out" in \
			__NONE__) echo "no crates affected; skipping coverage"; exit 0 ;; \
			__FULL__) $(MAKE) coverage ;; \
			*) COVERAGE_CRATES="$$(echo "$$out" | tr '\n' ' ')" $(MAKE) coverage ;; \
		esac

# Re-emit the last collected profraw as LCOV (for codecov, IDE plugins).
# Runs after `make coverage` — does not re-run the tests.
coverage-lcov: export CARGO_TARGET_DIR := $(COVERAGE_TARGET_DIR)
coverage-lcov:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { \
		echo "cargo-llvm-cov not installed. Install with: cargo install cargo-llvm-cov"; \
		exit 1; \
	}
	@set -e; \
		eval "$$($(CARGO) llvm-cov show-env --export-prefix)"; \
		$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST) --lcov \
			--output-path $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info; \
		echo "LCOV: $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info"

# ── End-to-end (Layer 1) + smoke ──────────────────────────────────────

# Builds the real `lns` and `lns-service` binaries and runs the cucumber
# harness in `crates/e2e-tests/` with their paths passed via LNS_BIN /
# LNS_SERVICE_BIN. Excluded from the coverage gate (spawns real
# subprocesses with side effects).
e2e: export LNS_INIT_BIN := skip
e2e: export LNS_SESSION_BROKER_BIN := skip
e2e: export LNS_NFT_BIN := skip
e2e: export LNS_SUPERVISOR_BIN := skip
e2e:
	$(CARGO) build -p lns-cli -p lns-service
	@LNS_BIN=$(CARGO_TARGET_DIR)/debug/lns \
		LNS_SERVICE_BIN=$(CARGO_TARGET_DIR)/debug/lns-service \
		$(CARGO) test -p e2e-tests --test e2e

# Boots a REAL microVM and runs the @microvm scenarios. Unlike `e2e`,
# this needs the embedded guest binaries (lns-init / broker / nft /
# supervisor) that only `make build` produces — hence the dependency and
# no LNS_*_BIN := skip. On macOS Vz also requires the codesigned
# com.apple.security.virtualization entitlement (handled by build-lns).
# On Linux the Cloud Hypervisor backend needs no entitlement but does
# need /dev/kvm plus cloud-hypervisor + virtiofsd reachable; the
# preflight-microvm prerequisite checks that before the slow build.
# Not part of the push gate; CI runs it nightly on a KVM x86_64 runner
# (.github/workflows/e2e-microvm.yml).
e2e-microvm: preflight-microvm build
	$(CARGO) test -p e2e-tests --test specutil_timeout
	@LNS_E2E_MICROVM=1 \
		LNS_BIN=$(CARGO_TARGET_DIR)/release/lns \
		LNS_SERVICE_BIN=$(CARGO_TARGET_DIR)/release/lns-service \
		$(CARGO) test -p e2e-tests --test e2e

preflight-microvm:
	@case "$$(uname -s)" in \
		Linux) ./scripts/preflight-microvm.sh ;; \
		Darwin) : ;; \
		*) echo "e2e-microvm: unsupported host $$(uname -s) (need macOS Vz or Linux KVM)" >&2; exit 1 ;; \
	esac

# ── Security advisories ───────────────────────────────────────────────

# Fails on any RUSTSEC advisory against the locked dependency graph.
audit:
	@command -v cargo-audit >/dev/null 2>&1 || { \
		echo "cargo-audit not installed. Install with: cargo install cargo-audit"; \
		exit 1; \
	}
	$(CARGO) audit

# ── Housekeeping ──────────────────────────────────────────────────────

clean:
	rm -rf bin/
	$(CARGO) clean
	rm -rf $(COVERAGE_TARGET_DIR)

# One-time setup per checkout: point git at the in-tree hooks dir so
# pre-push runs the gate automatically.
install-hooks:
	git config core.hooksPath scripts/hooks
	@echo "Installed git hooks from scripts/hooks (pre-push: lint + complexity + coverage)."
	@echo "Bypass when needed: git push --no-verify"
