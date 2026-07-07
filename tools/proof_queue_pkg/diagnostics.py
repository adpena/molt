"""Proof-run diagnostics engine (moved from tools/proof_queue.py).

The diagnostics dispatcher is decomposed out of the proof_queue facade to
keep each module below the structural-audit kitchen-sink ceiling. Shared
helpers, constants, and monkeypatch-sensitive primitives remain in the
facade module ``pq``; this module reaches them through ``pq.`` so test
monkeypatches on tools.proof_queue stay authoritative.
"""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path

from tools.proof_queue_pkg import pq


def _run_diagnostics(row: sqlite3.Row) -> list[dict[str, object]]:
    log_tail = pq._read_log_tail(Path(row["log_path"]))
    diagnostics: list[dict[str, object]] = []
    running_pytest_failures = pq._running_pytest_failures_observed_diagnostic(row)
    if running_pytest_failures is not None:
        diagnostics.append(running_pytest_failures)
    running_pytest_missing = pq._running_pytest_current_test_missing_diagnostic(row)
    if running_pytest_missing is not None:
        diagnostics.append(running_pytest_missing)
    running_child_missing = pq._running_child_missing_diagnostic(row)
    if running_child_missing is not None:
        diagnostics.append(running_child_missing)
    if row["status"] == "blocked":
        diagnostics.append(
            pq._diagnostic(
                signal_id="proof-dependency-blocked",
                severity="operator",
                summary="The proof did not run because a dependency edge did not pass.",
                evidence=(
                    pq._first_log_line_containing(
                        log_tail, "proof queue blocked by dependency"
                    )
                    or f"log_path={row['log_path']}"
                ),
                next_action=(
                    "Inspect the run DAG parents in evidence/status, fix or supersede "
                    "the failed dependency, then queue a new rerun edge."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )
        return diagnostics
    if not log_tail and row["status"] not in {"passed", "queued", "running"}:
        return [
            pq._diagnostic(
                signal_id="proof-log-missing",
                severity="infra",
                summary="The proof row is terminal but its queue log is missing.",
                evidence=f"log_path={row['log_path']}",
                next_action=(
                    "Treat this as incomplete evidence; inspect the queue DB and "
                    "rerun through the same queue lane after preserving the row id."
                ),
            )
        ]

    if (
        "proof queue refuses raw `cargo` commands" in log_tail
        or "proof queue refuses `uv run` commands" in log_tail
    ):
        diagnostics.append(
            pq._diagnostic(
                signal_id="queue-policy-rejection",
                severity="operator",
                summary="The queue rejected a noncanonical command before proof execution.",
                evidence=pq._last_nonempty_log_line(Path(row["log_path"])) or "",
                next_action=(
                    "Resubmit through the queue-native cargo lane or the active "
                    "uv contract; this row is DX policy evidence, not product proof."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    match = pq.QUEUE_COLD_SINGLE_CARGO_PROOF_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="queue-cold-single-cargo-proof",
                severity="operator",
                summary=(
                    "The queue rejected a cold-prone single-test Cargo proof "
                    f"for filter {match.group('filter')}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Batch the relevant crate shard in one compile, warm the "
                    "target dir first, or resubmit with --allow-warm-single-test "
                    "only after recording warm-target evidence in the queue note."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    fatal_queue_failure = (
        "proof queue fatal infrastructure failure" in log_tail
        or "proof queue failed before command execution" in log_tail
    )
    if fatal_queue_failure:
        diagnostics.append(
            pq._diagnostic(
                signal_id="queue-preexecution-failure",
                severity="infra",
                summary=(
                    "The queue hit a fatal infrastructure failure before "
                    "launching the proof command, but the row was made terminal "
                    "and logged."
                ),
                evidence=(
                    pq._first_log_line_containing(
                        log_tail, "proof queue fatal infrastructure failure"
                    )
                    or pq._first_log_line_containing(
                        log_tail, "proof queue failed before command execution"
                    )
                    or pq._last_nonempty_log_line(Path(row["log_path"]))
                    or ""
                ),
                next_action=(
                    "Fix the queue custody bug, then resubmit or run the same "
                    "queued lane; do not treat this row as product proof."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )

    if (
        not fatal_queue_failure
        and "proof queue nonfatal infrastructure failure" in log_tail
    ):
        diagnostics.append(
            pq._diagnostic(
                signal_id="queue-infra-warning",
                severity="infra",
                summary=(
                    "The proof command ran, but queue-side observability had a "
                    "nonfatal infrastructure failure."
                ),
                evidence=(
                    pq._first_log_line_containing(
                        log_tail, "proof queue nonfatal infrastructure failure"
                    )
                    or pq._last_nonempty_log_line(Path(row["log_path"]))
                    or ""
                ),
                next_action=(
                    "Preserve the proof result, then fix the queue projection or "
                    "note append issue before it becomes hidden collaboration debt."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    rust_test_result = pq.RUST_TEST_RESULT_FAILED_RE.search(log_tail)
    rust_cargo_test_failed = pq.RUST_CARGO_TEST_FAILED_RE.search(log_tail)
    if rust_test_result is not None or rust_cargo_test_failed is not None:
        failed_tests = tuple(
            dict.fromkeys(
                match.group("name")
                for match in pq.RUST_FAILED_TEST_LINE_RE.finditer(log_tail)
            )
        )
        evidence_parts: list[str] = []
        if rust_test_result is not None:
            evidence_parts.append(rust_test_result.group(0))
        if rust_cargo_test_failed is not None:
            evidence_parts.append(rust_cargo_test_failed.group(0))
        if failed_tests:
            listed = ", ".join(failed_tests[:5])
            if len(failed_tests) > 5:
                listed += f", ... (+{len(failed_tests) - 5} more)"
            evidence_parts.append(f"failed_tests={listed}")
        diagnostics.append(
            pq._diagnostic(
                signal_id="rust-test-failure",
                severity="error",
                summary=(
                    "Rust proof compiled and reached test execution, but "
                    f"cargo test reported {len(failed_tests) or 'failed'} "
                    "test failure(s)."
                ),
                evidence=" ".join(evidence_parts),
                next_action=(
                    "Fix the failing Rust test or the product contract it protects, "
                    "then rerun the same queue lane. This row reached test "
                    "execution; do not classify it as a compiler failure."
                ),
                scopes=("runtime/", "tools/proof_queue.py"),
            )
        )

    match = pq.RUST_COMPILER_ERROR_RE.search(log_tail)
    if (
        match is not None
        and rust_test_result is None
        and rust_cargo_test_failed is None
    ):
        code = match.group("code") or "rustc"
        message = match.group("message").strip()
        diagnostics.append(
            pq._diagnostic(
                signal_id="rust-compiler-error",
                severity="error",
                summary=f"Rust proof failed during compilation at {code}: {message}.",
                evidence=match.group(0),
                next_action=(
                    "Fix the Rust compiler error before rerunning the proof; this "
                    "row did not reach the intended runtime assertion."
                ),
                scopes=("runtime/", "tools/proof_queue.py"),
            )
        )

    match = pq.RUNTIME_WASM_RUST_TARGET_MISSING_RE.search(log_tail)
    if match is not None:
        target = match.group("target")
        diagnostics.append(
            pq._diagnostic(
                signal_id="runtime-wasm-rust-target-missing",
                severity="infra",
                summary=(
                    "Runtime WASM build reached execution without Rust target "
                    f"{target} available."
                ),
                evidence=match.group(0),
                next_action=(
                    "Install the checked-in Rust toolchain target, then rerun "
                    "through the wasm proof-queue resource family. If a queued "
                    "wasm row reaches this after preflight, fix the queue "
                    "toolchain preflight or resource-family classification."
                ),
                scopes=(
                    "rust-toolchain.toml",
                    "src/molt/cli/wasm_toolchain.py",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = pq.WASM_TOOLCHAIN_CONTRACT_IMPORT_MISSING_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        diagnostics.append(
            pq._diagnostic(
                signal_id="wasm-toolchain-contract-import-missing",
                severity="infra",
                summary=(
                    "WASM proof preflight could not import the toolchain "
                    f"contract because Python module {module} is missing."
                ),
                evidence=match.group(0),
                next_action=(
                    "Repair active uv/project provisioning before resubmitting "
                    "the WASM row; this failed before the proof command ran, so "
                    "do not treat it as product evidence or rerun a heavy build "
                    "unchanged."
                ),
                scopes=(
                    "tools/proof_queue.py",
                    "src/molt/cli/wasm_toolchain.py",
                    "pyproject.toml",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = pq.STATIC_PYMOD_EXEC_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        detail = match.group("detail").strip(" ;")
        artifacts = tuple(
            match.group("path") for match in pq.DIAGNOSTIC_JSON_RE.finditer(log_tail)
        )
        if detail:
            next_action = (
                "Fix the pending Python/C-API error surfaced by module exec, then "
                "rerun the same queue lane as a rerun edge."
            )
        else:
            next_action = (
                "Do not rerun the heavy lane until the module-exec primitive "
                "changes. Inspect the extension's Py_mod_exec body and route the "
                "missing C-API/ABI primitive through shared runtime authority."
            )
        if artifacts:
            next_action += " Start with the diagnostic_json artifact."
        diagnostics.append(
            pq._diagnostic(
                signal_id="static-pymodexec-nonzero",
                severity="error",
                summary=(
                    f"Static-linked extension module {module} reached Py_mod_exec "
                    "and returned non-zero."
                ),
                evidence=match.group(0),
                next_action=next_action,
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "runtime/molt-runtime/src/cpython_abi_hooks.rs",
                    "src/molt/cli/external_native.py",
                ),
                artifacts=artifacts,
            )
        )

    match = pq.RUNTIME_EXPORT_AUTHORITY_UNKNOWN_NAME_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            pq._diagnostic(
                signal_id="wasm-runtime-export-authority-unknown-name",
                severity="error",
                summary=(
                    "A required runtime export obligation is not declared by "
                    f"the generated WASM link authority: {symbol}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Declare the symbol through the generated WASM ABI link "
                    "authority (wasm_abi_manifest/gen_wasm_abi CPython ABI "
                    "surface), not by relaxing the export-name validator or "
                    "hand-editing generated files."
                ),
                scopes=(
                    "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml",
                    "tools/gen_wasm_abi.py",
                    "src/molt/_wasm_runtime_exports.py",
                ),
            )
        )

    match = pq.RUNTIME_WASM_MISSING_EXPORTS_RE.search(log_tail)
    if match is not None:
        symbols = tuple(
            symbol.strip()
            for symbol in match.group("symbols").split(",")
            if symbol.strip()
        )
        listed = ", ".join(symbols[:6])
        if len(symbols) > 6:
            listed += f", ... (+{len(symbols) - 6} more)"
        diagnostics.append(
            pq._diagnostic(
                signal_id="runtime-wasm-missing-required-exports",
                severity="error",
                summary=(
                    "Runtime wasm build cannot satisfy required runtime "
                    f"exports: {listed or 'unlisted symbols'}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Thread the obligations through the shared runtime export "
                    "authority (wasm_runtime_shared_export_link_args plus the "
                    "generated WASM ABI manifest) and keep the defining archive "
                    "retained in the runtime build; do not hand-edit the "
                    "artifact or bypass export validation."
                ),
                scopes=(
                    "src/molt/_wasm_runtime_exports.py",
                    "src/molt/cli/runtime_build.py",
                    "runtime/molt-cpython-abi/build.rs",
                ),
            )
        )

    match = pq.UNDEFINED_SYMBOL_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            pq._diagnostic(
                signal_id="native-undefined-symbol",
                severity="error",
                summary=f"Native/WASM link failed on unresolved symbol {symbol}.",
                evidence=match.group(0),
                next_action=(
                    "Add the symbol to the shared ABI/object-closure authority or "
                    "make package admission fail closed before link; do not patch "
                    "a package-local shim."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "src/molt/cli/external_native.py",
                    "tools/proof_queue.py",
                ),
            )
        )

    match = pq.UNSUPPORTED_DIRECT_CALL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="unsupported-direct-call",
                severity="error",
                summary="The compiler reached an unsupported direct-call boundary.",
                evidence=match.group(0),
                next_action=(
                    "Move the callable into package/import/native symbol closure "
                    "authority or fail closed at admission with this exact callable."
                ),
                scopes=("src/molt/cli/", "runtime/molt-backend-wasm/src/"),
            )
        )

    if "candidate_outputs.npz" in log_tail and any(
        token in log_tail.lower() for token in ("not found", "no such file", "missing")
    ):
        diagnostics.append(
            pq._diagnostic(
                signal_id="pact-candidate-output-missing",
                severity="error",
                summary="Pact acceptance did not produce candidate_outputs.npz.",
                evidence="candidate_outputs.npz was referenced with a missing-file signal",
                next_action=(
                    "Treat this as failed acceptance, not parity evidence. Use the "
                    "named pact-witness-acceptance lane after the structural fix."
                ),
                scopes=("tools/pact_witness_acceptance.py", "collab/pact/"),
            )
        )

    match = pq.PACT_WITNESS_FIXTURE_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="pact-witness-fixture-missing",
                severity="error",
                summary=(
                    "Pact acceptance failed after build/link because the Kernel A "
                    "fixture was not available to the run directory."
                ),
                evidence=match.group(0),
                next_action=(
                    "Make the acceptance runner regenerate the deterministic "
                    "fixture/reference oracle inside the run directory, then "
                    "rerun the named pact-witness-acceptance lane; do not check "
                    "binary fixture outputs into source."
                ),
                scopes=(
                    "tools/pact_witness_acceptance.py",
                    "collab/pact/pact_witness_kernel/make_fixture.py",
                    "collab/pact/pact_witness_kernel/field_solve.py",
                ),
            )
        )

    match = pq.NATIVE_RUNTIME_IMPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        package = match.group("package")
        diagnostics.append(
            pq._diagnostic(
                signal_id="external-native-runtime-import-custody",
                severity="error",
                summary=(
                    f"Sealed external package {package} cannot prove runtime "
                    "Python imports because its manifest is missing "
                    "runtime_python_import_modules."
                ),
                evidence=match.group(0),
                next_action=(
                    "Regenerate and re-seal the source-recompiled extension root "
                    "from live source/build custody so seal persists "
                    "runtime_python_import_modules; do not rerun the heavy "
                    "pact-witness-acceptance lane until package admission passes."
                ),
                scopes=(
                    "src/molt/cli/external_native.py",
                    "src/molt/cli/extension_seal.py",
                    "tools/pact_seal_witness_roots.py",
                ),
            )
        )

    match = pq.NATIVE_ARTIFACT_CUSTODY_RE.search(log_tail)
    if match is not None and not pq._diagnostics_have_signal(
        diagnostics, "external-native-runtime-import-custody"
    ):
        missing_abi_symbols = tuple(
            dict.fromkeys(
                symbol_match.group("symbol")
                for symbol_match in pq.NATIVE_ARTIFACT_ABI_SURFACE_RE.finditer(
                    match.group("detail")
                )
            )
        )
        if missing_abi_symbols:
            listed = ", ".join(missing_abi_symbols[:6])
            if len(missing_abi_symbols) > 6:
                listed += f", ... (+{len(missing_abi_symbols) - 6} more)"
            diagnostics.append(
                pq._diagnostic(
                    signal_id="external-native-abi-link-surface-missing",
                    severity="error",
                    summary=(
                        "External native object closure requires runtime ABI "
                        f"link imports missing from the generated WASM surface: {listed}."
                    ),
                    evidence=match.group(0),
                    next_action=(
                        "Route the missing symbols through the generated WASM ABI "
                        "manifest/link-import authority and link validation; do not "
                        "paper over them with prefix admission or package-local shims."
                    ),
                    scopes=(
                        "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml",
                        "tools/gen_wasm_abi.py",
                        "src/molt/cli/external_native.py",
                        "tests/test_gen_wasm_abi.py",
                        "tests/test_wasm_link_validation.py",
                    ),
                )
            )
        else:
            diagnostics.append(
                pq._diagnostic(
                    signal_id="external-native-artifact-custody",
                    severity="error",
                    summary=(
                        "External native package admission failed because a declared "
                        "callable export is not backed by a native method, direct "
                        "symbol, or sealed provider module."
                    ),
                    evidence=match.group(0),
                    next_action=(
                        "Fix package-native object closure or provider-module custody; "
                        "do not rerun the heavy lane until the manifest/source authority "
                        "can prove the callable without a facade."
                    ),
                    scopes=(
                        "src/molt/cli/external_native.py",
                        "src/molt/cli/source_extensions.py",
                    ),
                )
            )

    match = pq.NATIVE_SUPPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="external-native-support-custody",
                severity="error",
                summary=(
                    "Reachable native package support modules lack source or "
                    "artifact custody."
                ),
                evidence=match.group(0),
                next_action=(
                    "Publish reachable source-recompiled artifacts or sealed "
                    "source-plan custody for these support modules; package "
                    "visibility alone is not execution authority."
                ),
                scopes=(
                    "src/molt/cli/external_native.py",
                    "src/molt/cli/source_extensions.py",
                ),
            )
        )

    match = pq.STDLIB_PROFILE_REFUSAL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="stdlib-profile-refusal",
                severity="error",
                summary=(
                    f"Runtime feature {match.group('feature')} is reachable but "
                    f"excluded by profile {match.group('profile')}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Move the reached feature requirement through canonical "
                    "reachability/profile selection instead of broadening a profile "
                    "or hiding the missing feature in the proof command."
                ),
                scopes=(
                    "src/molt/cli/runtime_features.py",
                    "src/molt/cli/module_stdlib_policy.py",
                ),
            )
        )

    match = pq.MOLT_RUNTIME_INVALID_OBJECT_HEADER_RE.search(log_tail)
    if match is not None:
        detail = match.group("detail").strip()
        site_match = re.search(r"\bin (?P<site>[A-Za-z_][A-Za-z0-9_]*)", detail)
        site = site_match.group("site") if site_match is not None else None
        diagnostics.append(
            pq._diagnostic(
                signal_id="molt-runtime-invalid-object-header",
                severity="error",
                summary=(
                    "Molt runtime aborted on an invalid object header"
                    + (f" in {site}" if site else "")
                    + "."
                ),
                evidence=match.group(0),
                next_action=(
                    "Treat this as runtime object-lifetime corruption, not a "
                    "generic pytest failure. Inspect the owning refcount/borrow "
                    "boundary named by the fatal site and rerun the same queue "
                    "lane only after that ownership bug changes."
                ),
                scopes=(
                    "runtime/molt-runtime/",
                    "runtime/molt-backend-native/src/",
                    "tools/proof_queue.py",
                ),
            )
        )

    match = pq.SOURCE_LEASE_CHANGED_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            pq._diagnostic(
                signal_id="source-lease-changed-during-proof",
                severity="operator",
                summary=(
                    "A source file changed while the compiler was reading it; "
                    "the proof row is contaminated evidence."
                ),
                evidence=match.group(0),
                next_action=(
                    "Do not interpret downstream failures from this row as the "
                    "current product frontier. Let active edits settle, then "
                    "rerun the same queue lane from a stable git snapshot."
                ),
                scopes=(
                    match.group("module").strip(),
                    "tools/proof_queue.py",
                ),
            )
        )

    partial_module_match = pq.PARTIAL_MODULE_PUBLICATION_RE.search(log_tail)
    if partial_module_match is not None:
        diff_fail_match = None
        for candidate in pq.MOLT_DIFF_FAIL_RE.finditer(
            log_tail, 0, partial_module_match.start()
        ):
            diff_fail_match = candidate
        failing_case = (
            diff_fail_match.group("case") if diff_fail_match is not None else None
        )
        summary = (
            "Import failed because module "
            f"{partial_module_match.group('module')} was observed before publication."
        )
        evidence = partial_module_match.group(0)
        scopes = [
            "runtime/molt-runtime/src/builtins/module_table.rs",
            "runtime/molt-runtime/src/builtins/modules.rs",
            "src/molt/cli/backend_ir.py",
        ]
        if failing_case is not None:
            summary = f"{summary} Failing fixture: {failing_case}."
            evidence = f"{diff_fail_match.group(0)}\n{evidence}"
            scopes.insert(0, failing_case)
        diagnostics.append(
            pq._diagnostic(
                signal_id="import-partial-module-publication",
                severity="error",
                summary=summary,
                evidence=evidence,
                next_action=(
                    "Route this to the import/bootstrap module-state owner; do "
                    "not patch the frozen import layer from an unrelated lane."
                ),
                scopes=tuple(scopes),
            )
        )

    diff_fail_match = pq.MOLT_DIFF_FAIL_RE.search(log_tail)
    if (
        diff_fail_match is not None
        and not pq._diagnostics_have_signal(
            diagnostics, "import-partial-module-publication"
        )
        and pq.MEMORY_GUARD_TIMEOUT_RE.search(log_tail) is None
    ):
        failing_case = diff_fail_match.group("case").replace("\\", "/")
        stdout_lines = [
            f"{match.group('label')} stdout={match.group('value')}"
            for match in pq.MOLT_DIFF_STDOUT_LINE_RE.finditer(log_tail)
        ]
        evidence_parts = [diff_fail_match.group(0)]
        if stdout_lines:
            evidence_parts.extend(stdout_lines[:2])
        diagnostics.append(
            pq._diagnostic(
                signal_id="molt-diff-output-mismatch",
                severity="error",
                summary=(
                    "molt_diff found a "
                    f"{diff_fail_match.group('detail')} in "
                    f"{failing_case} "
                    f"on {diff_fail_match.group('target')}."
                ),
                evidence="\n".join(evidence_parts),
                next_action=(
                    "Treat this as the current product frontier. Fix the "
                    "semantic authority named by the fixture, then rerun the "
                    "same queue lane instead of relabeling the row as infra."
                ),
                scopes=(failing_case, "tests/molt_diff.py"),
            )
        )

    match = pq.PYTEST_ERROR_RE.search(log_tail)
    if match is not None:
        exception_line = pq.PYTEST_EXCEPTION_LINE_RE.search(log_tail)
        detail = (
            exception_line.group("error")
            if exception_line is not None
            else (match.group("detail") or match.group(0))
        )
        diagnostics.append(
            pq._diagnostic(
                signal_id="pytest-error",
                severity="error",
                summary=f"Pytest proof errored while running {match.group('nodeid')}.",
                evidence=detail,
                next_action=(
                    "Fix the collection/import/setup error before interpreting "
                    "the proof lane; this row did not reach the protected assertion."
                ),
                scopes=("tests/", "tools/proof_queue.py"),
            )
        )

    match = pq.PYTEST_FAILED_RE.search(log_tail)
    if match is not None:
        nodeid = match.group("nodeid")
        assertion = pq.PYTEST_ASSERTION_RE.search(log_tail)
        detail = assertion.group("error") if assertion is not None else match.group(0)
        if nodeid.startswith(pq.NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX):
            diagnostics.append(
                pq._diagnostic(
                    signal_id="native-call-lane-pytest-failure",
                    severity="error",
                    summary=(
                        "Native call-lane proof failed at "
                        f"{nodeid}; this lane is owned by the R1 integrator."
                    ),
                    evidence=detail,
                    next_action=(
                        "Route this row to the native call-lane owner. Do not patch "
                        "call/function.rs, fc/modules.rs, class_init.rs, containers, "
                        "exceptions, object/mod.rs, or the native import regression "
                        "test from an unrelated Codex lane."
                    ),
                    scopes=pq.NATIVE_CALL_LANE_SCOPES,
                )
            )
        else:
            diagnostics.append(
                pq._diagnostic(
                    signal_id="pytest-failure",
                    severity="error",
                    summary=f"Pytest proof failed at {nodeid}.",
                    evidence=detail,
                    next_action=(
                        "Fix the failing test or the changed contract it protects, "
                        "then rerun the same focused queue lane."
                    ),
                    scopes=("tests/",),
                )
            )

    match = pq.PYTHON_EXCEPTION_RE.search(log_tail)
    if match is not None and not diagnostics:
        diagnostics.append(
            pq._diagnostic(
                signal_id="python-exception",
                severity="error",
                summary=(
                    f"Python proof command raised {match.group('type')}: "
                    f"{match.group('message').strip()}"
                ),
                evidence=match.group(0),
                next_action=(
                    "Inspect the traceback once, then either fix the product "
                    "failure or promote the recurring pattern into a narrower "
                    "queue diagnostic."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )

    match = pq.MEMORY_GUARD_TIMEOUT_RE.search(log_tail)
    if match is not None:
        pytest_context = pq._pytest_timeout_context(row["summary_json"])
        pytest_suffix = ""
        evidence = match.group(0)
        next_action_context = "the last active phase"
        if pytest_context is not None:
            nodeid, phase = pytest_context
            pytest_suffix = f" while pytest was in {nodeid}"
            if phase is not None:
                pytest_suffix += f" ({phase})"
            evidence += f" pytest_nodeid={nodeid}"
            if phase is not None:
                evidence += f" pytest_phase={phase}"
            next_action_context = f"{nodeid}"
        if (
            pytest_context is not None
            and nodeid.startswith(pq.NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX)
        ):
            diagnostics.append(
                pq._diagnostic(
                    signal_id="native-call-lane-memory-guard-timeout",
                    severity="error",
                    summary=(
                        "Native call-lane proof timed out after "
                        f"{match.group('timeout')}s{pytest_suffix}; this lane is "
                        "owned by the R1 integrator."
                    ),
                    evidence=evidence,
                    next_action=(
                        "Route this timeout row to the native call-lane owner. "
                        "Treat it as incomplete evidence and do not rerun the same "
                        "shape unchanged from an unrelated Codex lane."
                    ),
                    scopes=(
                        "tools/memory_guard.py",
                        "tools/proof_queue.py",
                        *pq.NATIVE_CALL_LANE_SCOPES,
                    ),
                    artifacts=(str(row["summary_json"]), str(row["log_path"])),
                )
            )
        else:
            diagnostics.append(
                pq._diagnostic(
                    signal_id="memory-guard-timeout",
                    severity="error",
                    summary=(
                        "Memory guard terminated the proof after "
                        f"{match.group('timeout')}s{pytest_suffix}."
                    ),
                    evidence=evidence,
                    next_action=(
                        "Treat this proof result as incomplete. Inspect "
                        f"{next_action_context} once, then reshape the proof, warm "
                        "the target dir, or raise --timeout only for intentional "
                        "long-running work."
                    ),
                    scopes=(
                        "tools/memory_guard.py",
                        "tools/proof_queue.py",
                    ),
                    artifacts=(str(row["summary_json"]), str(row["log_path"])),
                )
            )

    match = pq.MEMORY_GUARD_ORPHANED_RE.search(log_tail)
    if match is not None:
        quarantine_match = pq.MEMORY_GUARD_CARGO_QUARANTINE_RE.search(log_tail)
        evidence = match.group(0)
        artifacts: tuple[str, ...] = ()
        if quarantine_match is not None:
            receipt = quarantine_match.group("receipt").strip()
            evidence += f" cargo_quarantine_receipt={receipt}"
            artifacts = (receipt,)
        nested_guard = (
            "guarded_exec:" in log_tail
            or "MOLT_TEST_SUITE guarded command" in log_tail
        )
        diagnostics.append(
            pq._diagnostic(
                signal_id=(
                    "nested-memory-guard-orphan-cleanup"
                    if nested_guard
                    else "memory-guard-orphan-cleanup"
                ),
                severity="warning",
                summary=(
                    "Nested guarded_exec memory guard cleaned up orphaned child "
                    "processes after its guarded command exited."
                    if nested_guard
                    else "Memory guard cleaned up orphaned child processes after "
                    "the proof command exited."
                ),
                evidence=evidence,
                next_action=(
                    "Preserve the proof result, then harden the nested guarded "
                    "command lifecycle or move intentional warm daemons inside a "
                    "suite sentinel that drains at scope exit."
                    if nested_guard
                    else "Preserve the proof result, then harden the child process "
                    "lifecycle or run intentional warm daemons inside a suite "
                    "sentinel that drains at scope exit."
                ),
                scopes=(
                    "tools/guarded_exec.py",
                    "tools/memory_guard.py",
                    "tools/proof_queue.py",
                ),
                artifacts=artifacts,
            )
        )

    incomplete_memory_guard = pq._finished_incomplete_memory_guard_diagnostic(row)
    if incomplete_memory_guard is not None:
        diagnostics.insert(0, incomplete_memory_guard)

    if row["status"] == "failed" and not diagnostics:
        last = pq._last_nonempty_log_line(Path(row["log_path"])) or ""
        diagnostics.append(
            pq._diagnostic(
                signal_id="unclassified-failed-proof",
                severity="unknown",
                summary="The proof failed without a recognized queue diagnostic.",
                evidence=last,
                next_action=(
                    "Inspect the log tail once, then add a deterministic diagnosis "
                    "rule before this failure pattern becomes tribal knowledge."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )
    return diagnostics
