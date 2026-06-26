#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import sys
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import check_subprocess_guard_coverage, guarded_entrypoints  # noqa: E402


@dataclass(frozen=True, slots=True)
class TokenContract:
    path: str
    tokens: tuple[str, ...]
    reason: str


@dataclass(frozen=True, slots=True)
class MissingPath:
    path: str
    reason: str


@dataclass(frozen=True, slots=True)
class MissingToken:
    path: str
    token: str
    reason: str


@dataclass(frozen=True, slots=True)
class SentinelTokenDrift:
    token: str
    direction: str


@dataclass(frozen=True, slots=True)
class MissingDirectTestGuard:
    path: str
    line: int
    reason: str


@dataclass(frozen=True, slots=True)
class MemoryGuardWiringAudit:
    python_contracts: int
    shell_contracts: int
    scanner_tokens: tuple[str, ...]
    sentinel_tokens: tuple[str, ...]
    missing_paths: tuple[MissingPath, ...]
    missing_tokens: tuple[MissingToken, ...]
    required_sentinel_missing: tuple[str, ...]
    sentinel_drift: tuple[SentinelTokenDrift, ...]
    direct_test_guard_missing: tuple[MissingDirectTestGuard, ...]
    subprocess_guard_scanned_files: int
    subprocess_guard_raw_call_count: int
    subprocess_guard_unexpected: tuple[
        check_subprocess_guard_coverage.RawSubprocessCall, ...
    ]
    subprocess_guard_stale_allowlist: tuple[
        check_subprocess_guard_coverage.AllowedRawSubprocessUse, ...
    ]
    subprocess_guard_expanded_allowlist: tuple[
        check_subprocess_guard_coverage.ExpandedAllowedUse, ...
    ]

    @property
    def ok(self) -> bool:
        return (
            not self.missing_paths
            and not self.missing_tokens
            and not self.required_sentinel_missing
            and not self.sentinel_drift
            and not self.direct_test_guard_missing
            and not self.subprocess_guard_unexpected
            and not self.subprocess_guard_stale_allowlist
            and not self.subprocess_guard_expanded_allowlist
        )


PYTHON_GUARD_CONTRACTS: tuple[TokenContract, ...] = (
    TokenContract(
        "tests/molt_diff.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "start_repo_sentinel",
            "repo_sentinel_active_env_key",
            "_record_memory_guard_sentinel_violation",
        ),
        "differential harness delegates process custody to the shared harness guard",
    ),
    TokenContract(
        "tools/bench.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "HarnessExecutionContext",
            "repo_process_sentinel",
        ),
        "benchmark harness must guard CPython/Molt/WASM child runs",
    ),
    TokenContract(
        "tools/batch_compile_client.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "start_repo_sentinel",
            "process_group_kwargs",
        ),
        "batch compiler server must launch with sentinel process custody",
    ),
    TokenContract(
        "tools/guarded_exec.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "canonical_harness_env",
        ),
        "standalone wrapper must enter the shared guard",
    ),
    TokenContract(
        "tools/dev.py",
        (
            "harness_memory_guard",
            "_check_call_guarded",
            "guarded_completed_process",
            "canonical_harness_env",
            "MOLT_TEST_SUITE",
        ),
        "developer command runner must guard test-suite children",
    ),
    TokenContract(
        "tools/ci_gate.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "_resolve_memory_limits",
            "compile_governor.compile_slot",
            "MOLT_CI_GATE",
            "guarded_exec.py",
        ),
        "tiered CI gate must guard child checks, clamp parallelism, and launch "
        "background runs through guarded_exec",
    ),
    TokenContract(
        "tools/dev_test_runner.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_DEV_TEST",
        ),
        "multi-version dev test runner must guard pytest batches",
    ),
    TokenContract(
        "tools/artifact_cleanup.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_DEV_CLEANUP",
        ),
        "artifact cleanup process/shell probes must stay guarded",
    ),
    TokenContract(
        "deploy/scripts/benchmark_simd.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "canonical_harness_env",
        ),
        "SIMD deployment benchmark must guard external tool invocations",
    ),
    TokenContract(
        "tools/cloudflare_demo_verify.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "canonical_harness_env",
        ),
        "Cloudflare demo verification must guard worker subprocesses",
    ),
    TokenContract(
        "drivers/cloudflare/thin_adapter/verify.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "canonical_harness_env",
        ),
        "Cloudflare thin-adapter verification must guard worker subprocesses",
    ),
    TokenContract(
        "tools/bench_wasm.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "WASM benchmark runner must guard native/WASM compile and run steps",
    ),
    TokenContract(
        "tools/bench_individual.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "individual benchmark runner must guard child runs",
    ),
    TokenContract(
        "tools/bench_friends.py",
        (
            "harness_memory_guard",
            "repo_process_sentinel",
            "MOLT_BENCH",
        ),
        "friend benchmark orchestrator must keep suite-level guard limits and sentinels",
    ),
    TokenContract(
        "tools/bench_friends_phase.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "friend benchmark phase runner must guard child commands",
    ),
    TokenContract(
        "tools/bench_audit.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "benchmark audit helper must guard external commands",
    ),
    TokenContract(
        "tools/bench_backend_incremental.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "backend incremental benchmark helper must guard child commands",
    ),
    TokenContract(
        "tools/benchmark_luau_vs_cpython.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "Luau-vs-CPython benchmark runner must guard child commands",
    ),
    TokenContract(
        "tools/cpython_regrtest.py",
        (
            "harness_memory_guard",
            "HarnessExecutionContext",
            "canonical_regrtest_env",
            "repo_process_sentinel",
            "MOLT_REGRTEST",
        ),
        "CPython regrtest driver must guard test-worker subprocesses",
    ),
    TokenContract(
        "tools/molt_regrtest_shim.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_REGRTEST",
        ),
        "Molt regrtest shim must guard child execution",
    ),
    TokenContract(
        "tools/profile.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "profiling runner must guard benchmark children",
    ),
    TokenContract(
        "tools/throughput_matrix.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "throughput matrix runner must guard compile/run children",
    ),
    TokenContract(
        "tools/translation_validate.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "guarded_harness_scope",
        ),
        "translation validator must guard CPython/Molt comparison children",
    ),
    TokenContract(
        "tools/runtime_safety.py",
        (
            "harness_memory_guard",
            "canonical_harness_env",
            "guarded_completed_process",
            "MOLT_RUNTIME_SAFETY",
        ),
        "runtime sanitizer/Miri/fuzz safety gates must guard Cargo children "
        "with canonical artifact roots",
    ),
    TokenContract(
        "tools/wasm_hotspot_profile.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "WASM hotspot profiler must guard external tools",
    ),
    TokenContract(
        "tools/wasm_pipeline.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "WASM pipeline runner must guard compile/run tools",
    ),
    TokenContract(
        "tools/wasm_profile.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "WASM profiler must guard Node/wasm tools",
    ),
    TokenContract(
        "tools/wasm_run_matrix.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "WASM run matrix must guard host runner children",
    ),
    TokenContract(
        "tools/wasm_stub_wasi.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "WASI stub runner must guard wasm host commands",
    ),
    TokenContract(
        "tools/wasm_strip_unused.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "WASM stripping tool must guard external optimization commands",
    ),
    TokenContract(
        "tests/harness/run_molt_conformance.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "HarnessExecutionContext",
            "repo_process_sentinel",
        ),
        "Molt conformance runner must guard compile/run children",
    ),
    TokenContract(
        "tests/harness/run_monty_conformance.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "Monty conformance runner must guard child commands",
    ),
    TokenContract(
        "tests/benchmarks/bench_generator.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_BENCH",
        ),
        "generator benchmark must guard CPython/Molt runs",
    ),
    TokenContract(
        "tests/compliance/process_guard.py",
        (
            "harness_memory_guard",
            "run_guarded_test_process",
            "MOLT_COMPLIANCE",
        ),
        "compliance tests must use their guarded process helper",
    ),
    TokenContract(
        "pyproject.toml",
        (
            "molt.pytest_memory_guard_bootstrap",
            "molt.pytest_memory_guard_config_plugin",
        ),
        "pytest config must load the startup guard plugin explicitly while "
        "the package entry point guards console-script pytest before "
        "conftest loading",
    ),
    TokenContract(
        "src/molt/pytest_memory_guard_bootstrap.py",
        (
            "tools.pytest_memory_guard_bootstrap",
            "ensure_current_file_test_script_memory_guard",
            "ensure_repo_test_module_memory_guard",
            "pytest_load_initial_conftests",
            "pytest_runtest_call",
        ),
        "packaged pytest plugin shim must make repo startup guard importable "
        "from console-script pytest before pytest mutates sys.path",
    ),
    TokenContract(
        "src/molt/pytest_memory_guard_config_plugin.py",
        (
            "pytest_load_initial_conftests",
            "pytest_runtest_call",
        ),
        "repo pytest config plugin must keep memory-guard startup active even "
        "when pytest entry-point autoload is disabled",
    ),
    TokenContract(
        "src/sitecustomize.py",
        (
            "ensure_python_test_memory_guard",
            "tools.pytest_memory_guard_bootstrap",
        ),
        "project-managed Python startup must guard uv-run direct tests without "
        "adding harness files inside differential corpus directories",
    ),
    TokenContract(
        "sitecustomize.py",
        ("ensure_python_test_memory_guard",),
        "Python startup must enter pytest or direct tests/** script memory "
        "custody before test code can run outside the guard",
    ),
    TokenContract(
        "tests/_sitecustomize.py",
        (
            "install_test_memory_guard_sitecustomize",
            "ensure_repo_test_script_memory_guard",
        ),
        "tests/** path-local sitecustomize routers must share the same "
        "guard bootstrap helper instead of per-file memory custody code",
    ),
    TokenContract(
        "tools/pytest_memory_guard_bootstrap.py",
        (
            "MOLT_MEMORY_GUARD_ACTIVE",
            "MOLT_MEMORY_GUARD_PID",
            "MOLT_PYTEST_OUTER_GUARD_REEXEC",
            "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC",
            "MOLT_PYTEST_CURRENT_TEST_FILE",
            "install_pytest_current_test_file_env",
            "ensure_current_file_test_script_memory_guard",
            "ensure_repo_test_module_memory_guard",
            "PYTEST_XDIST_WORKER",
            "tools/memory_guard.py",
            "MOLT_TEST_SUITE",
            "--noconftest",
            "--confcutdir",
            "PYTEST_ADDOPTS",
            "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
            "pyproject.toml",
            "sample_processes",
            "handoff_to_outer_guard",
            "subprocess.run",
            "os._exit",
            "os.execvpe",
        ),
        "test startup bootstrap must fail closed on forged guard markers, "
        "reject pytest hook-disabling flags, and re-exec through memory_guard.py",
    ),
    TokenContract(
        "tools/memory_guard.py",
        (
            "test_custody_launch_env",
            "MOLT_PYTEST_CURRENT_TEST_FILE",
            "PYTEST_OUTER_GUARD_SUMMARY_DIR",
            "repro_context_payload",
        ),
        "memory guard must allocate test custody sidecars before child spawn "
        "so incident repro diagnostics can name the running test",
    ),
    TokenContract(
        "tools/harness_memory_guard.py",
        (
            "test_custody_launch_env",
            "_guard_repro_message",
            "guarded_completed_process",
        ),
        "harness wrappers must pass the same test custody env to child launch "
        "and repro formatting",
    ),
    TokenContract(
        "tests/conftest.py",
        (
            "harness_memory_guard",
            "outer_memory_guard_active",
            "validate_pytest_guardable_env",
            "repo_process_sentinel",
            "limits_from_env",
            "MOLT_PYTEST",
            "drain_on_exit=True",
        ),
        "pytest collection must install a suite-level repo sentinel after "
        "startup memory custody is active",
    ),
    TokenContract(
        "tests/runtime_compat/test_runtime_compat.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "MOLT_RUNTIME_COMPAT",
        ),
        "runtime compatibility tests must guard child commands",
    ),
    TokenContract(
        "src/molt/harness_layers.py",
        (
            "harness_memory_guard",
            "guarded_completed_process",
            "repo_process_sentinel",
        ),
        "harness quality layers must guard tool subprocesses",
    ),
    TokenContract(
        "src/molt/cli/command_runtime.py",
        (
            "_load_cli_harness_memory_guard",
            "run_completed_command",
            "guarded_completed_process_to_tempfiles",
            "MOLT_DIFF",
        ),
        "CLI shared command runtime must route subprocesses through process_guard",
    ),
    TokenContract(
        "src/molt/cli/commands.py",
        (
            "memory_guard_prefix",
            "MOLT_BUILD",
            "MOLT_BENCH",
            "MOLT_DIFF",
        ),
        "CLI build/test/bench/diff commands must pass family guard prefixes",
    ),
    TokenContract(
        "src/molt/cli/setup_readiness.py",
        ("MOLT_BUILD",),
        "CLI setup/toolchain readiness must use build-family guard prefixes",
    ),
    TokenContract(
        "src/molt/cli/toolchain_validation.py",
        (
            "_load_cli_harness_memory_guard",
            "MOLT_BENCH",
            "MOLT_CONFORMANCE",
        ),
        "CLI validation must summarize and use family guard prefixes",
    ),
)


SHELL_WRAPPER_CONTRACTS: tuple[TokenContract, ...] = (
    TokenContract(
        "bench/run_all.sh",
        ("tools/guarded_exec.py", "MOLT_BENCH", "TMPDIR"),
        "legacy benchmark shell wrapper must enter guarded_exec",
    ),
    TokenContract(
        "bench/scripts/run_stack.sh",
        ("tools/guarded_exec.py", "MOLT_GUARDED_STACK_INNER"),
        "stack benchmark shell wrapper must enter guarded_exec",
    ),
    TokenContract(
        "bench/scripts/run_db_stub.sh",
        ("tools/guarded_exec.py", "MOLT_BENCH", "TMPDIR"),
        "database benchmark shell wrapper must enter guarded_exec",
    ),
    TokenContract(
        "deploy/scripts/deploy.sh",
        ("tools/guarded_exec.py", "MOLT_DEPLOY"),
        "deployment shell wrapper must enter guarded_exec",
    ),
    TokenContract(
        "tools/scripts/compile-bench-wasm.sh",
        ("tools/guarded_exec.py", "MOLT_BENCH"),
        "WASM benchmark shell wrapper must enter guarded_exec",
    ),
    TokenContract(
        "tools/scripts/molt-compile-check.sh",
        (
            "tools/guarded_exec.py",
            "MOLT_TEST_SUITE",
            'mktemp -d "$ROOT/tmp/molt-check-XXXXXX"',
        ),
        "compile-check shell wrapper must enter guarded_exec and repo-local tmp",
    ),
    TokenContract(
        "tests/parity/run_parity.sh",
        ("tools/parity_gate.py", "TMPDIR"),
        "parity shell wrapper must enter the guarded parity gate",
    ),
)


REQUIRED_SENTINEL_TOKENS: tuple[str, ...] = (
    "/bench/harness.py",
    "/bench/wasm_bench.py",
    "/bench/scripts/run_demo_bench.py",
    "/bench/scripts/run_db_stub.py",
    "/bench/luau/run_benchmarks.py",
    "/tests/benchmarks/bench_generator.py",
)

DIRECT_TEST_SITECUSTOMIZE_TOKEN = "install_test_memory_guard_sitecustomize"
DIRECT_TEST_SCRIPT_EXCLUDED_PARTS = frozenset(
    {
        "__pycache__",
        "benchmarks",
        "differential",
        "fixtures",
        "molt_only",
    }
)


def _read_contract_text(root: Path, contract: TokenContract) -> str | None:
    path = root / contract.path
    if not path.exists():
        return None
    return path.read_text(encoding="utf-8")


def _audit_token_contracts(
    root: Path,
    contracts: Sequence[TokenContract],
) -> tuple[tuple[MissingPath, ...], tuple[MissingToken, ...]]:
    missing_paths: list[MissingPath] = []
    missing_tokens: list[MissingToken] = []
    for contract in contracts:
        text = _read_contract_text(root, contract)
        if text is None:
            missing_paths.append(MissingPath(contract.path, contract.reason))
            continue
        for token in contract.tokens:
            if token not in text:
                missing_tokens.append(
                    MissingToken(contract.path, token, contract.reason)
                )
    return tuple(missing_paths), tuple(missing_tokens)


def _process_sentinel_tokens(root: Path) -> tuple[str, ...]:
    if root.resolve() != REPO_ROOT.resolve():
        return ()
    from tools import process_sentinel  # noqa: PLC0415

    return tuple(process_sentinel.GUARDED_ENTRYPOINT_TOKENS)


def _subprocess_guard_targets(root: Path) -> tuple[Path, ...]:
    if root.resolve() == REPO_ROOT.resolve():
        return tuple(check_subprocess_guard_coverage.DEFAULT_TARGETS)
    return tuple(
        root / target.resolve().relative_to(REPO_ROOT.resolve())
        for target in check_subprocess_guard_coverage.DEFAULT_TARGETS
    )


def _is_name(node: ast.AST, value: str) -> bool:
    return isinstance(node, ast.Name) and node.id == value


def _is_constant(node: ast.AST, value: str) -> bool:
    return isinstance(node, ast.Constant) and node.value == value


def _is_dunder_main_guard(node: ast.AST) -> bool:
    if not isinstance(node, ast.Compare) or len(node.ops) != 1:
        return False
    if not isinstance(node.ops[0], ast.Eq) or len(node.comparators) != 1:
        return False
    left = node.left
    right = node.comparators[0]
    return (
        _is_name(left, "__name__")
        and _is_constant(right, "__main__")
        or _is_constant(left, "__main__")
        and _is_name(right, "__name__")
    )


def _main_guard_line(tree: ast.Module) -> int | None:
    for node in ast.walk(tree):
        if isinstance(node, ast.If) and _is_dunder_main_guard(node.test):
            return node.lineno
    return None


def _audit_direct_executable_test_guards(
    root: Path,
) -> tuple[MissingDirectTestGuard, ...]:
    tests_root = root / "tests"
    if not tests_root.exists():
        return ()

    missing: list[MissingDirectTestGuard] = []
    for path in sorted(tests_root.rglob("*.py")):
        relative = path.relative_to(root)
        relative_text = relative.as_posix()
        if any(part in DIRECT_TEST_SCRIPT_EXCLUDED_PARTS for part in relative.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8")
            tree = ast.parse(text, filename=relative_text)
        except (OSError, SyntaxError) as exc:
            missing.append(
                MissingDirectTestGuard(
                    relative_text,
                    0,
                    f"could not parse direct-executable test candidate: {exc}",
                )
            )
            continue
        line = _main_guard_line(tree)
        if line is None:
            continue
        guard_path = path.parent / "sitecustomize.py"
        try:
            guard_text = guard_path.read_text(encoding="utf-8")
        except OSError:
            guard_text = ""
        if DIRECT_TEST_SITECUSTOMIZE_TOKEN in guard_text:
            continue
        missing.append(
            MissingDirectTestGuard(
                relative_text,
                line,
                "direct-executable tests require a path-local sitecustomize.py "
                "that imports the shared test memory-guard bootstrap",
            )
        )
    return tuple(missing)


def audit_repo(
    root: Path = REPO_ROOT,
    *,
    python_contracts: Sequence[TokenContract] = PYTHON_GUARD_CONTRACTS,
    shell_contracts: Sequence[TokenContract] = SHELL_WRAPPER_CONTRACTS,
    required_sentinel_tokens: Sequence[str] = REQUIRED_SENTINEL_TOKENS,
    scanner_tokens: Sequence[str] | None = None,
    sentinel_tokens: Sequence[str] | None = None,
    subprocess_guard_audit: (
        check_subprocess_guard_coverage.SubprocessGuardAudit | None
    ) = None,
) -> MemoryGuardWiringAudit:
    root = root.resolve()
    python_missing_paths, python_missing_tokens = _audit_token_contracts(
        root, python_contracts
    )
    shell_missing_paths, shell_missing_tokens = _audit_token_contracts(
        root, shell_contracts
    )
    scanned = tuple(
        scanner_tokens
        if scanner_tokens is not None
        else guarded_entrypoints.guarded_entrypoint_tokens(root)
    )
    sentinel = tuple(
        sentinel_tokens
        if sentinel_tokens is not None
        else _process_sentinel_tokens(root)
    )

    scanned_set = set(scanned)
    sentinel_set = set(sentinel)
    required_missing = tuple(
        token for token in required_sentinel_tokens if token not in scanned_set
    )
    drift: list[SentinelTokenDrift] = []
    if sentinel:
        for token in sorted(scanned_set - sentinel_set):
            drift.append(SentinelTokenDrift(token, "scanner_not_in_process_sentinel"))
        for token in sorted(sentinel_set - scanned_set):
            drift.append(SentinelTokenDrift(token, "process_sentinel_not_in_scanner"))
    subprocess_audit = (
        subprocess_guard_audit
        if subprocess_guard_audit is not None
        else check_subprocess_guard_coverage.audit_paths(
            _subprocess_guard_targets(root),
            root=root,
        )
    )
    direct_test_guard_missing = _audit_direct_executable_test_guards(root)

    return MemoryGuardWiringAudit(
        python_contracts=len(python_contracts),
        shell_contracts=len(shell_contracts),
        scanner_tokens=tuple(sorted(scanned)),
        sentinel_tokens=tuple(sorted(sentinel)),
        missing_paths=(*python_missing_paths, *shell_missing_paths),
        missing_tokens=(*python_missing_tokens, *shell_missing_tokens),
        required_sentinel_missing=required_missing,
        sentinel_drift=tuple(drift),
        direct_test_guard_missing=direct_test_guard_missing,
        subprocess_guard_scanned_files=subprocess_audit.scanned_files,
        subprocess_guard_raw_call_count=len(subprocess_audit.raw_calls),
        subprocess_guard_unexpected=subprocess_audit.unexpected,
        subprocess_guard_stale_allowlist=subprocess_audit.stale_allowlist,
        subprocess_guard_expanded_allowlist=subprocess_audit.expanded_allowlist,
    )


def _audit_to_dict(audit: MemoryGuardWiringAudit) -> dict[str, object]:
    return {
        "ok": audit.ok,
        "python_contracts": audit.python_contracts,
        "shell_contracts": audit.shell_contracts,
        "scanner_token_count": len(audit.scanner_tokens),
        "sentinel_token_count": len(audit.sentinel_tokens),
        "missing_paths": [asdict(item) for item in audit.missing_paths],
        "missing_tokens": [asdict(item) for item in audit.missing_tokens],
        "required_sentinel_missing": list(audit.required_sentinel_missing),
        "sentinel_drift": [asdict(item) for item in audit.sentinel_drift],
        "direct_test_guard_missing": [
            asdict(item) for item in audit.direct_test_guard_missing
        ],
        "subprocess_guard": {
            "ok": (
                not audit.subprocess_guard_unexpected
                and not audit.subprocess_guard_stale_allowlist
                and not audit.subprocess_guard_expanded_allowlist
            ),
            "scanned_files": audit.subprocess_guard_scanned_files,
            "raw_call_count": audit.subprocess_guard_raw_call_count,
            "unexpected": [asdict(item) for item in audit.subprocess_guard_unexpected],
            "stale_allowlist": [
                asdict(item) for item in audit.subprocess_guard_stale_allowlist
            ],
            "expanded_allowlist": [
                {
                    "entry": asdict(item.entry),
                    "actual_count": item.actual_count,
                }
                for item in audit.subprocess_guard_expanded_allowlist
            ],
        },
    }


def _format_text(audit: MemoryGuardWiringAudit) -> str:
    if audit.ok:
        return (
            "OK: memory guard wiring audit passed "
            f"(python_contracts={audit.python_contracts}, "
            f"shell_contracts={audit.shell_contracts}, "
            f"scanner_tokens={len(audit.scanner_tokens)}, "
            f"sentinel_tokens={len(audit.sentinel_tokens)})\n"
        )

    lines = ["ERROR: memory guard wiring audit failed"]
    if audit.missing_paths:
        lines.append("Missing contract paths:")
        for item in audit.missing_paths:
            lines.append(f"- {item.path}: {item.reason}")
    if audit.missing_tokens:
        lines.append("Missing guard tokens:")
        for item in audit.missing_tokens:
            lines.append(f"- {item.path}: {item.token} ({item.reason})")
    if audit.required_sentinel_missing:
        lines.append("Required process-sentinel scanner tokens missing:")
        for token in audit.required_sentinel_missing:
            lines.append(f"- {token}")
    if audit.sentinel_drift:
        lines.append("Process-sentinel scanner drift:")
        for item in audit.sentinel_drift:
            lines.append(f"- {item.direction}: {item.token}")
    if audit.direct_test_guard_missing:
        lines.append("Direct-executable test scripts missing startup guard:")
        for item in audit.direct_test_guard_missing:
            lines.append(f"- {item.path}:{item.line} {item.reason}")
    if audit.subprocess_guard_unexpected:
        lines.append("Unexpected raw subprocess calls:")
        for item in audit.subprocess_guard_unexpected:
            lines.append(
                f"- {item.path}:{item.line} {item.qualname} "
                f"subprocess.{item.method}: {item.source}"
            )
    if audit.subprocess_guard_stale_allowlist:
        lines.append("Stale subprocess guard allowlist entries:")
        for item in audit.subprocess_guard_stale_allowlist:
            lines.append(
                f"- {item.path} {item.qualname} subprocess.{item.method}: {item.reason}"
            )
    if audit.subprocess_guard_expanded_allowlist:
        lines.append("Expanded subprocess guard allowlist entries:")
        for item in audit.subprocess_guard_expanded_allowlist:
            entry = item.entry
            lines.append(
                f"- {entry.path} {entry.qualname} subprocess.{entry.method}: "
                f"expected {entry.expected_count}, found {item.actual_count}"
            )
    lines.append(
        "Keep benchmark, conformance, regrtest, compliance, CLI, and dev-test "
        "entrypoints routed through tools.harness_memory_guard or an approved "
        "guarded wrapper before adding them to release validation."
    )
    return "\n".join(lines) + "\n"


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audit Molt dev/test/bench entrypoints for default memory guard wiring."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="Repository root to audit.",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON output.")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = _parse_args(argv)
    audit = audit_repo(args.root)
    if args.json:
        print(json.dumps(_audit_to_dict(audit), indent=2, sort_keys=True))
    else:
        sys.stdout.write(_format_text(audit))
    return 0 if audit.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
