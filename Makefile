.PHONY: dev build build-lns build-lns-service test lint fmt complexity complexity-all clean coverage coverage-data coverage-affected coverage-lcov e2e e2e-microvm preflight-microvm audit install-hooks gate-report parity shell-tests \
	lint-impl test-impl complexity-impl coverage-impl coverage-data-impl parity-impl coverage-affected-impl

CARGO ?= cargo

# `CARGO_LOCKED=--locked make <step>` from CI; empty locally.
CARGO_LOCKED ?=

WORKSPACE_ROOT := $(shell $(CARGO) metadata --format-version 1 --no-deps | \
	(jq -r .workspace_root 2>/dev/null || \
	 sed -n 's/.*"workspace_root":"\([^"]*\)".*/\1/p'))
WORKSPACE_MANIFEST := $(WORKSPACE_ROOT)/Cargo.toml
CARGO_TARGET_DIR := $(shell $(CARGO) metadata --format-version 1 --no-deps | \
	(jq -r .target_directory 2>/dev/null || \
	 sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'))

# Coverage isolates instrumented builds from `make test`'s artifacts:
# cargo-llvm-cov injects -Cinstrument-coverage via RUSTC_WRAPPER which
# cargo's fingerprint doesn't track — sharing the artifact dir silently
# reuses non-instrumented builds, emits no profraw, and the report step
# fails with "not found *.profraw files". Nesting inside target/
# (cargo-llvm-cov's own default layout) keeps the isolation while
# letting `cargo clean` sweep it. Derived from WORKSPACE_ROOT, not
# CARGO_TARGET_DIR — the coverage recipes' target-specific
# `export CARGO_TARGET_DIR` would shadow it into double-nesting.
COVERAGE_TARGET_DIR := $(WORKSPACE_ROOT)/target/llvm-cov-target

# `complexity` runs per-crate clippy with different trailing args than the
# workspace-wide `lint`. Cargo fingerprints those args, so sharing a target
# dir makes each step re-check what the other just built. Its own dir ends
# the thrash; nesting inside target/ keeps `cargo clean` sweeping it.
COMPLEXITY_TARGET_DIR := $(WORKSPACE_ROOT)/target/complexity

# Gate bookkeeping that must outlive `cargo clean` and stay out of the target
# dirs CI caches: rust-cache strips loose files from a cached target root.
GATE_STATE_DIR := $(WORKSPACE_ROOT)/.gate

# Every gate step records itself, whoever ran it — a terminal, an agent, the
# pre-push hook, CI. Each public target is a timed wrapper around its own
# `-impl`, so no caller has to remember to ask for telemetry.
TIMED := ./scripts/gate-timing.sh run

# Crates whose code is enforced by the per-crate `complexity` gate,
# which iterates this list with `cd crates/<crate> && cargo …` so each
# invocation runs with that crate's own feature set instead of the
# workspace-unified one (see the comment on the `complexity` target
# below for why that matters). Decoupled from the shipping-artifact
# list above (`build-lns` / `build-lns-service`) — a crate can be in
# this gate without producing a binary (e.g. `lns-ipc` is a pure
# library).
GATE_CRATES := lns-cli lns-service lns-ipc lns-audit lns-ocsf lns-policy lns-artifact lns-placement lns-spec

# Every crate under crates/ EXCEPT e2e-tests. Used by `make coverage`
# iteration. e2e-tests is excluded because Layer 1 tests don't
# contribute to the coverage gate (see CLAUDE.md "Test layers").
ALL_CRATES := $(filter-out crates/e2e-tests,$(patsubst %/,%,$(wildcard crates/*/)))

# When COVERAGE_CRATES is set (space- or newline-separated crate names),
# the coverage recipe narrows to just those crates. Unset = all crates.
COVERAGE_CRATES_LIST := $(if $(strip $(COVERAGE_CRATES)),$(foreach c,$(COVERAGE_CRATES),crates/$(c)),$(ALL_CRATES))
COVERAGE_CARGO_SCOPE := $(if $(strip $(COVERAGE_CRATES)),$(foreach c,$(COVERAGE_CRATES),-p $(c)),--workspace --exclude e2e-tests)

# ── Dev loop ──────────────────────────────────────────────────────────

# Inner dev loop: debug build of the two user-facing crates. Debug builds
# skip the aarch64-musl guest cross-builds and the static-nft embed (see
# lns-service/build.rs), which every plain `cargo` invocation and
# rust-analyzer also get — one fingerprint, no rebuild when you switch
# between them. Set LNS_INIT_BIN / LNS_SESSION_BROKER_BIN / LNS_NFT_BIN /
# LNS_SUPERVISOR_BIN to `<path>` at runtime to use a pre-built guest binary.
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

# `complexity` and `coverage-data` each own their target dir, so opting out
# of incremental there only bounds disk. `lint` and `test` share the dir
# with `make dev` and rust-analyzer, and CARGO_INCREMENTAL is part of the
# fingerprint — they keep the default so switching between them is free.
complexity-impl coverage-data-impl: export CARGO_INCREMENTAL := 0

lint:
	@$(TIMED) lint -- $(MAKE) --no-print-directory lint-impl

lint-impl: shell-tests
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets $(CARGO_LOCKED) -- -D warnings -D clippy::undocumented_unsafe_blocks

# `--exclude e2e-tests`: the Layer 1 cucumber harness spawns real
# binaries — owned by `make e2e`, not the fast in-process test gate.
test:
	@$(TIMED) test -- $(MAKE) --no-print-directory test-impl

test-impl:
	$(CARGO) test --workspace --exclude e2e-tests --all-targets $(CARGO_LOCKED)

# Per-crate cargo invocations (not workspace-wide): workspace feature
# unification expands shared deps differently than per-crate clippy,
# and `cognitive_complexity` workspace-wide flags functions that pass
# the per-crate gate. Keep complexity per-crate on both sides for
# parity.
complexity:
	@$(TIMED) complexity -- $(MAKE) --no-print-directory complexity-impl

complexity-impl: export CARGO_TARGET_DIR := $(COMPLEXITY_TARGET_DIR)
complexity-impl:
	@status=0; for crate in $(GATE_CRATES); do \
		(cd crates/$$crate && $(CARGO) clippy --all-targets -- -D clippy::cognitive_complexity) || status=$$?; \
	done; exit $$status

complexity-all: complexity

fmt:
	$(CARGO) fmt --all

# The shell harnesses that cover the gate's own scripts. Cheap enough to sit
# inside `lint`, which is where a contributor already looks for "is it clean".
# Run under both CI and non-CI, because these scripts branch on it and a
# harness that only passes in one environment is the defect this gate exists
# to catch.
shell-tests:
	@status=0; failed=""; \
		for t in scripts/*.test.sh; do \
			echo "── $$t (no CI) ──"; \
			env -u CI "./$$t" || { status=$$?; failed="$$failed $$t(no-CI)"; }; \
			echo "── $$t (CI=1) ──"; \
			CI=1 "./$$t" || { status=$$?; failed="$$failed $$t(CI)"; }; \
		done; \
		[ -z "$$failed" ] || echo "shell-tests failed:$$failed"; \
		exit $$status

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
coverage-data:
	@$(TIMED) coverage-data -- $(MAKE) --no-print-directory coverage-data-impl

coverage-data-impl: export CARGO_TARGET_DIR := $(COVERAGE_TARGET_DIR)
coverage-data-impl:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { \
		echo "cargo-llvm-cov not installed. Install with: cargo install cargo-llvm-cov"; \
		exit 1; \
	}
	# Strip tool built OUTSIDE the cargo-llvm-cov env so it isn't
	# instrumented. Its output (the stripped lcov) is what the gate measures.
	# Debug mode: `syn` (heavy dep) compiles ~3x faster in debug than release;
	# runtime cost over ~100 source files is sub-second either way.
	$(CARGO) build -p coverage-strip-ast
	# A source-only change rebuilds a test binary in place, so clearing the
	# counters is enough. A toolchain or manifest change shifts the artifact
	# hashes instead: the superseded binaries stay behind and `llvm-cov
	# report` still picks them up as objects. The stamp catches that and
	# falls back to the full clean.
	@set -e; \
		stamp="$(GATE_STATE_DIR)/coverage-toolchain-stamp"; \
		inputs_sha=$$( cat Cargo.lock crates/*/Cargo.toml Cargo.toml | (shasum -a 256 2>/dev/null || sha256sum) | cut -d' ' -f1 ); \
		want="$$(rustc -V)|$$($(CARGO) llvm-cov --version)|$$inputs_sha"; \
		if [ "$$(cat "$$stamp" 2>/dev/null)" = "$$want" ]; then \
			./scripts/gate-timing.sh detail coverage-data warm; \
			$(CARGO) llvm-cov clean --profraw-only; \
		else \
			./scripts/gate-timing.sh detail coverage-data cold; \
			echo "coverage: toolchain or manifests changed — full artifact clean"; \
			$(CARGO) llvm-cov clean --workspace; \
			mkdir -p "$(GATE_STATE_DIR)"; \
			printf '%s\n' "$$want" >"$$stamp"; \
		fi
	@set -e; \
		eval "$$($(CARGO) llvm-cov show-env --export-prefix)"; \
		$(if $(strip $(COVERAGE_CRATES)),$(if $(filter lns-service,$(COVERAGE_CRATES)),$(CARGO) build -p lns-service;,),$(CARGO) build -p lns-service;) \
		$(CARGO) test $(COVERAGE_CARGO_SCOPE) --all-targets; \
		mkdir -p $(COVERAGE_TARGET_DIR)/llvm-cov; \
		$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST) --lcov \
			--output-path $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info; \
		$(COVERAGE_TARGET_DIR)/debug/coverage-strip-ast $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info; \
		if [ "$(COVERAGE_HTML)" = "1" ]; then \
			$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST); \
			$(CARGO) llvm-cov report --manifest-path $(WORKSPACE_MANIFEST) --html; \
			echo "HTML report: $(COVERAGE_TARGET_DIR)/llvm-cov/html/index.html"; \
		fi

# Runs the binaries `coverage-data` just built, so it costs no compilation.
parity:
	@$(TIMED) parity -- $(MAKE) --no-print-directory parity-impl

parity-impl: export CARGO_TARGET_DIR := $(COVERAGE_TARGET_DIR)
parity-impl:
	@set -e; \
		eval "$$($(CARGO) llvm-cov show-env --export-prefix)"; \
		bins=$$($(CARGO) test $(COVERAGE_CARGO_SCOPE) --all-targets --no-run --message-format=json 2>/dev/null | \
			(jq -r 'select(.profile.test == true) | .executable' 2>/dev/null || \
			 sed -n 's/.*"test":true[^{]*"executable":"\([^"]*\)".*/\1/p') | \
			grep -v '^null$$' | sort -u); \
		./scripts/env-parity.sh $$bins

coverage:
	@$(TIMED) coverage -- $(MAKE) --no-print-directory coverage-impl

# Its duration covers coverage-data and parity, which record their own rows.
coverage-impl: coverage-data
	@$(MAKE) parity
	@status=0; \
		for pkg in $(COVERAGE_CRATES_LIST); do \
			echo ""; \
			echo "── $$pkg ──"; \
			./scripts/coverage-floor.sh $(COVERAGE_TARGET_DIR)/llvm-cov/lcov.info "$$pkg/" || status=$$?; \
		done; \
		exit $$status

BASE_REF ?= origin/main
coverage-affected:
	@$(TIMED) coverage-affected -- $(MAKE) --no-print-directory coverage-affected-impl

coverage-affected-impl:
	@out=$$(./scripts/affected-crates.sh $(BASE_REF)); \
		./scripts/gate-timing.sh note coverage-scope "$$(echo "$$out" | tr '\n' ' ' | sed 's/ *$$//')"; \
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
e2e:
	$(CARGO) build -p lns-cli -p lns-service
	$(CARGO) test -p e2e-tests --test specutil_env
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
	$(CARGO) test -p e2e-tests --test specutil_env
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

# ── Gate telemetry ────────────────────────────────────────────────────
# scripts/gate-timing.sh records one row per gate step (pre-push wraps
# each `make` call) plus the affected-crates verdict. The log lives in the
# shared git dir so worktrees pool one history; LNS_GATE_TIMING=0 stops it.

gate-report:
	@./scripts/gate-timing.sh report

# ── Housekeeping ──────────────────────────────────────────────────────

clean:
	rm -rf bin/
	$(CARGO) clean

# One-time setup per checkout: point git at the in-tree hooks dir so
# pre-push runs the gate automatically.
install-hooks:
	git config core.hooksPath scripts/hooks
	@echo "Installed git hooks from scripts/hooks:"
	@echo "  commit-msg  conventional-commit check (commitlint)"
	@echo "  pre-commit  cargo fmt --check, and markdownlint"
	@echo "  pre-push    lint + complexity + coverage-affected"
	@if ! sh -c '. scripts/hooks/lib.sh; [ -n "$$(node_bin commitlint)" ] && [ -n "$$(node_bin markdownlint-cli2)" ]' \
		|| ! command -v node >/dev/null 2>&1; then \
		echo "  note: commitlint and markdownlint are skipped until \`npm install\` and a reachable node."; \
	fi
	@echo "Bypass when needed: git push --no-verify"
