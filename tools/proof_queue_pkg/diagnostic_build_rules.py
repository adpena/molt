"""Compiler, Cargo, and source-extension proof failure rules."""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path

from tools.proof_queue_pkg.diagnostic_evidence import (
    _first_log_line_containing,
    _last_nonempty_log_line,
)
from tools.proof_queue_pkg.diagnostic_model import _diagnostic

SOURCE_EXTENSION_NM_MISSING_RE = re.compile(
    r"unable to read global symbol table for compiled extension object "
    r"(?P<object>[^\r\n;]+); canonical LLVM/WASI nm authority is unavailable"
)

RUST_COMPILER_ERROR_RE = re.compile(
    r"(?m)^error(?:\[(?P<code>E\d{4})\])?: (?P<message>[^\r\n]+)"
)

RUST_TEST_RESULT_FAILED_RE = re.compile(
    r"(?m)^test result: FAILED\.(?P<detail>[^\r\n]*)"
)

RUST_CARGO_TEST_FAILED_RE = re.compile(
    r"(?m)^error: test failed, to rerun pass `(?P<rerun>[^`]+)`"
)

RUST_FAILED_TEST_LINE_RE = re.compile(
    r"(?m)^test (?P<name>[A-Za-z0-9_:<>_.-]+) \.\.\. FAILED\r?$"
)

RUNTIME_WASM_RUST_TARGET_MISSING_RE = re.compile(
    r"(?m)^Runtime wasm build requires Rust target (?P<target>[A-Za-z0-9_-]+), "
    r"but the active Rust toolchain does not provide it\. "
    r"Run: (?P<command>rustup target add [^\r\n]+)\r?$"
)

WASM_TOOLCHAIN_CONTRACT_IMPORT_MISSING_RE = re.compile(
    r"(?m)^failed to import WASM toolchain contract: "
    r"No module named ['\"](?P<module>[A-Za-z0-9_.]+)['\"]\r?$"
)

SOURCE_EXTENSION_BUILD_PLAN_MISSING_RE = re.compile(
    r"source extension build plan not found: (?P<path>[^\r\n\"]+)"
)

SOURCE_EXTENSION_COMPILE_HEADER_MISSING_RE = re.compile(
    r"Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?fatal error: "
    r"'(?P<header>[^']+)' file not found"
)

SOURCE_EXTENSION_CIMPORT_HEADER_MISMATCH_RE = re.compile(
    r"(?P<evidence>Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?"
    r"(?:call to undeclared function "
    r"'(?P<symbol>PyDataType_[^']+|_PyUFuncObject_GET_ITEM_DATA)'|"
    r"member reference type 'int' is not a pointer)[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)

SOURCE_EXTENSION_CPYTHON_ABI_DECL_MISSING_RE = re.compile(
    r"(?P<evidence>Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?"
    r"call to undeclared function '(?P<symbol>_?Py[A-Za-z0-9_]+)'[\s\S]*?"
    r"Python\.h[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)

SOURCE_EXTENSION_CYTHON_REGENERATION_FAILED_RE = re.compile(
    r"Standalone `cython -3` regeneration of (?P<source>[^`]+) failed: "
    r"(?P<error>[^\r\n\"]+)"
)

CPYTHON_ABI_PYMOD_GIL_SLOT_RE = re.compile(
    r"(?P<evidence>Failed compiling [^\r\n]+:[\s\S]*?"
    r"incompatible integer to pointer conversion[\s\S]*?"
    r"Py_mod_multiple_interpreters[\s\S]*?"
    r"Py_MOD_PER_INTERPRETER_GIL_SUPPORTED[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)


def _append_diagnostics(
    row: sqlite3.Row,
    log_tail: str,
    diagnostics: list[dict[str, object]],
    *,
    fatal_queue_failure: bool,
) -> None:
    if (
        not fatal_queue_failure
        and "proof queue nonfatal infrastructure failure" in log_tail
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="queue-infra-warning",
                severity="infra",
                summary=(
                    "The proof command ran, but queue-side observability had a "
                    "nonfatal infrastructure failure."
                ),
                evidence=(
                    _first_log_line_containing(
                        log_tail, "proof queue nonfatal infrastructure failure"
                    )
                    or _last_nonempty_log_line(Path(row["log_path"]))
                    or ""
                ),
                next_action=(
                    "Preserve the proof result, then fix the queue projection or "
                    "note append issue before it becomes hidden collaboration debt."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    if (
        "[scoreboard] machine NOT quiescent" in log_tail
        and "[scoreboard] refusing non-authoritative measurement before starting benchmark builds"
        in log_tail
    ):
        evidence_parts = [
            _first_log_line_containing(log_tail, "[scoreboard] machine NOT quiescent")
            or "",
            _first_log_line_containing(
                log_tail,
                "[scoreboard] refusing non-authoritative measurement",
            )
            or "",
        ]
        diagnostics.append(
            _diagnostic(
                signal_id="perf-scoreboard-not-quiescent",
                severity="operator",
                summary=(
                    "The canonical perf scoreboard failed closed before "
                    "benchmarking because the machine never became quiescent."
                ),
                evidence="\n".join(part for part in evidence_parts if part),
                next_action=(
                    "Let active build/proof work drain or schedule an exclusive "
                    "perf window, then rerun the same canonical scoreboard from "
                    "current origin/main. Do not use --allow-nonauthoritative for "
                    "release or acceptance evidence."
                ),
                scopes=(
                    "tools/perf_scoreboard.py",
                    "tools/proof_queue.py",
                    "docs/agent/ORCHESTRATION.md",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    rust_test_result = RUST_TEST_RESULT_FAILED_RE.search(log_tail)
    rust_cargo_test_failed = RUST_CARGO_TEST_FAILED_RE.search(log_tail)
    if rust_test_result is not None or rust_cargo_test_failed is not None:
        failed_tests = tuple(
            dict.fromkeys(
                match.group("name")
                for match in RUST_FAILED_TEST_LINE_RE.finditer(log_tail)
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
            _diagnostic(
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

    match = RUST_COMPILER_ERROR_RE.search(log_tail)
    if (
        match is not None
        and rust_test_result is None
        and rust_cargo_test_failed is None
    ):
        code = match.group("code") or "rustc"
        message = match.group("message").strip()
        diagnostics.append(
            _diagnostic(
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

    match = RUNTIME_WASM_RUST_TARGET_MISSING_RE.search(log_tail)
    if match is not None:
        target = match.group("target")
        diagnostics.append(
            _diagnostic(
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

    match = WASM_TOOLCHAIN_CONTRACT_IMPORT_MISSING_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        diagnostics.append(
            _diagnostic(
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

    match = SOURCE_EXTENSION_NM_MISSING_RE.search(log_tail)
    if match is not None:
        object_path = Path(match.group("object"))
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-nm-missing",
                severity="infra",
                summary=(
                    "Source-extension object-symbol scan could not read "
                    f"{object_path.name} because canonical LLVM/WASI nm "
                    "authority was unavailable."
                ),
                evidence=match.group(0),
                next_action=(
                    "Repair or install the complete managed LLVM/WASI tool family "
                    "under MOLT_TARGET_ROOT; the compiler/linker/symbol-reader "
                    "family is one authority, not a per-command override."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "src/molt/cli/backend_cache.py",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_BUILD_PLAN_MISSING_RE.search(log_tail)
    if match is not None:
        source_plan_path = Path(match.group("path").strip())
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-build-plan-missing",
                severity="infra",
                summary=(
                    "Source-extension build could not find the declared "
                    f"source plan {source_plan_path.name}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Route this through source-extension package custody and "
                    "toolchain provisioning: derive Meson/Cython build metadata, "
                    "generated headers, include roots, and build-root resolution "
                    "from the package's own build system; do not hand-author "
                    "package metadata or rerun the same row unchanged."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_COMPILE_HEADER_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-compile-header-missing",
                severity="infra",
                summary=(
                    "Source-extension compile could not resolve required header "
                    f"{match.group('header')!r} while compiling "
                    f"{match.group('source').strip()}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Fix the shared source-extension build-plan/provisioning "
                    "authority so generated headers and include roots are "
                    "derived from package build metadata and preserved in the "
                    "source plan; do not copy headers or patch compiler commands "
                    "by hand."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CYTHON_REGENERATION_FAILED_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cython-regeneration-failed",
                severity="infra",
                summary=(
                    "Source-extension Cython regeneration failed for "
                    f"{match.group('source').strip()}: "
                    f"{match.group('error').strip()}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Fix the shared source-extension Cython provisioning "
                    "authority so regeneration uses the package's declared "
                    "build metadata, generated dependency graph, include roots, "
                    "and toolchain configuration; do not add a package-specific "
                    "standalone Cython command."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CIMPORT_HEADER_MISMATCH_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol") or "package C accessor"
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cimport-header-mismatch",
                severity="infra",
                summary=(
                    "Source-extension compile used Cython pxd facts that do not "
                    f"match the C header include surface while compiling "
                    f"{match.group('source').strip()} ({symbol})."
                ),
                evidence=match.group("evidence"),
                next_action=(
                    "Keep cimport .pxd roots and package C header include roots "
                    "under the same build-interpreter package custody. Derive "
                    "both from source cimports and package build hooks; do not "
                    "pin an older Cython, copy package headers, or add a "
                    "package-specific source-plan/header overlay."
                ),
                scopes=(
                    "src/molt/cli/source_extension_cython.py",
                    "src/molt/cli/extension_commands.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CPYTHON_ABI_DECL_MISSING_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cpython-abi-declaration-missing",
                severity="error",
                summary=(
                    "Source-extension compile requires CPython ABI declaration "
                    f"{symbol}, but Molt's cpython-abi header does not expose it."
                ),
                evidence=match.group("evidence"),
                next_action=(
                    "Route to the cpython-abi owner to add the missing "
                    "declaration, macro, or helper as a shared C-API primitive. "
                    "Do not relax compiler diagnostics, pin an older Cython, or "
                    "patch the package source/source-plan around the missing ABI."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/include/Python.h",
                    "runtime/molt-cpython-abi/",
                    "src/molt/cli/source_extensions.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = CPYTHON_ABI_PYMOD_GIL_SLOT_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="cpython-abi-pymod-gil-slot-token-mismatch",
                severity="error",
                summary=(
                    "CPython-ABI header exposes Py_MOD_PER_INTERPRETER_GIL_SUPPORTED "
                    "as an integer token where PyModuleDef_Slot.value expects a "
                    "pointer-shaped value."
                ),
                next_action=(
                    "Route to the cpython-abi owner to make the Py_mod_multiple_interpreters "
                    "slot token ABI-compatible as a reusable C-API primitive; "
                    "do not work around this in a package source-plan or compiler "
                    "command."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/include/Python.h",
                    "runtime/molt-cpython-abi/",
                ),
                evidence=match.group("evidence"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )
