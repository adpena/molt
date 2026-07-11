# Substrate Health Audit

Date: 2026-07-11
Scope: test truth, Windows behavior, build/proof orchestration, and durable artifact custody.
Method: live `origin/main` inspection, `cargo metadata --no-deps --format-version 1`, CI/dev-gate command tracing, focused source review, and primary Cargo/Python/Windows documentation.

## Executive map

The dominant substrate defect is **split authority over whether a test exists and whether it ran**. Cargo owns target discovery, but CI historically hand-enumerated a few packages/binaries and used Cargo's default executable-level fail-fast behavior. That combination made three illegal states representable: a target could compile but never execute, an early binary could hide later failures, and local/CI claims could describe different test surfaces. The structural fix is one Cargo-owned default-feature workspace execution (`cargo test --workspace --tests --no-fail-fast`) plus a static gate that rejects topology drift. Feature-specific lanes remain additive only where target features materially change the program.

The Windows defects share a second crux: **ambient process and filesystem state is treated as implicit authority**. Current working directories, platform encodings, ignored worktree-local artifacts, archive suffixes, and repo-path process matches can all silently change behavior. Each must become an explicit, attested input or be rejected.

## Findings

### S1. Cross-binary `test_sequences` access violation

- **Symptom/evidence:** `runtime/molt-cpython-abi/tests/test_sequences.rs` intermittently exits with Windows `STATUS_ACCESS_VIOLATION` in broad crate runs; `docs/agent/MIRI_STRICT_PROVENANCE.md` records the failure as outside the strict-provenance result rather than resolved. The same binary is stable often enough in isolation to poison trust rather than fail deterministically.
- **Crux:** confirmed by the concurrent `CPYTHON-ABI-TEST-ISOLATION` landing: Cargo libtest ran sibling tests concurrently inside each integration-test process, but the emulated C-extension ABI intentionally assumes GIL-serialized execution over one `GLOBAL_BRIDGE`. Small-int proxies were deduplicated by value while non-atomic `ob_refcnt` mutations occurred outside the bridge lock, so sibling tests could race a shared proxy to zero and evict a live handle. Other binaries delta-asserted shared call counters, and the allocation-budget test observed a process-global allocator counter polluted by sibling threads.
- **Class vs instance:** class defect in test isolation around process-global ABI state, not a product-runtime UAF. `test_sequences` was one victim; slice, object-protocol, module, type-ready, and allocation-budget tests exposed the same mismatch.
- **Structural fix:** completed by the reserved owner: affected integration binaries now hold a poison-tolerant file-wide test lock for each test body, restoring the production GIL-serialized invariant, and the allocation counter is thread-local. Product assertions were not weakened. Harness-side `--no-fail-fast` remains necessary so any future crash cannot hide later reds.
- **Status:** killed on the live board with full `cargo test -p molt-lang-cpython-abi --no-fail-fast` 90/90 green plus repeated targeted stress loops (`test_slice` 800, `test_modules` 1700, `test_type_ready_inheritance` 400, `test_object_protocol` 300, allocation budget 150).

### S2. Cargo executable fail-fast masks later binaries

- **Symptom/evidence:** `.github/workflows/ci.yml` previously ran broad package commands without `--no-fail-fast`; Cargo exits after the first failing executable, while tests inside that executable continue. The explicit `debug_call_bind` step documented that an earlier binary had masked `trace_function_bind_meta_emits_summary`.
- **Crux:** Molt relied on Cargo's default orchestration semantics while interpreting the result as a complete suite verdict.
- **Class vs instance:** class defect across every multi-executable `cargo test` command.
- **Structural fix:** all multi-executable commands use `--no-fail-fast`; `tools/check_cargo_test_truth.py` rejects recurrence in CI and `tools/molt_dev_gates.toml`.
- **Status:** fixed in this lane.

### S3. Compiled-but-unexecuted Rust test targets

- **Symptom/evidence:** `cargo metadata` reports 116 workspace integration-test targets on this checkout. CI manually selected a small subset, and `cargo clippy --workspace --all-targets` compiled many targets without executing them. `debug_call_bind` required an ad hoc explicit step at former `.github/workflows/ci.yml:254`.
- **Crux:** target discovery authority lived in Cargo while execution authority lived in hand-maintained YAML.
- **Class vs instance:** class defect; any newly added `tests/*.rs` target could silently become compile-only.
- **Structural fix:** CI executes `cargo test --workspace --tests --no-fail-fast`, so Cargo target discovery is execution discovery. The truth gate requires exactly one canonical workspace execution.
- **Status:** fixed in this lane for default features. Feature-gated targets remain owned by explicit feature lanes.

### S4. Known-red normalization and baseline accumulation

- **Symptom/evidence:** comments and claims normalize `test_sequences`, `pyset_ops_fail_closed_on_non_set`, LLVM partition failures, and target-specific libc failures as unrelated/pre-existing. `tools/check_suite_honesty.py` covers tracked differential failures, but Rust/target/build failures have no single owner/expiry registry.
- **Crux:** exception policy is fragmented across prose, claims, CI comments, and lane knowledge; no machine authority distinguishes a registered failure from a new failure.
- **Class vs instance:** class defect across Rust, differential, WASM, and build lanes.
- **Structural fix:** drive deterministic reds to zero. For genuinely external blockers, extend one suite-honesty registry with exact test identity, platform/target predicate, owner, evidence, introduced SHA, and expiry; reject missing/expired entries and unexpected passes. Do not create a second Rust-only registry.
- **Status:** follow-on; registry unification is larger than the fail-fast landing and must coordinate with suite-honesty ownership.

### W1. Windows pruned-worktree directory retention

- **Symptom/evidence:** `tools/drift_harvest.py` removes/prunes worktrees while agents commonly remain inside the target directory. Windows documents that the current directory cannot be removed; long paths add a second failure mode.
- **Crux:** cleanup is invoked from a process whose ambient CWD can be inside the object being deleted.
- **Class vs instance:** class defect for any self-deleting worktree/session directory.
- **Structural fix:** deletion APIs must reject a target containing either PowerShell's location or the process current directory, emit the required outside-CWD command, and record deferred cleanup. The orchestrator must `chdir C:\Molt\molt-src` before prune. Long-path cleanup must use the exact resolved target and `\\?\` form, then `git worktree prune`.
- **Status:** follow-on in drift-harvest; operator workflow requirement remains mandatory meanwhile.

### W2. cp1252/default-encoding crashes

- **Symptom/evidence:** `tools/encoding_gate.py` documents the successful-build-then-`UnicodeEncodeError` incident. The gate scans first-party `tools/**/*.py` and `src/molt/**/*.py`; `tools/_io_utf8.py` provides the stdio backstop.
- **Crux:** ambient locale was an undeclared serialization format for files, subprocess text, and console output.
- **Class vs instance:** class defect, substantially addressed but not fully closed: tests, scripts outside the two scan roots, YAML shell snippets, Rust child decoding, and non-Python launchers remain outside the AST gate.
- **Structural fix:** keep explicit UTF-8 at serialization boundaries; expand registration to all first-party Python roots rather than another regex gate; launch Python with UTF-8 mode in CI/queue entrypoints; preserve explicit non-UTF-8 only in codec tests.
- **Status:** existing fix has teeth but scope is incomplete.

### W3. Duplicate `.lib` and `.a` archive custody

- **Symptom/evidence:** Windows native linking can admit both MSVC `.lib` and GNU-style `lib*.a` representations of one shim, producing duplicate symbols. The pyarg instance is owned by `E1-SHIM-EXPORT-CUSTODY`.
- **Crux:** archive discovery treats filename suffixes as independent candidates rather than alternate representations of one logical library identity.
- **Class vs instance:** class defect across every native extension/shim archive.
- **Structural fix:** one target-aware archive resolver canonicalizes logical library identity, selects the platform-native representation, rejects multiple providers, and emits an attested selected-provider list. Link assembly must consume only that resolver output.
- **Status:** general gate follow-on; pyarg implementation remains active-lane owned.

### W4. Path, junction, case, and length ambiguity

- **Symptom/evidence:** seal/toolchain custody uses junctions and resolved paths; code mixes `Path.resolve()`, string comparisons, slash normalization, and platform-specific deletion. NTFS is normally case-insensitive, reparse points have distinct deletion semantics, and long paths can exceed legacy APIs.
- **Crux:** path strings are used as identity without a single Windows canonicalization primitive that preserves reparse-point intent.
- **Class vs instance:** class defect in seal custody, worktree cleanup, artifact poison checks, and module-root validation.
- **Structural fix:** one path-identity utility returns normalized absolute path, comparison key, reparse-point kind, and containment result; destructive operations must validate the resolved target remains under an allowed root without traversing an unintended junction.
- **Status:** follow-on; requires a call-site inventory and migration, not a local shim.

### W5. Orphan cleanup and process-custody ambiguity

- **Symptom/evidence:** `tools/harness_memory_guard.py` contains stale-orphan cleanup and repo-scoped diagnostics; prior incidents included `tail.exe`. Existing sentinel work removed several repo-path/parent-chain kill assumptions, but cleanup policy remains distributed across guard, sentinel, queue, and daemon custody.
- **Crux:** process identity and cleanup authority are represented by overlapping heuristics rather than one unforgeable custody record.
- **Class vs instance:** class defect; `tail.exe` is one leaked child.
- **Structural fix:** only launchers create custody tokens containing owner PID/start time/job/process-group identity; only the token owner can reap descendants. Repo path and ancestry remain diagnostics, never authority. All reapers consume the same custody API.
- **Status:** partially fixed; complete unification remains.

### H1. Shared-checkout stale HEAD

- **Symptom/evidence:** commands run from `C:\Molt\molt-src` can test the checkout's current branch/session base rather than latest `origin/main`; docs already require fetch/rebase before arcs.
- **Crux:** verification provenance is implicit in CWD instead of recorded as an input and result.
- **Class vs instance:** class defect across audits, gates, and proof claims.
- **Structural fix:** proof records capture HEAD, dirty digest, merge-base, and origin/main SHA; release/shared claims fail closed unless HEAD equals the requested provenance. Fresh worktrees are for isolation, not a substitute for provenance checks.
- **Status:** proof queue records snapshots, but direct gates still need a common provenance preflight.

### H2. Worktree-local ignored artifact drift

- **Symptom/evidence:** NumPy seal version and long-double archive regressions demonstrated that gitignored `tmp/`, sysroot, and per-worktree artifacts can satisfy one lane and disappear in another. Search surfaces additional package seals, generated headers, native archives, and toolchain probes with similar risk.
- **Crux:** derived artifacts are addressed by location/session rather than content/version/target identity and effectiveness attestation.
- **Class vs instance:** class defect; NumPy and long-double are instances.
- **Structural fix:** a shared versioned artifact store keyed by source digest, target, toolchain, build recipe, and ABI; consumers verify an attestation and proof-of-effect before use. Worktrees contain references, never unique durable state.
- **Status:** NumPy pattern landed; generalization remains for SciPy and native/sysroot archives.

### H3. Gate sprawl and overlapping authority

- **Symptom/evidence:** CI directly invokes numerous independent gates while `tools/dev.py`, `tools/molt_dev_gates.toml`, hooks, and proof-queue lanes maintain overlapping command lists. Some gates are ratchets, some generators, some execution lanes, and some wrappers.
- **Crux:** there is no typed gate manifest defining owner, inputs, platform, cost, freshness, and CI/dev/proof placement.
- **Class vs instance:** class defect; each duplicated command is an instance.
- **Structural fix:** make `molt_dev_gates.toml` (or a replacement typed manifest) the single gate graph; CI selects named tiers from it. Gate-liveness validates every registered gate is reachable and every CI gate is registered. Wrappers only execute graph nodes.
- **Status:** follow-on. The Cargo truth gate is intentionally one new invariant until it can be registered in that spine.

### H4. Test selection comments become permanent exceptions

- **Symptom/evidence:** the LLVM CI lane scopes to one module because another module has a pre-existing red; runtime previously named one binary because broad fail-fast could mask it.
- **Crux:** selectors are used to route around failures, and prose becomes the exception registry.
- **Class vs instance:** class defect.
- **Structural fix:** selectors may reduce cost only after the complete lane runs elsewhere. Any exclusion must be represented in the canonical suite-honesty authority with owner and expiry.
- **Status:** unresolved; workspace execution removes the default-feature hole, LLVM feature exceptions remain.

### H5. `--lib` is overloaded as both speed optimization and truth claim

- **Symptom/evidence:** CI/dev gates use `--lib` for speed, while integration binaries carry important contracts. Clippy `--all-targets` then creates a misleading sense that the target was covered.
- **Crux:** compile coverage, unit execution, integration execution, and feature coverage are not separately named artifacts.
- **Class vs instance:** class defect.
- **Structural fix:** report four explicit dimensions per crate/feature set: compiled targets, executed targets, skipped targets with predicates, and failures. Never call a `--lib` result a crate-suite result.
- **Status:** default CI truth fixed by workspace execution; reporting remains a follow-on.

### H6. Queue/direct-command semantic drift

- **Symptom/evidence:** queue-native Cargo owns guards, contention, logs, snapshots, and timeouts, while CI/direct dev commands invoke `guarded_exec` manually. Equivalent-looking commands therefore produce different custody/evidence.
- **Crux:** execution policy is bound to entrypoints rather than a shared command specification.
- **Class vs instance:** class defect.
- **Structural fix:** named gate nodes compile to queue, CI, or local executors while preserving command, environment, provenance, and result schema. Expensive local runs remain queue-only.
- **Status:** follow-on under gate-spine unification.

### H7. Cross-backend enum additions can leave a compiled consumer unproven

- **Symptom/evidence:** rebased proof queue run `20260711T120609-substrate-workspace-test-truth-rebased-4ce806ae64e744a7` failed compiling `runtime/molt-backend-native/src/native_backend/simple_backend/trampolines.rs:40`: `TrampolineKind::CallFrame` is not covered. The variant landed for reserved C-extension call frames, but the native simple-backend consumer was not updated. Narrow existing CI lanes did not compile this default workspace consumer combination.
- **Crux:** closed IR/ABI enum authority and backend exhaustiveness are not landed atomically or generated/gated across every consumer.
- **Class vs instance:** class defect. `CallFrame` is the instance; any new closed-domain variant can strand a backend that is absent from the originating lane's focused checks.
- **Structural fix:** the enum authority change must carry a generated consumer matrix or a workspace compile gate that enumerates every backend consumer. Match sites must remain exhaustive; do not add wildcard fallbacks. The active `molt-backend-native` orchestrator subagent owns the exact match repair.
- **Status:** newly exposed, not masked. This lane does not edit the explicitly reserved native-backend file.

## Priority

1. **Land the Cargo test truth spine** from this lane; it prevents fail-fast and silent-target masking now.
2. **Repair the native `CallFrame` exhaustive consumer** in the active owner lane, then rerun the workspace truth proof.
3. **Unify known-red policy** into suite honesty with owners and expiry, then burn the registry to zero.
4. **Canonicalize artifact custody** for all package seals/sysroots/native archives.
5. **Unify process custody and Windows path identity** before enabling any automatic orphan cleanup.
6. **Collapse gate topology** into one typed graph with consistent local/CI/queue executors.

## Primary-source checks

- Cargo Book, `cargo test`: `--no-fail-fast` runs all test executables despite failures; without it Cargo stops after the first failing executable.
- Cargo Book, targets: each integration test is a separate executable and Cargo owns target discovery.
- Microsoft `rmdir`: Windows cannot remove the current directory; the process must change outside it first.
- Python Windows documentation: UTF-8 mode is enabled by `-X utf8` or `PYTHONUTF8=1`; otherwise locale/default encoding can remain the ANSI code page.
