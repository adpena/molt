from __future__ import annotations

import datetime as dt
import os
import shlex
import shutil
import sys
import time
from pathlib import Path
from typing import Any, Literal, Mapping, Sequence

from molt.cli.atomic_io import _write_json_sidecar
from molt.cli.backend_diagnostics import _FALSY_ENV_VALUES
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _load_cli_harness_memory_guard,
    _run_completed_command,
)
from molt.cli.env_paths import _base_env
from molt.cli.models import _MaintenanceStep, _ValidationStep
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_molt_root, _require_molt_root

from molt.cli.setup_readiness import (
    _canonical_env_defaults,
    _detect_llvm_backend_toolchain,
    _required_llvm_backend_pin,
)

_VALIDATE_PROOF_BYPASS_ENV = frozenset(
    {
        "MOLT_SKIP_BINARY_VALIDITY_CHECK",
        "MOLT_SKIP_CARGO_LOCK",
        "MOLT_SKIP_RUNTIME_REBUILD",
    }
)
_VALIDATE_SUITE_CHOICES = (
    "full",
    "smoke",
    "commands",
    "conformance",
    "bench",
    "custody-proof",
)


def _planned_update_steps(
    root: Path,
    *,
    include_toolchains: bool,
    include_locks: bool,
    include_manifests: bool,
) -> tuple[list[_MaintenanceStep], list[str]]:
    steps: list[_MaintenanceStep] = []
    warnings: list[str] = []

    if include_toolchains:
        if shutil.which("rustup"):
            steps.extend(
                [
                    _MaintenanceStep(
                        "rustup-update-stable",
                        ["rustup", "update", "stable"],
                        root,
                        "toolchain",
                    ),
                    _MaintenanceStep(
                        "rustup-target-add-wasm32-unknown-unknown",
                        ["rustup", "target", "add", "wasm32-unknown-unknown"],
                        root,
                        "toolchain",
                    ),
                    _MaintenanceStep(
                        "rustup-target-add-wasm32-wasip1",
                        ["rustup", "target", "add", "wasm32-wasip1"],
                        root,
                        "toolchain",
                    ),
                ]
            )
        else:
            warnings.append(
                "rustup is not installed; skipping Rust toolchain refresh steps"
            )
        if shutil.which("cargo"):
            cargo_tool_steps: list[tuple[str, str, str]] = [
                ("wasm-tools", "wasm-tools", "wasm-tools"),
                ("wasm-pack", "wasm-pack", "wasm-pack"),
            ]
            for tool_name, crate_name, command_name in cargo_tool_steps:
                if shutil.which(command_name):
                    continue
                steps.append(
                    _MaintenanceStep(
                        f"cargo-install-{tool_name}",
                        ["cargo", "install", crate_name, "--locked"],
                        root,
                        "toolchain",
                    )
                )
            llvm_major, llvm_toolchain = _detect_llvm_backend_toolchain(root)
            if llvm_major is not None and llvm_toolchain is None:
                llvm_pin = _required_llvm_backend_pin(root)
                release = (
                    llvm_pin.default_release
                    if llvm_pin is not None
                    else f"{llvm_major}.1.0"
                )
                env_var = (
                    llvm_pin.env_var
                    if llvm_pin is not None
                    else f"LLVM_SYS_{llvm_major * 10 + 1}_PREFIX"
                )
                warnings.append(
                    "LLVM backend toolchain is missing; run "
                    f"python tools/bootstrap_llvm.py --version {release} "
                    f"--prefix target/toolchains/llvm-{release} and set "
                    f"{env_var} to that prefix"
                )
        else:
            warnings.append(
                "cargo is not installed; skipping cargo-installable toolchain helpers"
            )

    if include_locks:
        steps.extend(
            [
                _MaintenanceStep(
                    "cargo-update-root",
                    ["cargo", "update", "--manifest-path", "Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "cargo-update-runtime",
                    ["cargo", "update", "--manifest-path", "runtime/Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "cargo-update-fuzz",
                    ["cargo", "update", "--manifest-path", "fuzz/Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "uv-lock-upgrade",
                    ["uv", "lock", "-U"],
                    root,
                    "lock",
                ),
            ]
        )

    if include_manifests:
        if shutil.which("cargo-upgrade") is None:
            steps.append(
                _MaintenanceStep(
                    "cargo-edit-bootstrap",
                    ["cargo", "install", "cargo-edit", "--locked"],
                    root,
                    "manifest",
                )
            )
        steps.extend(
            [
                _MaintenanceStep(
                    "cargo-upgrade-root",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
                _MaintenanceStep(
                    "cargo-upgrade-runtime",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "runtime/Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
                _MaintenanceStep(
                    "cargo-upgrade-fuzz",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "fuzz/Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
            ]
        )

    return steps, warnings


def update_repo(
    *,
    json_output: bool = False,
    verbose: bool = False,
    check_only: bool = False,
    include_toolchains: bool = True,
    include_locks: bool = True,
    include_manifests: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "update")
    if root_error is not None:
        return root_error

    steps, warnings = _planned_update_steps(
        root,
        include_toolchains=include_toolchains,
        include_locks=include_locks,
        include_manifests=include_manifests,
    )
    step_rows = [
        {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
        }
        for step in steps
    ]

    if check_only:
        payload = _json_payload(
            "update",
            "ok",
            data={
                "root": str(root),
                "check_only": True,
                "steps": step_rows,
            },
            warnings=warnings,
        )
        if json_output:
            _emit_json(payload, json_output=True)
        else:
            print(f"Update plan for {root}:")
            for row in step_rows:
                print(f"- [{row['category']}] {row['name']}: {shlex.join(row['cmd'])}")
            for warning in warnings:
                print(f"warning: {warning}", file=sys.stderr)
        return 0

    results: list[dict[str, Any]] = []
    for step in steps:
        if verbose and not json_output:
            print(f"[molt update] {step.name}: {shlex.join(step.cmd)}", file=sys.stderr)
        proc = _run_completed_command(
            step.cmd,
            cwd=step.cwd,
            capture_output=True,
            env=None,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        entry: dict[str, Any] = {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "returncode": proc.returncode,
        }
        if proc.stdout:
            entry["stdout"] = proc.stdout
        if proc.stderr:
            entry["stderr"] = proc.stderr
        results.append(entry)
        if proc.returncode != 0:
            payload = _json_payload(
                "update",
                "error",
                data={
                    "root": str(root),
                    "check_only": False,
                    "steps": step_rows,
                    "results": results,
                },
                warnings=warnings,
                errors=[
                    f"{step.name} failed with exit code {proc.returncode}",
                ],
            )
            if json_output:
                _emit_json(payload, json_output=True)
            else:
                print(
                    f"molt update failed at {step.name}: {shlex.join(step.cmd)}",
                    file=sys.stderr,
                )
                if proc.stderr:
                    print(proc.stderr, file=sys.stderr, end="")
            return proc.returncode or 1

    payload = _json_payload(
        "update",
        "ok",
        data={
            "root": str(root),
            "check_only": False,
            "steps": step_rows,
            "results": results,
        },
        warnings=warnings,
    )
    if json_output:
        _emit_json(payload, json_output=True)
    else:
        print(f"Updated toolchains/dependencies for {root}")
        for result in results:
            print(
                f"- [{result['category']}] {result['name']} (rc={result['returncode']})"
            )
    return 0


def _planned_validate_steps(
    root: Path,
    *,
    suite: Literal[
        "full",
        "smoke",
        "commands",
        "conformance",
        "bench",
        "custody-proof",
    ],
    backend: Literal["all", "native", "llvm", "wasm", "luau"],
    profile: Literal["all", "dev", "release"],
) -> list[_ValidationStep]:
    python = sys.executable
    bench_profile = "release" if profile == "all" else profile
    build_profile = "release" if profile == "all" else profile
    steps: list[_ValidationStep] = [
        _ValidationStep(
            "cli-run-json",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/cli/test_cli_smoke.py",
                "-k",
                "test_cli_run_json",
            ],
            root,
            "command",
            ("native",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "cli-command-json",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/cli/test_cli_smoke.py",
                "-k",
                (
                    "test_cli_build_json_binary_executes_for_native_profiles "
                    "or test_cli_compare_json "
                    "or test_cli_run_exec_eval_raise_runtime_error"
                ),
            ],
            root,
            "command",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "subprocess-guard-audit",
            [
                python,
                "tools/check_subprocess_guard_coverage.py",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "memory-guard-wiring-audit",
            [
                python,
                "tools/check_memory_guard_wiring.py",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "custody-proof",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_memory_guard_wiring.py",
                "tests/tools/test_memory_guard_windows_sampling.py",
                "tests/tools/test_process_sentinel.py",
                "tests/cli/test_cli_smoke.py::test_cli_hash_seed_windows_handoff_waits_for_restarted_process",
                "tests/cli/test_cli_smoke.py::test_cli_hash_seed_reexec_argv_uses_active_python_executable",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "native-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_native_lir_loop_join_semantics.py",
                "-k",
                "not llvm",
            ],
            root,
            "correctness",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "llvm-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_native_lir_loop_join_semantics.py",
                "-k",
                "llvm_simple_exception_catch or llvm_exception_loop",
            ],
            root,
            "correctness",
            ("llvm",),
            ("release",),
            "smoke",
        ),
        _ValidationStep(
            "wasm-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_wasm_control_flow.py",
                "tests/test_wasm_class_smoke.py",
                "-k",
                "preserves_type_name or wasm_module_try_exception_loop_parity",
            ],
            root,
            "correctness",
            ("wasm",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-support-matrix",
            [
                python,
                "tools/gen_luau_support_matrix.py",
                "--check",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-compile-smoke",
            [
                python,
                "-m",
                "molt.cli",
                "build",
                "examples/hello.py",
                "--target",
                "luau",
                "--profile",
                build_profile,
                "--output",
                str(root / "tmp" / "validate" / "luau-smoke" / "hello.luau"),
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-runner-available",
            [
                python,
                "-c",
                (
                    "import shutil, sys; "
                    "runner = shutil.which('luau') or shutil.which('lune'); "
                    "print(runner) if runner else sys.exit("
                    "'luau or lune is required for Luau validation')"
                ),
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-ord-at-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_ord_at_native.py",
                "-k",
                "luau",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "native-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "native-backend",
                "--test",
                "entry_block_param_shadow",
                "--test",
                "lir_loop_and_join_regressions",
                "--test",
                "native_extern_linkage",
                "--test",
                "ir_contract_validation",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "wasm-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "wasm-backend",
                "--test",
                "lir_wasm_repr_regressions",
                "--test",
                "wasm_lir_fast_path_integration",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("wasm",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend-luau",
                "--features",
                "luau-backend",
                "--lib",
                "luau::tests::",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-lowering-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend-luau",
                "--features",
                "luau-backend",
                "--lib",
                "luau_lower::tests::",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "conformance-smoke",
            [
                python,
                "tests/harness/run_molt_conformance.py",
                "--suite",
                "smoke",
                "--json-out",
                str(root / "logs" / "validate-conformance-smoke.json"),
            ],
            root,
            "conformance",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "conformance-full",
            [
                python,
                "tests/harness/run_molt_conformance.py",
                "--suite",
                "full",
                "--json-out",
                str(root / "logs" / "validate-conformance-full.json"),
            ],
            root,
            "conformance",
            ("native",),
            ("dev", "release"),
            "full",
        ),
        _ValidationStep(
            "bench-smoke",
            [
                python,
                "tools/bench.py",
                "--smoke",
                "--warmup",
                "1",
                "--molt-profile",
                bench_profile,
                "--json-out",
                str(root / "bench" / "results" / "validate-bench-smoke.json"),
            ],
            root,
            "benchmark",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "bench-full",
            [
                python,
                "tools/bench.py",
                "--molt-profile",
                bench_profile,
                "--json-out",
                str(root / "bench" / "results" / "validate-bench-full.json"),
            ],
            root,
            "benchmark",
            ("native",),
            ("dev", "release"),
            "full",
        ),
    ]

    selected: list[_ValidationStep] = []
    for step in steps:
        if suite == "custody-proof" and step.name != "custody-proof":
            continue
        if suite == "commands" and step.category != "command":
            continue
        if suite == "conformance" and step.category != "conformance":
            continue
        if suite == "bench" and step.category != "benchmark":
            continue
        if suite == "smoke" and step.suite != "smoke":
            continue
        if (
            suite == "full"
            and step.suite == "smoke"
            and step.category
            in {
                "conformance",
                "benchmark",
            }
        ):
            continue
        if backend != "all" and backend not in step.backends:
            continue
        if profile != "all" and profile not in step.profiles:
            continue
        selected.append(step)
    return selected


def _validate_guard_prefix(step: _ValidationStep) -> str:
    if step.category == "benchmark":
        return "MOLT_BENCH"
    if step.category == "conformance":
        return "MOLT_CONFORMANCE"
    return "MOLT_TEST_SUITE"


def _validation_guard_summary(
    root: Path,
    env: Mapping[str, str],
    steps: Sequence[_ValidationStep],
) -> dict[str, Any]:
    harness_memory_guard = _load_cli_harness_memory_guard(root)
    prefixes = sorted({_validate_guard_prefix(step) for step in steps})
    summary: dict[str, Any] = {}
    for prefix in prefixes:
        limits = harness_memory_guard.limits_from_env(prefix, env)
        summary[prefix] = harness_memory_guard.limits_summary(limits)
    return summary


def _format_validate_guard_summary(prefix: str, limits: Mapping[str, Any]) -> str:
    def gb(name: str) -> str:
        value = limits[name]
        return f"{value:.2f}" if isinstance(value, float) else str(value)

    return (
        f"- {prefix}: process={gb('max_process_rss_gb')}GB "
        f"tree={gb('max_total_rss_gb')}GB "
        f"global={gb('max_global_rss_gb')}GB "
        f"child_rlimit={gb('child_rlimit_gb')}GB"
    )


def _default_validate_summary_path(
    root: Path,
    *,
    suite: str,
    backend: str,
    profile: str,
) -> Path:
    return root / "logs" / f"validate-{suite}-{backend}-{profile}.json"


def _resolve_validate_summary_path(root: Path, summary_out: str | None) -> Path:
    if summary_out is None:
        raise ValueError("summary_out must not be None")
    path = Path(summary_out).expanduser()
    if not path.is_absolute():
        path = root / path
    return path


def _persist_validate_summary(
    payload: dict[str, Any],
    *,
    summary_path: Path | None,
) -> str | None:
    if summary_path is None:
        return None
    payload["data"]["summary_path"] = str(summary_path)
    try:
        _write_json_sidecar(summary_path, payload)
    except OSError as exc:
        return f"Failed to write validate summary {summary_path}: {exc}"
    return None


def _validate_proof_bypass_errors(env: Mapping[str, str]) -> list[str]:
    errors: list[str] = []
    for key in sorted(_VALIDATE_PROOF_BYPASS_ENV):
        value = env.get(key)
        if value is None or value.strip().lower() in _FALSY_ENV_VALUES:
            continue
        errors.append(
            f"{key}={value} disables a validation proof gate; unset it before running molt validate."
        )
    return errors


def validate(
    *,
    suite: Literal[
        "full",
        "smoke",
        "commands",
        "conformance",
        "bench",
        "custody-proof",
    ] = "full",
    backend: Literal["all", "native", "llvm", "wasm", "luau"] = "all",
    profile: Literal["all", "dev", "release"] = "all",
    json_output: bool = False,
    verbose: bool = False,
    check_only: bool = False,
    summary_out: str | None = None,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "validate")
    if root_error is not None:
        return root_error
    bypass_errors = _validate_proof_bypass_errors(os.environ)
    if bypass_errors:
        return _fail(" ".join(bypass_errors), json_output, command="validate")
    steps = _planned_validate_steps(
        root,
        suite=suite,
        backend=backend,
        profile=profile,
    )
    if not steps:
        return _fail(
            "No validation steps matched the requested filters.",
            json_output,
            command="validate",
        )
    step_rows = [
        {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "memory_guard_prefix": _validate_guard_prefix(step),
            "backends": list(step.backends),
            "profiles": list(step.profiles),
            "suite": step.suite,
        }
        for step in steps
    ]
    env = _base_env(root, molt_root=root)
    for key, value in _canonical_env_defaults(root).items():
        env.setdefault(key, value)
    env.setdefault("MOLT_SESSION_ID", "validate")
    guard_summary = _validation_guard_summary(root, env, steps)
    summary_path = (
        _resolve_validate_summary_path(root, summary_out)
        if summary_out is not None
        else (
            None
            if check_only
            else _default_validate_summary_path(
                root,
                suite=suite,
                backend=backend,
                profile=profile,
            )
        )
    )
    started_at = dt.datetime.now(dt.timezone.utc).isoformat()
    validate_started = time.perf_counter()
    if check_only:
        payload = _json_payload(
            "validate",
            "ok",
            data={
                "check_only": True,
                "started_at": started_at,
                "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                "elapsed_s": round(time.perf_counter() - validate_started, 6),
                "suite": suite,
                "backend": backend,
                "profile": profile,
                "steps": step_rows,
                "memory_guard": guard_summary,
            },
        )
        summary_error = _persist_validate_summary(payload, summary_path=summary_path)
        if summary_error is not None:
            payload["status"] = "error"
            payload["errors"].append(summary_error)
        if json_output:
            _emit_json(payload, json_output=True)
        else:
            print("Validation plan:")
            for row in step_rows:
                print(f"- [{row['category']}] {row['name']}: {shlex.join(row['cmd'])}")
            print("Memory guard:")
            for prefix, limits in guard_summary.items():
                print(_format_validate_guard_summary(prefix, limits))
            if summary_path is not None:
                print(f"Summary: {summary_path}")
        return 1 if summary_error is not None else 0

    results: list[dict[str, Any]] = []
    for step in steps:
        if verbose and not json_output:
            print(
                f"[molt validate] {step.name}: {shlex.join(step.cmd)}",
                file=sys.stderr,
            )
        guard_prefix = _validate_guard_prefix(step)
        start = time.perf_counter()
        proc = _run_completed_command(
            [str(part) for part in step.cmd],
            cwd=step.cwd,
            env=env,
            capture_output=True,
            memory_guard_prefix=guard_prefix,
        )
        duration_s = round(time.perf_counter() - start, 6)
        entry: dict[str, Any] = {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "returncode": proc.returncode,
            "duration_s": duration_s,
        }
        if proc.stdout:
            entry["stdout"] = proc.stdout
        if proc.stderr:
            entry["stderr"] = proc.stderr
        results.append(entry)
        if verbose and not json_output:
            print(
                f"[molt validate] {step.name} finished "
                f"(rc={proc.returncode}, {duration_s:.2f}s)",
                file=sys.stderr,
            )
        if proc.returncode != 0:
            finished_at = dt.datetime.now(dt.timezone.utc).isoformat()
            payload = _json_payload(
                "validate",
                "error",
                data={
                    "check_only": False,
                    "started_at": started_at,
                    "finished_at": finished_at,
                    "elapsed_s": round(time.perf_counter() - validate_started, 6),
                    "suite": suite,
                    "backend": backend,
                    "profile": profile,
                    "steps": step_rows,
                    "results": results,
                    "memory_guard": guard_summary,
                },
                errors=[f"{step.name} failed with exit code {proc.returncode}"],
            )
            summary_error = _persist_validate_summary(
                payload,
                summary_path=summary_path,
            )
            if summary_error is not None:
                payload["errors"].append(summary_error)
            if json_output:
                _emit_json(payload, json_output=True)
            else:
                print(
                    f"molt validate failed at {step.name}: {shlex.join(step.cmd)}",
                    file=sys.stderr,
                )
                if proc.stderr:
                    print(proc.stderr, file=sys.stderr, end="")
                if summary_path is not None:
                    print(f"Summary: {summary_path}", file=sys.stderr)
            return proc.returncode or 1

    finished_at = dt.datetime.now(dt.timezone.utc).isoformat()
    payload = _json_payload(
        "validate",
        "ok",
        data={
            "check_only": False,
            "started_at": started_at,
            "finished_at": finished_at,
            "elapsed_s": round(time.perf_counter() - validate_started, 6),
            "suite": suite,
            "backend": backend,
            "profile": profile,
            "steps": step_rows,
            "results": results,
            "memory_guard": guard_summary,
        },
    )
    summary_error = _persist_validate_summary(payload, summary_path=summary_path)
    if summary_error is not None:
        payload["status"] = "error"
        payload["errors"].append(summary_error)
    if json_output:
        _emit_json(payload, json_output=True)
    else:
        print("Validation succeeded:")
        for result in results:
            print(
                f"- [{result['category']}] {result['name']} "
                f"(rc={result['returncode']}, {result['duration_s']:.2f}s)"
            )
        print("Memory guard:")
        for prefix, limits in guard_summary.items():
            print(_format_validate_guard_summary(prefix, limits))
        if summary_path is not None:
            print(f"Summary: {summary_path}")
    return 1 if summary_error is not None else 0
