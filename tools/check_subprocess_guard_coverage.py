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
    REPO_ROOT / "src" / "molt" / "cli",
    REPO_ROOT / "src" / "molt" / "process_guard.py",
    REPO_ROOT / "src" / "molt" / "repl.py",
    REPO_ROOT / "src" / "molt_accel",
    REPO_ROOT / "packaging",
)
DEFAULT_TEXT_TARGETS = (
    REPO_ROOT / "Makefile",
    REPO_ROOT / "Makefile.pgo",
    REPO_ROOT / "packaging",
    REPO_ROOT / "tools",
)
EXCLUDED_PREFIXES = (
    "tests/differential/",
    "tests/harness/corpus/",
)
TEXT_FILE_NAMES = frozenset({"Makefile", "Makefile.pgo"})
TEXT_FILE_SUFFIXES = frozenset({".bat", ".cmd", ".ps1", ".sh"})
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
PROCESS_OBJECT_SIGNAL_METHODS = frozenset({"kill", "terminate"})
SHELL_KILL_PATTERNS = (
    "Stop-Process",
    "kill -9",
    "kill -KILL",
    "kill -TERM",
    "killall ",
    "killall\t",
    "pkill ",
    "pkill\t",
    "pkill -",
    "taskkill",
)


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
        "tools/batch_compile_client.py",
        "BatchCompileServerClient.force_close",
        "process.terminate",
        "interactive batch-build server fallback closes only the owned direct "
        "child when no guard force-close hook is available",
    ),
    AllowedRawSubprocessUse(
        "tools/batch_compile_client.py",
        "BatchCompileServerClient.force_close",
        "process.kill",
        "interactive batch-build server fallback escalates only the owned direct "
        "child after terminate timeout when no guard force-close hook is available",
    ),
    AllowedRawSubprocessUse(
        "tools/bench.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; benchmark child execution uses MOLT_BENCH guard",
    ),
    AllowedRawSubprocessUse(
        "tools/bench_friends_output.py",
        "_git_rev",
        "run",
        "bounded git metadata probe for friend-suite report provenance; suite "
        "phases use guarded_completed_process",
    ),
    AllowedRawSubprocessUse(
        "tools/bench_wasm.py",
        "_git_rev",
        "run",
        "bounded git metadata probe; wasm build/run phases use MOLT_BENCH guard",
    ),
    AllowedRawSubprocessUse(
        "tools/build_graph_audit.py",
        "run_cargo_metadata",
        "run",
        "bounded cargo metadata probe; no build/test child execution",
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
        "tools/agent_coordination.py",
        "run_codex_stall_diagnostic",
        "process.terminate",
        "interactive Codex stall diagnostic closes only its direct launched child "
        "on KeyboardInterrupt; the launched command is tools/memory_guard.py by "
        "default and no name/process-group sweep is performed",
    ),
    AllowedRawSubprocessUse(
        "tools/agent_coordination.py",
        "run_codex_stall_diagnostic",
        "process.kill",
        "interactive Codex stall diagnostic escalates only the same direct "
        "launched child after terminate timeout; no Codex/Claude name sweep is "
        "performed",
    ),
    AllowedRawSubprocessUse(
        "tools/agent_coordination.py",
        "git_status_paths",
        "run",
        "bounded git status metadata probe used only to recommend focused proof "
        "lanes before agents start long-running work",
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
        "tools/bootstrap_llvm.py",
        "_run",
        "run",
        "bounded toolchain bootstrap command runner used by explicit LLVM setup",
    ),
    AllowedRawSubprocessUse(
        "tools/bootstrap_llvm.py",
        "_visual_studio_installation",
        "run",
        "bounded vswhere metadata probe for Windows LLVM/MSVC setup",
    ),
    AllowedRawSubprocessUse(
        "tools/bootstrap_llvm.py",
        "_windows_msvc_env",
        "run",
        "bounded VsDevCmd environment probe for Windows LLVM/MSVC setup",
    ),
    AllowedRawSubprocessUse(
        "tools/bootstrap_llvm.py",
        "_verify_llvm_config",
        "run",
        "bounded llvm-config version probe after explicit LLVM setup",
    ),
    AllowedRawSubprocessUse(
        "tools/check_perf_freshness.py",
        "_tracked_perf_artifacts",
        "run",
        "bounded git ls-files metadata probe for performance-artifact freshness",
    ),
    AllowedRawSubprocessUse(
        "tools/check_generator_manifest.py",
        "check_idempotence",
        "run",
        "bounded generator idempotence child with explicit timeout and captured output",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rustfmt.py",
        "_run_git",
        "run",
        "bounded git metadata probe selecting Rust files for repo-owned formatting",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rustfmt.py",
        "_merge_base",
        "run",
        "bounded git merge-base probe for committed-ahead changed Rust formatting",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rustfmt.py",
        "_upstream_ref",
        "run",
        "bounded git upstream probe for committed-ahead changed Rust formatting",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rustfmt.py",
        "_rustfmt_stdout",
        "run",
        "bounded rustfmt stdout child for no-op-safe Rust formatting writes",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rustfmt.py",
        "_run_rustfmt",
        "run",
        "bounded rustfmt check child for selected human Rust files",
    ),
    AllowedRawSubprocessUse(
        "tools/check_rust_toolchain.py",
        "_run",
        "run",
        "bounded git/rustup/rustc/cargo metadata probes for Rust toolchain contract checks",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/setup_readiness.py",
        "_build_toolchain_report",
        "run",
        "bounded rustc version metadata probe for setup-readiness diagnostics",
    ),
    AllowedRawSubprocessUse(
        "tools/dirty_tree_landing_audit.py",
        "_run_git",
        "run",
        "bounded git metadata/status probe for landing-safety diagnostics",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_codecs.py",
        "_rustfmt_rust_source",
        "run",
        "bounded rustfmt stdout child for generated codec Rust tables",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_stringprep_tables.py",
        "_rustfmt_text",
        "run",
        "bounded rustfmt child for checked-in generated Rust stringprep tables",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_wasm_abi.py",
        "_rustfmt",
        "run",
        "bounded rustfmt stdout child for one generated WASM ABI Rust module",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_wasm_abi.py",
        "_rustfmt_many",
        "run",
        "bounded rustfmt batch child for generated WASM ABI Rust modules",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_wasm_abi.py",
        "_rustfmt_version",
        "run",
        "bounded rustfmt version probe for generated WASM ABI cache keys",
    ),
    AllowedRawSubprocessUse(
        "tools/gen_protocol.py",
        "_format_generated_text",
        "run",
        "bounded ruff-format child for checked-in generated protocol text",
    ),
    AllowedRawSubprocessUse(
        "tools/molt_dev_probe.py",
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
        "tools/memory_guard_core/memory_limits.py",
        "_darwin_physical_memory_bytes",
        "run",
        "memory guard platform probe for adaptive host budgets",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/memory_limits.py",
        "_darwin_available_memory_bytes",
        "run",
        "memory guard vm_stat platform probe for adaptive host budgets",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/process_model.py",
        "sample_processes_posix",
        "run",
        "memory guard POSIX process sampler",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/windows_snapshot.py",
        "_windows_process_snapshot_rows_hard_timeout",
        "run",
        "memory guard Windows process snapshot helper with a hard timeout",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard.py",
        "_run_child_runner",
        "run",
        "Windows child-runner custody preserves the guarded child under "
        "tools/memory_guard.py when POSIX exec is unavailable",
    ),
    AllowedRawSubprocessUse(
        "tools/pact_witness_acceptance.py",
        "_run",
        "run",
        "named proof-queue lane runner; outer queue/memory guard owns timeout and process custody",
    ),
    AllowedRawSubprocessUse(
        "tools/pact_witness_oracle.py",
        "_run",
        "run",
        "bounded oracle helper used by Pact witness tooling under explicit runner custody",
    ),
    AllowedRawSubprocessUse(
        "tools/proof_queue.py",
        "_git_snapshot.run_git",
        "run",
        "bounded git snapshot probe recorded with every proof-queue row",
    ),
    AllowedRawSubprocessUse(
        "tools/proof_queue.py",
        "_run_one",
        "Popen",
        "proof queue custody boundary launching guarded proof commands with logs and contention keys",
    ),
    AllowedRawSubprocessUse(
        "tools/proof_queue.py",
        "_launch_detached_runner",
        "Popen",
        "proof queue detached runner custody boundary records run id, command, and log path",
    ),
    AllowedRawSubprocessUse(
        "tools/proof_queue.py",
        "_pid_alive",
        "os.kill",
        "bounded PID liveness probe for stale proof-queue row pruning",
    ),
    AllowedRawSubprocessUse(
        "tests/test_agent_contract_budget.py",
        "_tracked_agent_docs",
        "run",
        "bounded git ls-files metadata probe for agent-doc budget tests",
    ),
    AllowedRawSubprocessUse(
        "tests/test_generate_worker.py",
        "test_loader_bridge_enforces_manifest_reserved_callable_dispatch",
        "run",
        "bounded node fixture for generated worker loader manifest enforcement",
    ),
    AllowedRawSubprocessUse(
        "tests/tools/test_build_graph_audit.py",
        "test_cli_check_exits_zero_on_clean_tree",
        "run",
        "bounded subprocess smoke of build_graph_audit CLI",
    ),
    AllowedRawSubprocessUse(
        "tests/tools/test_dirty_tree_landing_audit.py",
        "_git",
        "run",
        "bounded synthetic-repository git helper for dirty-tree landing audit tests",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/source_extension_toolchain.py",
        "_probe_wasm_source_extension_compiler",
        "run",
        "bounded wasm source-extension compiler probe with explicit timeout",
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
        "tools/memory_guard_core/process_custody.py",
        "_pid_exited_or_unobservable",
        "os.kill",
        "memory guard pid-existence probe with signal 0 after a scoped "
        "termination attempt; not signal authority",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/process_custody.py",
        "_process_group_exited_or_unobservable",
        "os.killpg",
        "memory guard process-group existence probe with signal 0 after a "
        "scoped termination attempt; not signal authority",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/process_custody.py",
        "_send_pid_signal_action",
        "os.kill",
        "memory guard watched-root and escaped-PID signal primitive",
    ),
    AllowedRawSubprocessUse(
        "tools/memory_guard_core/process_custody.py",
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
        "tools/perf_authority.py",
        "_git_output",
        "run",
        "bounded git metadata probe for performance-authority provenance",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_authority.py",
        "git_rev_is_ancestor_of_origin",
        "run",
        "bounded git ancestry probe for performance-authority freshness",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_calibration.py",
        "_sample_peak_rss",
        "run",
        "bounded ps/tasklist metadata probe for calibration RSS reporting",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_calibration.py",
        "run_and_measure",
        "Popen",
        "calibration workload launcher owns its explicit benchmark child and "
        "records timing/RSS evidence",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_calibration.py",
        "run_and_measure",
        "process.kill",
        "calibration workload timeout cleanup kills only the owned benchmark "
        "child spawned by the same function",
    ),
    AllowedRawSubprocessUse(
        "tools/perf_calibration.py",
        "_competing_build_count",
        "run",
        "bounded tasklist/ps metadata probes for benchmark-noise reporting",
        expected_count=2,
    ),
    AllowedRawSubprocessUse(
        "tools/safe_run.py",
        "main",
        "run",
        "compatibility facade launches tools/memory_guard.py as the custody owner",
    ),
    AllowedRawSubprocessUse(
        "tools/check_subprocess_guard_coverage.py",
        "<module>",
        "shell.kill",
        "static checker pattern vocabulary for shell kill detection",
        expected_count=10,
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
        "tests/tools/test_subprocess_guard_coverage.py",
        "test_unclassified_makefile_pkill_fails",
        "shell.kill",
        "static checker fixture that proves unclassified Makefile process-kill "
        "strings fail",
    ),
    AllowedRawSubprocessUse(
        "tests/cli/test_cli_import_collection.py",
        "test_run_subprocess_captured_to_tempfiles_does_not_block_on_inherited_pipes",
        "os.kill",
        "bounded test cleanup for the subprocess pipe-drain regression child",
    ),
    AllowedRawSubprocessUse(
        "tools/uv_project_env.py",
        "run_command",
        "call",
        "bounded project-environment helper command runner",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/command_runtime.py",
        "_run_completed_command",
        "run",
        "CLI subprocess helper's explicit unguarded branch for opt-out call sites",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/arg_helpers.py",
        "_reexec_cli_with_hash_seed",
        "run",
        "Windows deterministic-PYTHONHASHSEED self-reexec path; POSIX uses "
        "execvpe and the restarted process preserves the same CLI custody path",
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
        "src/molt/process_guard.py",
        "run_completed_command",
        "run",
        "shared subprocess guard helper's explicit unguarded branch for opt-out call sites",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/backend_execution.py",
        "_start_backend_daemon",
        "Popen",
        "backend daemon start uses HarnessExecutionContext, process-group kwargs, "
        "and repo-sentinel startup custody",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/setup_readiness.py",
        "_llvm_config_matches_major",
        "run",
        "bounded llvm-config version probe for explicit toolchain validation",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/setup_readiness.py",
        "_clang_llvm_version_detail",
        "run",
        "bounded clang version probe for explicit toolchain validation",
    ),
    AllowedRawSubprocessUse(
        "src/molt/cli/setup_readiness.py",
        "_windows_vsdevcmd_path",
        "run",
        "bounded vswhere metadata probe for explicit Windows toolchain validation",
    ),
    AllowedRawSubprocessUse(
        "src/molt_accel/client.py",
        "MoltClient._ensure_process",
        "Popen",
        "stdio accelerator client starts only its configured worker command as a "
        "direct child and owns that child through MoltClient.close",
    ),
    AllowedRawSubprocessUse(
        "src/molt_accel/client.py",
        "MoltClient._close_locked",
        "process.terminate",
        "stdio accelerator client closes only its owned direct worker child; no "
        "name, ancestor, or process-group cleanup is performed",
    ),
    AllowedRawSubprocessUse(
        "src/molt_accel/client.py",
        "MoltClient._close_locked",
        "process.kill",
        "stdio accelerator client escalates only its owned direct worker child "
        "after terminate timeout",
    ),
    AllowedRawSubprocessUse(
        "packaging/bootstrap.py",
        "_install_wheel",
        "check_call",
        "explicit packaging bootstrap installs the already-built wheel into the "
        "repo-local bootstrap venv",
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
        method = self._os_signal_method(node)
        if method is not None:
            return method
        return self._process_object_signal_method(node)

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

    def _process_object_signal_method(self, node: ast.expr) -> str | None:
        if (
            isinstance(node, ast.Attribute)
            and node.attr in PROCESS_OBJECT_SIGNAL_METHODS
        ):
            return f"process.{node.attr}"
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


def _is_guard_text_file(path: Path) -> bool:
    return path.name in TEXT_FILE_NAMES or path.suffix in TEXT_FILE_SUFFIXES


def _iter_text_files(paths: Sequence[Path], *, root: Path) -> Iterable[Path]:
    seen: set[Path] = set()
    for raw_path in paths:
        path = raw_path if raw_path.is_absolute() else root / raw_path
        path = path.resolve()
        if path.is_file():
            candidates = [path]
        else:
            candidates = sorted(
                candidate
                for candidate in path.rglob("*")
                if candidate.is_file() and _is_guard_text_file(candidate)
            )
        for candidate in candidates:
            if (
                candidate in seen
                or not _is_guard_text_file(candidate)
                or _is_excluded(candidate, root=root)
            ):
                continue
            seen.add(candidate)
            yield candidate


def _scan_file(path: Path, *, root: Path) -> tuple[RawSubprocessCall, ...]:
    rel = _normalize_path(path, root=root)
    source_text = path.read_text(encoding="utf-8-sig")
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", SyntaxWarning)
        tree = ast.parse(source_text, filename=rel)
    visitor = _SubprocessVisitor(path=rel, source_text=source_text)
    visitor.visit(tree)
    return tuple(visitor.calls)


def _scan_text_file(path: Path, *, root: Path) -> tuple[RawSubprocessCall, ...]:
    rel = _normalize_path(path, root=root)
    source_text = path.read_text(encoding="utf-8", errors="replace")
    calls: list[RawSubprocessCall] = []
    for line_no, line in enumerate(source_text.splitlines(), start=1):
        if any(pattern in line for pattern in SHELL_KILL_PATTERNS):
            calls.append(
                RawSubprocessCall(
                    path=rel,
                    line=line_no,
                    qualname="<text>",
                    method="shell.kill",
                    source=" ".join(line.strip().split()),
                )
            )
    return tuple(calls)


def audit_paths(
    paths: Sequence[Path] = DEFAULT_TARGETS,
    *,
    root: Path = REPO_ROOT,
    allowlist: Sequence[AllowedRawSubprocessUse] = ALLOWLIST,
    text_paths: Sequence[Path] | None = None,
) -> SubprocessGuardAudit:
    if text_paths is None:
        text_paths = DEFAULT_TEXT_TARGETS if tuple(paths) == DEFAULT_TARGETS else ()
    scanned_files = 0
    raw_calls: list[RawSubprocessCall] = []
    for path in _iter_python_files(paths, root=root):
        scanned_files += 1
        raw_calls.extend(_scan_file(path, root=root))
    for path in _iter_text_files(text_paths, root=root):
        scanned_files += 1
        raw_calls.extend(_scan_text_file(path, root=root))

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
