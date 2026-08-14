"""Static-link, WASM ABI, native custody, and profile refusal rules."""

from __future__ import annotations

import re
import sqlite3

from tools.proof_queue_pkg.diagnostic_model import (
    _diagnostic,
    _diagnostics_have_signal,
)

STATIC_PYMOD_EXEC_RE = re.compile(
    r"(?:ImportError:\s+|Original error was:\s*)"
    r"(?P<module>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r": static-link PyModuleDef Py_mod_exec slot returned non-zero"
    r"(?P<detail>[^\r\n]*)"
)

UNDEFINED_SYMBOL_RE = re.compile(
    r"(?:wasm-ld: error: .*?undefined symbol:|undefined symbol:)\s+"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_@.$]*)"
)

RUNTIME_WASM_MISSING_EXPORTS_RE = re.compile(
    r"Runtime wasm (?:build produced artifact|artifact) missing required "
    r"exports[:;]?\s*(?P<symbols>[^\r\n]*)"
)

RUNTIME_EXPORT_AUTHORITY_UNKNOWN_NAME_RE = re.compile(
    r"ValueError: unknown WASM runtime import/export name: "
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_@.$]*)"
)

UNSUPPORTED_DIRECT_CALL_RE = re.compile(
    r"(?is)(?:unsupported|not supported|not linkable).*?"
    r"(?:direct call|direct-call).*?"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_.]*)"
)

DIAGNOSTIC_JSON_RE = re.compile(r"diagnostic_json=(?P<path>\S+)")

PACT_WITNESS_FIXTURE_MISSING_RE = re.compile(
    r"missing Pact fixture:\s+(?P<path>[^\r\n]+)"
)

NATIVE_ARTIFACT_CUSTODY_RE = re.compile(
    r"External static package native-artifact custody errors:\s+(?P<detail>[^\r\n]+)"
)

NATIVE_RUNTIME_IMPORT_CUSTODY_RE = re.compile(
    r"External static package native-artifact custody errors:\s+"
    r"(?P<package>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r": (?P<detail>sealed extension manifest lacks a "
    r"'runtime_python_import_modules' field[^\r\n]*)"
)

NATIVE_ARTIFACT_ABI_SURFACE_RE = re.compile(
    r"runtime ABI symbol '(?P<symbol>[^']+)' is not in the generated "
    r"WASM ABI/link import surface"
)

NATIVE_SUPPORT_CUSTODY_RE = re.compile(
    r"reachable native support source imports native package modules without source "
    r"or artifact custody:\s+(?P<detail>[^\r\n]+)"
)

STDLIB_PROFILE_REFUSAL_RE = re.compile(
    r"Profile '(?P<profile>[^']+)' excludes the '(?P<feature>[^']+)' "
    r"runtime feature"
)


def _append_diagnostics(
    row: sqlite3.Row,
    log_tail: str,
    diagnostics: list[dict[str, object]],
) -> None:
    match = STATIC_PYMOD_EXEC_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        detail = match.group("detail").strip(" ;")
        artifacts = tuple(
            match.group("path") for match in DIAGNOSTIC_JSON_RE.finditer(log_tail)
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
            _diagnostic(
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

    match = RUNTIME_EXPORT_AUTHORITY_UNKNOWN_NAME_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
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

    match = RUNTIME_WASM_MISSING_EXPORTS_RE.search(log_tail)
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
            _diagnostic(
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
                    "src/molt/cli/runtime_wasm_build.py",
                    "src/molt/cli/runtime_wasm_pair_build.py",
                    "runtime/molt-cpython-abi/build.rs",
                ),
            )
        )

    match = UNDEFINED_SYMBOL_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
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

    match = UNSUPPORTED_DIRECT_CALL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
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
            _diagnostic(
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

    match = PACT_WITNESS_FIXTURE_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
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

    match = NATIVE_RUNTIME_IMPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        package = match.group("package")
        diagnostics.append(
            _diagnostic(
                signal_id="external-native-runtime-import-custody",
                severity="error",
                summary=(
                    f"Sealed external package {package} cannot prove runtime "
                    "Python imports because its manifest is missing "
                    "runtime_python_import_modules."
                ),
                evidence=match.group(0),
                next_action=(
                    "Reproduce the configured extension set from live upstream "
                    "Meson custody so the atomic seal persists "
                    "runtime_python_import_modules; do not rerun the heavy "
                    "pact-witness-acceptance lane until package admission passes."
                ),
                scopes=(
                    "src/molt/cli/external_native.py",
                    "src/molt/cli/extension_seal.py",
                    "src/molt/cli/source_extension_producer.py",
                    "tools/pact_seal_witness_roots.py",
                ),
            )
        )

    match = NATIVE_ARTIFACT_CUSTODY_RE.search(log_tail)
    if match is not None and not _diagnostics_have_signal(
        diagnostics, "external-native-runtime-import-custody"
    ):
        missing_abi_symbols = tuple(
            dict.fromkeys(
                symbol_match.group("symbol")
                for symbol_match in NATIVE_ARTIFACT_ABI_SURFACE_RE.finditer(
                    match.group("detail")
                )
            )
        )
        if missing_abi_symbols:
            listed = ", ".join(missing_abi_symbols[:6])
            if len(missing_abi_symbols) > 6:
                listed += f", ... (+{len(missing_abi_symbols) - 6} more)"
            diagnostics.append(
                _diagnostic(
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
                _diagnostic(
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

    match = NATIVE_SUPPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
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

    match = STDLIB_PROFILE_REFUSAL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
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
