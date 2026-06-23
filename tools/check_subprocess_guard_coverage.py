#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
from collections import Counter
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import sys
import warnings


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TARGETS = (
    REPO_ROOT / "tools",
    REPO_ROOT / "tests",
    REPO_ROOT / "src" / "molt" / "backend_daemon_custody.py",
    REPO_ROOT / "src" / "molt" / "cli.py",
    REPO_ROOT / "src" / "molt" / "process_guard.py",
    REPO_ROOT / "src" / "molt" / "repl.py",
)
EXCLUDED_PREFIXES = (
    "tests/differential/",
    "tests/harness/corpus/",
)
SUBPROCESS_METHODS = frozenset(
    {
        "run",
        "Popen",
        "check_call",
        "check_output",
        "call",
    }
)
OS_SIGNAL_METHODS = frozenset({"kill", "killpg"})
SHELL_KILL_PATTERNS = ("pkill ", "pkill\t", "pkill -", "kill -TERM", "kill -KILL")


@dataclass(frozen=True, slots=True)
class RawSubprocessCall:
    path: str
    line: int
    qualname: str
    method: str
    source: str

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.path, self.qualname, self.method)


@dataclass(frozen=True, slots=True)
class AllowedRawSubprocessUse:
    path: str
    qualname: str
    method: str
    reason: str
    expected_count: int = 1

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.path, self.qualname, self.method)


@dataclass(frozen=True, slots=True)
class ExpandedAllowedUse:
    entry: AllowedRawSubprocessUse
    actual_count: int


@dataclass(frozen=True, slots=True)
class SubprocessGuardAudit:
    scanned_files: int
    raw_calls: tuple[RawSubprocessCall, ...]
    unexpected: tuple[RawSubprocessCall, ...]
    stale_allowlist: tuple[AllowedRawSubprocessUse, ...]
    expanded_allowlist: tuple[ExpandedAllowedUse, ...]

    @property
    def ok(self) -> bool:
        return (
            not self.unexpected
            and not self.stale_allowlist
            and not self.expanded_allowlist
        )


ALLOWLIST: tuple[AllowedRawSubprocessUse, ...] = (
    AllowedRawSubprocessUse(
        "tools/batch_compile_client.py",
        "BatchCompileServerClient.__init__",
        "Popen",
        "interactive batch-build server Popen runs under HarnessExecutionContext, "
        "process-group kwargs, force-close custody, and a repo sentinel",
    ),
    AllowedRawSubprocessUse(
        "tools/bench.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; benchmark child execution uses MOLT_BENCH guard",
    ),
    AllowedRawSubprocessUse(
        "tools/bench_friends.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; suite phases use guarded_completed_process",
    ),
    AllowedRawSubprocessUse(
        "tools/bench_wasm.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; wasm build/run phases use MOLT_BENCH guard",
    ),
    AllowedRawSubprocessUse(
        "tools/agent_coordination.py",
        "run_codex_stall_diagnostic",
        "Popen",
        "interactive Codex stall diagnostic launches tools/memory_guard.py by "
        "default, mirrors child streams live, writes only timing/byte-count "
        "metadata under canonical artifact roots, and force-closes the direct "
        "child on interruption",
    ),
    AllowedRawSubprocessUse(
        "tools/check_correspondence.py",
        "_find_repo_root",
        "check_output",
        "bounded git metadata probe used before static file correspondence checks",
    ),
    AllowedRawSubprocessUse(
        "tools/check_correspondence_extended.py",
        "_find_repo_root",
        "check_output",
        "bounded git metadata probe used before static file correspondence checks",
    ),
    AllowedRawSubprocessUse(
        "tools/ci_gate.py",
        "launch_background_gate",
        "Popen",
        "background launcher execs tools/guarded_exec.py as the direct child",
    ),
    AllowedRawSubprocessUse(
        "tools/code_search.py",
        "main",
        "run",
        "bounded developer ripgrep helper; not a Molt build/test/bench child lane",
    ),
    AllowedRawSubprocessUse(
        "tools/compile_governor.py",
        "_count_active_compile_processes",
        "run",
        "bounded ps metadata probe for adaptive compile throttling",
    ),
    AllowedRawSubprocessUse(
        "tools/compile_progress.py",
        "_kill_run_scoped_processes",
        "run",
        "bounded ps metadata probe before killing marker-scoped compile children",
    ),
    AllowedRawSubprocessUse(
        "tools/compile_progress.py",
        "_kill_run_scoped_processes",
        "os.kill",
        "marker-scoped compile-child cleanup; backend daemon commands are excluded",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_stringprep_tables.py",
        "_rustfmt_text",
        "run",
        "bounded rustfmt child for checked-in generated Rust stringprep tables",
    ),
    AllowedRawSubprocessUse(
        "tools/molt_dev.py",
        "probe_pid",
        "os.kill",
        "bounded pid-liveness probe only; detached-run and gate children avoid raw "
        "subprocess launchers",
    ),
    AllowedRawSubprocessUse(
        "tools/linear_workspace.py",
        "_git_run",
        "run",
        "bounded git workspace metadata/mutation helper outside benchmark/test lanes",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_darwin_physical_memory_bytes",
        "run",
        "memory guard platform probe for adaptive host budgets",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_darwin_available_memory_bytes",
        "run",
        "memory guard vm_stat platform probe for adaptive host budgets",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "sample_processes_posix",
        "run",
        "memory guard POSIX process sampler",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_run_child_runner",
        "run",
        "Windows child-runner custody preserves the guarded child under "
        "tools/memory_guard.py when POSIX exec is unavailable",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "main",
        "run",
        "Windows hidden-argv custody runs the internal memory_guard worker "
        "without exposing the guarded command on the parent argv",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "run_guarded",
        "Popen",
        "lowest-level guarded subprocess implementation",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_pid_exited_or_unobservable",
        "os.kill",
        "memory guard pid-existence probe with signal 0 after a scoped "
        "termination attempt; not signal authority",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_process_group_exited_or_unobservable",
        "os.killpg",
        "memory guard process-group existence probe with signal 0 after a "
        "scoped termination attempt; not signal authority",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_terminate_single_process_group",
        "os.killpg",
        "memory guard low-level process-group teardown primitive",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_terminate_single_process_group",
        "os.kill",
        "memory guard low-level process-group fallback termination primitive",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_terminate_single_pid",
        "os.kill",
        "memory guard exact escaped-PID teardown primitive",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_send_pid_signal_action",
        "os.kill",
        "memory guard watched-root and escaped-PID signal primitive",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_send_process_group_signal_action",
        "os.killpg",
        "memory guard watched process-group signal primitive",
    ),
    AllowedRawSubprocessUse(
        "tools/pytest_memory_guard_bootstrap.py",
        "handoff_to_outer_guard",
        "run",
        "Windows import-time custody handoff waits for tools/memory_guard.py "
        "and exits the bootstrap process with the guarded result",
    ),
    AllowedRawSubprocessUse(
        "tools/profile.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; profiling commands use MOLT_BENCH guard",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_scoreboard.py",
        "_metadata_probe",
        "run",
        "single bounded read-only host metadata probe for pgrep/sysctl/ps/pmset/"
        "git/version checks; workload and profiling children use guard custody",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_scoreboard.py",
        "_profiling_popen",
        "Popen",
        "single interactive profiling child launcher with MOLT_BENCH process-group "
        "custody and shared force-close cleanup",
    ),
    AllowedRawSubprocessUse(
        "tools/secret_guard.py",
        "_run",
        "run",
        "bounded git diff/show helper for static secret scanning",
    ),
    AllowedRawSubprocessUse(
        "tools/safe_run.py",
        "_group_rss_kib",
        "run",
        "safe_run low-level process-group RSS sampler",
    ),
    AllowedRawSubprocessUse(
        "tools/safe_run.py",
        "main",
        "Popen",
        "safe_run low-level guarded subprocess implementation",
    ),
    AllowedRawSubprocessUse(
        "tools/safe_run.py",
        "_kill_group",
        "os.killpg",
        "safe_run low-level process-group teardown primitive",
    ),
    AllowedRawSubprocessUse(
        "tools/process_sentinel.py",
        "terminate_group",
        "os.killpg",
        "repo process sentinel low-level process-group teardown primitive",
        expected_count=3,
    ),
    AllowedRawSubprocessUse(
        "tools/process_sentinel.py",
        "terminate_group",
        "os.kill",
        "repo process sentinel Windows PID teardown primitive when process "
        "groups are unavailable",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tools/bench_backend_incremental.py",
        "_terminate_process",
        "os.killpg",
        "backend incremental benchmark tears down its own timeout child group",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tools/check_subprocess_guard_coverage.py",
        "<module>",
        "shell.kill",
        "static checker pattern vocabulary for shell kill detection",
        expected_count=5,
    ),
    AllowedRawSubprocessUse(
        "tests/cli/test_backend_daemon_sequential.py",
        "daemon_socket",
        "Popen",
        "interactive daemon fixture uses CLI test process-group custody and close",
    ),
    AllowedRawSubprocessUse(
        "tests/cli/test_cli_import_collection.py",
        "test_internal_batch_build_server_ping_shutdown_roundtrip",
        "Popen",
        "interactive stdin/stdout protocol test uses CLI test process-group custody",
    ),
    AllowedRawSubprocessUse(
        "tests/cli/process_guard.py",
        "guarded_cli_test_popen",
        "Popen",
        "interactive CLI test child runs as tools/memory_guard.py wrapper with "
        "MOLT_CLI_TEST limits, process-group custody, and force-close helper",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_ps_supports_field",
        "run",
        "differential harness platform ps capability probe",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_pid_rss_age",
        "run",
        "differential harness process RSS/age sampler",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_list_backend_daemon_processes",
        "run",
        "differential harness daemon metadata sampler",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_list_orphan_diff_workers",
        "run",
        "differential harness orphan-worker metadata sampler",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_list_process_rows",
        "run",
        "differential harness process-table sampler",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_dyld_preflight_error",
        "run",
        "differential harness bounded dyld preflight compile probe",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_pid_alive",
        "os.kill",
        "differential harness generic pid-liveness probe",
    ),
    AllowedRawSubprocessUse(
        "tests/molt_diff.py",
        "_kill_pid",
        "os.kill",
        "differential harness generic orphan worker/build-helper teardown; "
        "backend daemon pruning is covered by identity-custody tests",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tests/test_tkinter_phase0_wrappers.py",
        "_run_probe",
        "run",
        "bounded runtime-compat bootstrap probe that injects a custom intrinsic table",
    ),
    AllowedRawSubprocessUse(
        "tests/test_wasm_split_runtime.py",
        "_run_split_worker_live",
        "Popen",
        "interactive wrangler live-worker probe applies wasm-test process-group "
        "custody and explicit force-close cleanup",
    ),
    AllowedRawSubprocessUse(
        "tests/test_wasm_split_runtime.py",
        "_run_split_worker_live._terminate_worker_tree",
        "os.killpg",
        "interactive wrangler live-worker probe force-closes its own child group",
    ),
    AllowedRawSubprocessUse(
        "tests/tools/test_subprocess_guard_coverage.py",
        "test_unclassified_shell_pkill_string_fails",
        "shell.kill",
        "static checker fixture that proves unclassified shell process-kill strings fail",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli.py",
        "_run_completed_command",
        "run",
        "CLI subprocess helper's explicit unguarded branch for opt-out call sites",
    ),
    AllowedRawSubprocessUse(
        "src/molt/backend_daemon_custody.py",
        "_process_command",
        "run",
        "bounded ps metadata probe before daemon identity sidecars can authorize signals",
    ),
    AllowedRawSubprocessUse(
        "src/molt/backend_daemon_custody.py",
        "_pid_alive",
        "os.kill",
        "backend daemon custody pid-liveness probe only; not signal authority",
    ),
    AllowedRawSubprocessUse(
        "src/molt/backend_daemon_custody.py",
        "terminate_backend_daemon_identity",
        "os.kill",
        "backend daemon custody verified termination and verified escalation",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "src/molt/process_guard.py",
        "run_completed_command",
        "run",
        "shared subprocess guard helper's explicit unguarded branch for opt-out call sites",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli.py",
        "_reexec_cli_with_hash_seed",
        "run",
        "Windows deterministic-PYTHONHASHSEED self-reexec path; POSIX uses "
        "execvpe and the restarted process preserves the same CLI custody path",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli.py",
        "_start_backend_daemon",
        "Popen",
        "backend daemon start uses HarnessExecutionContext, process-group kwargs, "
        "and repo-sentinel startup custody",
    ),
)


class _SubprocessVisitor(ast.NodeVisitor):
    def __init__(self, *, path: str, source_text: str) -> None:
        self.path = path
        self.source_text = source_text
        self.subprocess_aliases: set[str] = {"subprocess"}
        self.os_aliases: set[str] = {"os"}
        self.direct_imports: dict[str, str] = {}
        self.direct_os_imports: dict[str, str] = {}
        self.stack: list[str] = []
        self.calls: list[RawSubprocessCall] = []

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            if alias.name == "subprocess":
                self.subprocess_aliases.add(alias.asname or "subprocess")
            if alias.name == "os":
                self.os_aliases.add(alias.asname or "os")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module == "subprocess":
            for alias in node.names:
                if alias.name in SUBPROCESS_METHODS:
                    self.direct_imports[alias.asname or alias.name] = alias.name
        if node.module == "os":
            for alias in node.names:
                if alias.name in OS_SIGNAL_METHODS:
                    self.direct_os_imports[alias.asname or alias.name] = alias.name
        self.generic_visit(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.stack.append(node.name)
        self.generic_visit(node)
        self.stack.pop()

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.stack.append(node.name)
        self.generic_visit(node)
        self.stack.pop()

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.visit_FunctionDef(node)

    def visit_Call(self, node: ast.Call) -> None:
        method = self._raw_call_method(node.func)
        if method is not None:
            source = ast.get_source_segment(self.source_text, node)
            if source is None:
                source = f"{method}(...)"
            self.calls.append(
                RawSubprocessCall(
                    path=self.path,
                    line=node.lineno,
                    qualname=".".join(self.stack) if self.stack else "<module>",
                    method=method,
                    source=" ".join(source.strip().split()),
                )
            )
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        if isinstance(node.value, str) and any(
            pattern in node.value for pattern in SHELL_KILL_PATTERNS
        ):
            source = ast.get_source_segment(self.source_text, node) or repr(node.value)
            self.calls.append(
                RawSubprocessCall(
                    path=self.path,
                    line=node.lineno,
                    qualname=".".join(self.stack) if self.stack else "<module>",
                    method="shell.kill",
                    source=" ".join(source.strip().split()),
                )
            )
        self.generic_visit(node)

    def _raw_call_method(self, node: ast.expr) -> str | None:
        method = self._subprocess_method(node)
        if method is not None:
            return method
        return self._os_signal_method(node)

    def _subprocess_method(self, node: ast.expr) -> str | None:
        if (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id in self.subprocess_aliases
            and node.attr in SUBPROCESS_METHODS
        ):
            return node.attr
        if isinstance(node, ast.Name):
            return self.direct_imports.get(node.id)
        return None

    def _os_signal_method(self, node: ast.expr) -> str | None:
        if (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id in self.os_aliases
            and node.attr in OS_SIGNAL_METHODS
        ):
            return f"os.{node.attr}"
        if isinstance(node, ast.Name):
            method = self.direct_os_imports.get(node.id)
            if method is not None:
                return f"os.{method}"
        return None


def _normalize_path(path: Path, *, root: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def _is_excluded(path: Path, *, root: Path) -> bool:
    rel = _normalize_path(path, root=root)
    return any(rel.startswith(prefix) for prefix in EXCLUDED_PREFIXES)


def _iter_python_files(paths: Sequence[Path], *, root: Path) -> Iterable[Path]:
    seen: set[Path] = set()
    for raw_path in paths:
        path = raw_path if raw_path.is_absolute() else root / raw_path
        path = path.resolve()
        if path.is_file():
            candidates = [path]
        else:
            candidates = sorted(path.rglob("*.py"))
        for candidate in candidates:
            if candidate in seen or _is_excluded(candidate, root=root):
                continue
            seen.add(candidate)
            yield candidate


def _scan_file(path: Path, *, root: Path) -> tuple[RawSubprocessCall, ...]:
    rel = _normalize_path(path, root=root)
    source_text = path.read_text(encoding="utf-8")
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", SyntaxWarning)
        tree = ast.parse(source_text, filename=rel)
    visitor = _SubprocessVisitor(path=rel, source_text=source_text)
    visitor.visit(tree)
    return tuple(visitor.calls)


def audit_paths(
    paths: Sequence[Path] = DEFAULT_TARGETS,
    *,
    root: Path = REPO_ROOT,
    allowlist: Sequence[AllowedRawSubprocessUse] = ALLOWLIST,
) -> SubprocessGuardAudit:
    scanned_files = 0
    raw_calls: list[RawSubprocessCall] = []
    for path in _iter_python_files(paths, root=root):
        scanned_files += 1
        raw_calls.extend(_scan_file(path, root=root))

    allowed_by_key: Mapping[tuple[str, str, str], AllowedRawSubprocessUse] = {
        entry.key: entry for entry in allowlist
    }
    counts = Counter(call.key for call in raw_calls)
    unexpected = tuple(call for call in raw_calls if call.key not in allowed_by_key)
    stale = tuple(entry for entry in allowlist if counts[entry.key] == 0)
    expanded = tuple(
        ExpandedAllowedUse(entry, counts[entry.key])
        for entry in allowlist
        if counts[entry.key] not in {0, entry.expected_count}
    )
    return SubprocessGuardAudit(
        scanned_files=scanned_files,
        raw_calls=tuple(raw_calls),
        unexpected=unexpected,
        stale_allowlist=stale,
        expanded_allowlist=expanded,
    )


def _audit_to_dict(audit: SubprocessGuardAudit) -> dict[str, object]:
    return {
        "ok": audit.ok,
        "scanned_files": audit.scanned_files,
        "raw_call_count": len(audit.raw_calls),
        "unexpected": [asdict(call) for call in audit.unexpected],
        "stale_allowlist": [asdict(entry) for entry in audit.stale_allowlist],
        "expanded_allowlist": [
            {
                "entry": asdict(item.entry),
                "actual_count": item.actual_count,
            }
            for item in audit.expanded_allowlist
        ],
    }


def _format_text(audit: SubprocessGuardAudit) -> str:
    lines: list[str] = []
    if audit.ok:
        lines.append(
            "OK: subprocess guard coverage audit passed "
            f"(scanned_files={audit.scanned_files}, "
            f"raw_calls={len(audit.raw_calls)}, "
            f"allowlist_entries={len(ALLOWLIST)})"
        )
        return "\n".join(lines) + "\n"
    lines.append("ERROR: subprocess guard coverage audit failed")
    if audit.unexpected:
        lines.append("Unexpected raw subprocess calls:")
        for call in audit.unexpected:
            lines.append(
                f"- {call.path}:{call.line} {call.qualname} "
                f"subprocess.{call.method}: {call.source}"
            )
    if audit.stale_allowlist:
        lines.append("Stale allowlist entries:")
        for entry in audit.stale_allowlist:
            lines.append(
                f"- {entry.path} {entry.qualname} subprocess.{entry.method}: "
                f"{entry.reason}"
            )
    if audit.expanded_allowlist:
        lines.append("Expanded allowlist entries:")
        for item in audit.expanded_allowlist:
            entry = item.entry
            lines.append(
                f"- {entry.path} {entry.qualname} subprocess.{entry.method}: "
                f"expected {entry.expected_count}, found {item.actual_count}"
            )
    lines.append(
        "Route heavy dev/test/bench subprocesses through "
        "tools.harness_memory_guard or a tests/* process_guard helper. "
        "Only add allowlist entries for bounded metadata probes, guard "
        "internals, or interactive Popen paths with explicit process custody."
    )
    return "\n".join(lines) + "\n"


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audit raw subprocess calls against the memory-guard contract."
    )
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="Files or directories to scan (defaults to dev/test/CLI surfaces).",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON output.")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = _parse_args(argv)
    paths = tuple(args.paths) if args.paths else DEFAULT_TARGETS
    audit = audit_paths(paths)
    if args.json:
        print(json.dumps(_audit_to_dict(audit), indent=2, sort_keys=True))
    else:
        sys.stdout.write(_format_text(audit))
    return 0 if audit.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
