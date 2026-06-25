#!/usr/bin/env python3
"""Unified CI gate with tiered verification pipeline.

Runs ALL correctness verification tools in dependency order across three tiers:

  Tier 1 — Fast (< 60s, every commit):
    Linting, formatting, correspondence checks, layout checks, coverage
    analysis, perf-scoreboard contract checks, property/mutation/fuzz smoke
    tests.

  Tier 2 — Medium (< 10min, on PR):
    Quint simulation, translation validation, full property tests,
    reproducible build spot-check.

  Tier 3 — Heavy (< 60min, nightly/weekly):
    Full formal methods (Lean + Quint), deep reproducibility sweep,
    extended fuzzing, mutation testing, model-based tests.

Usage:
    uv run --python 3.12 python3 tools/ci_gate.py
    uv run --python 3.12 python3 tools/ci_gate.py --tier 2
    uv run --python 3.12 python3 tools/ci_gate.py --tier all --parallel
    uv run --python 3.12 python3 tools/ci_gate.py --tier 1 --json
    uv run --python 3.12 python3 tools/ci_gate.py --dry-run
    uv run --python 3.12 python3 tools/ci_gate.py --tier 2 --fail-fast
"""

from __future__ import annotations

import argparse
import contextlib
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

_THIS_FILE = Path(__file__).resolve()
_REPO_ROOT = _THIS_FILE.parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

import tools.memory_guard as memory_guard  # noqa: E402
import tools.harness_memory_guard as harness_memory_guard  # noqa: E402
import tools.compile_governor as compile_governor  # noqa: E402

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

ROOT = _REPO_ROOT
CI_GATE = _THIS_FILE
TOOLS = ROOT / "tools"
TESTS = ROOT / "tests"
LOG_ROOT = ROOT / "logs" / "ci_gate"

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def yellow(t: str) -> str:
    return _c("33", t)


def bold(t: str) -> str:
    return _c("1", t)


def dim(t: str) -> str:
    return _c("2", t)


# ---------------------------------------------------------------------------
# Check definition
# ---------------------------------------------------------------------------


@dataclass
class Check:
    """A single verification step."""

    name: str
    tier: int
    cmd: list[str]
    cwd: str | None = None
    env_extra: dict[str, str] = field(default_factory=dict)
    timeout: int = 300  # seconds
    required: bool = True  # False = continue-on-error
    needs_rust: bool = False
    needs_lean: bool = False
    needs_quint: bool = False
    needs_pytest: bool = False


@dataclass
class CheckResult:
    name: str
    tier: int
    status: str  # "pass", "fail", "skip", "error"
    duration_s: float = 0.0
    returncode: int = 0
    stdout: str = ""
    stderr: str = ""
    skip_reason: str = ""


MemoryGuardLimits = harness_memory_guard.HarnessMemoryLimits


_DEFAULT_MEMORY_LIMITS = object()


def default_memory_guard_limits(
    *,
    prefix: str = "MOLT_CI_GATE",
    environ: Mapping[str, str] | None = None,
) -> MemoryGuardLimits:
    return harness_memory_guard.limits_from_env(prefix, environ)


def _resolve_memory_limits(
    limits: MemoryGuardLimits | None | object,
) -> MemoryGuardLimits:
    if limits is _DEFAULT_MEMORY_LIMITS or limits is None:
        resolved = default_memory_guard_limits()
    elif isinstance(limits, MemoryGuardLimits):
        resolved = limits
    else:
        raise TypeError(
            "memory limits must be MemoryGuardLimits, None, or the default sentinel"
        )
    if not resolved.enabled:
        return replace(resolved, enabled=True)
    return resolved


@dataclass(frozen=True, slots=True)
class BackgroundGateMetadata:
    pid: int
    command: list[str]
    log_path: Path
    metadata_path: Path
    cwd: Path
    created_at: str


# ---------------------------------------------------------------------------
# UV / tool helpers
# ---------------------------------------------------------------------------

_UV = shutil.which("uv") or "uv"
_PYTHON = "3.12"


def _uv_run(*args: str) -> list[str]:
    """Build a 'uv run --python 3.12 python3 ...' command."""
    return [_UV, "run", "--python", _PYTHON, "python3", *args]


def _uv_pytest(*args: str) -> list[str]:
    """Build a 'uv run --python 3.12 pytest ...' command."""
    return [_UV, "run", "--python", _PYTHON, "pytest", *args]


def _has_tool(name: str) -> bool:
    return shutil.which(name) is not None


# ---------------------------------------------------------------------------
# Check registry
# ---------------------------------------------------------------------------


def _build_checks() -> list[Check]:
    """Return all checks, all tiers."""
    checks: list[Check] = []

    # ── Tier 1: Fast (< 60s, every commit) ─────────────────────────────

    checks.append(
        Check(
            name="ruff-check",
            tier=1,
            cmd=[_UV, "run", "--python", _PYTHON, "ruff", "check", "."],
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="ruff-format",
            tier=1,
            cmd=[_UV, "run", "--python", _PYTHON, "ruff", "format", "--check", "."],
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="cargo-fmt",
            tier=1,
            cmd=["cargo", "fmt", "--check"],
            timeout=60,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="cargo-clippy",
            tier=1,
            cmd=["cargo", "clippy", "--", "-D", "warnings"],
            timeout=120,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="correspondence-check",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_correspondence.py"), "--json"),
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="differential-suite-layout",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_differential_suite_layout.py")),
            timeout=30,
        )
    )
    checks.append(
        Check(
            name="diff-coverage-analysis",
            tier=1,
            cmd=_uv_run(str(TOOLS / "diff_coverage_analysis.py"), "--json"),
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="structural-audit-ratchet",
            tier=1,
            cmd=_uv_run(str(TOOLS / "structural_audit.py"), "--check"),
            timeout=60,
        )
    )
    checks.append(
        Check(
            # Fail closed if the canonical perf gate is ever un-wired from main
            # again (a gate that never fires certifies nothing -- the TIER-0
            # proxy-measurement meta-bug).
            name="perf-gate-wiring",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_perf_gate_wiring.py")),
            timeout=30,
        )
    )
    checks.append(
        Check(
            name="analysis-capsule-contract",
            tier=1,
            cmd=_uv_pytest(str(TESTS / "tools" / "test_analysis_capsule.py"), "-q"),
            timeout=60,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="perf-scoreboard-contract",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "tools" / "test_perf_causality.py"),
                str(TESTS / "tools" / "test_pass_delta_dashboard.py"),
                str(TESTS / "tools" / "test_perf_schema.py"),
                str(TESTS / "tools" / "test_perf_scoreboard.py"),
                str(TESTS / "tools" / "test_perf_authority.py"),
                "-q",
            ),
            timeout=120,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="perf-doc-freshness",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_perf_freshness.py")),
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="property-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "property"),
                "-x",
                "--molt-max-examples=10",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="mutation-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "mutation" / "test_mutation_smoke.py"),
                "-x",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="fuzz-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "fuzz" / "test_fuzz_smoke.py"),
                "-x",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )

    # ── Tier 2: Medium (< 10min, on PR) ────────────────────────────────

    checks.append(
        Check(
            name="formal-methods-quint-only",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "check_formal_methods.py"),
                "--skip-build",
            ),
            timeout=120,
            needs_quint=True,
        )
    )
    checks.append(
        Check(
            name="translation-validate-core",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "translation_validate.py"),
                "--json",
                str(TESTS / "differential" / "basic" / "core_types"),
            ),
            timeout=300,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="property-tests-full",
            tier=2,
            cmd=_uv_pytest(
                str(TESTS / "property"),
                "--molt-max-examples=200",
                "-q",
            ),
            timeout=300,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="reproducible-build-spot",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "verify_reproducible.py"),
                "--runs",
                "2",
                "--programs",
                "examples/hello.py",
                "--object",
            ),
            timeout=300,
            needs_rust=True,
        )
    )

    # ── Tier 3: Heavy (< 60min, nightly/weekly) ────────────────────────

    checks.append(
        Check(
            name="formal-methods-full",
            tier=3,
            cmd=_uv_run(str(TOOLS / "check_formal_methods.py")),
            timeout=1200,
            needs_lean=True,
            needs_quint=True,
        )
    )
    checks.append(
        Check(
            name="reproducible-build-sweep",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "verify_reproducible.py"),
                "--runs",
                "5",
                "--object",
            ),
            timeout=600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="fuzz-compiler-extended",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "fuzz_compiler.py"),
                "--count",
                "100",
                "--timeout",
                "300",
            ),
            timeout=600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="mutation-test-extended",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "mutation_test.py"),
                "--max-mutations",
                "50",
                "--timeout",
                "60",
                "--no-fail",
            ),
            timeout=3600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="translation-validate-full",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "translation_validate.py"),
                "--json",
                str(TESTS / "differential"),
            ),
            timeout=1800,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="model-based-tests",
            tier=3,
            cmd=_uv_pytest(
                str(TESTS / "model_based"),
                "-x",
                "-q",
            ),
            timeout=600,
            needs_pytest=True,
        )
    )

    return checks


# ---------------------------------------------------------------------------
# Execution engine
# ---------------------------------------------------------------------------


def _skip_reason(check: Check) -> str | None:
    """Return a skip reason if prerequisites are missing, else None."""
    if check.needs_rust and not _has_tool("cargo"):
        return "cargo not found"
    if check.needs_lean and not _has_tool("lake"):
        return "lake (Lean 4) not found"
    if check.needs_quint and not _has_tool("quint"):
        return "quint not found"
    # Check that tool script exists for uv-run checks
    if check.cmd and len(check.cmd) > 4 and check.cmd[0] == _UV:
        script = check.cmd[4] if len(check.cmd) > 4 else None
        if script and script.startswith(str(TOOLS)) and not Path(script).exists():
            return f"script not found: {script}"
    # Check test directories for pytest checks — find the first arg that
    # looks like a path (after the "pytest" token in the command list).
    if check.needs_pytest:
        try:
            pytest_idx = check.cmd.index("pytest")
            for arg in check.cmd[pytest_idx + 1 :]:
                if arg.startswith("-"):
                    continue
                if not Path(arg).exists():
                    return f"test path not found: {arg}"
                break
        except ValueError:
            pass
    return None


def _check_env(check: Check) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["PYTHONUNBUFFERED"] = "1"
    env.update(check.env_extra)
    _apply_canonical_env_defaults(env)
    return env


def _apply_canonical_env_defaults(env: dict[str, str]) -> None:
    ext_root = Path(env.setdefault("MOLT_EXT_ROOT", str(ROOT))).expanduser()
    cargo_target_dir = env.setdefault("CARGO_TARGET_DIR", str(ext_root / "target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", cargo_target_dir)
    env.setdefault("CARGO_INCREMENTAL", "0")
    env.setdefault("MOLT_CACHE", str(ext_root / ".molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "tmp" / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / ".uv-cache"))
    env.setdefault("TMPDIR", str(ext_root / "tmp"))
    env.setdefault("MOLT_SESSION_ID", f"ci-gate-{os.getpid()}")
    env.setdefault("CARGO_BUILD_JOBS", "2")
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
    ):
        with contextlib.suppress(OSError):
            Path(env[key]).expanduser().mkdir(parents=True, exist_ok=True)


def _truncate_output(text: str) -> str:
    return text[-4096:] if len(text) > 4096 else text


def _status_from_process_result(
    *,
    returncode: int,
    violation: memory_guard.RssViolation | None = None,
    timed_out: bool = False,
) -> str:
    if (
        timed_out
        or violation is not None
        or returncode
        in {
            memory_guard.GUARD_RETURN_CODE,
            memory_guard.TIMEOUT_RETURN_CODE,
        }
    ):
        return "error"
    return "pass" if returncode == 0 else "fail"


def _run_check(
    check: Check,
    dry_run: bool = False,
    memory_limits: MemoryGuardLimits | None | object = _DEFAULT_MEMORY_LIMITS,
) -> CheckResult:
    """Execute a single check and return the result."""
    skip = _skip_reason(check)
    if skip:
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="skip",
            skip_reason=skip,
        )

    if dry_run:
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="skip",
            skip_reason="dry-run",
        )

    resolved_memory_limits = _resolve_memory_limits(memory_limits)
    env = _check_env(check)
    cwd = check.cwd or str(ROOT)
    slot_context = (
        compile_governor.compile_slot(env=env, label=f"ci_gate:{check.name}")
        if check.needs_rust
        else contextlib.nullcontext()
    )

    start = time.monotonic()
    try:
        with slot_context:
            guarded = harness_memory_guard.guarded_completed_process(
                check.cmd,
                prefix="MOLT_CI_GATE",
                cwd=cwd,
                env=env,
                timeout=check.timeout,
                capture_output=True,
                text=True,
                limits=resolved_memory_limits,
            )
            returncode = guarded.returncode
            stdout = guarded.stdout or ""
            stderr = guarded.stderr or ""
            status = _status_from_process_result(
                returncode=guarded.returncode,
            )
        duration = time.monotonic() - start
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status=status,
            duration_s=round(duration, 2),
            returncode=returncode,
            stdout=_truncate_output(stdout),
            stderr=_truncate_output(stderr),
        )
    except subprocess.TimeoutExpired:
        duration = time.monotonic() - start
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="error",
            duration_s=round(duration, 2),
            returncode=-1,
            stderr=f"timeout after {check.timeout}s",
        )
    except Exception as exc:  # noqa: BLE001
        duration = time.monotonic() - start
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="error",
            duration_s=round(duration, 2),
            returncode=-1,
            stderr=str(exc),
        )


def _status_icon(status: str) -> str:
    icons = {
        "pass": green("PASS"),
        "fail": red("FAIL"),
        "skip": yellow("SKIP"),
        "error": red("ERR "),
    }
    return icons.get(status, status)


def _print_result(result: CheckResult, verbose: bool = False) -> None:
    icon = _status_icon(result.status)
    timing = dim(f"({result.duration_s:.1f}s)") if result.duration_s > 0 else ""
    skip_info = dim(f" [{result.skip_reason}]") if result.skip_reason else ""
    print(f"  {icon}  {result.name} {timing}{skip_info}")
    if verbose and result.status in ("fail", "error"):
        if result.stderr:
            for line in result.stderr.strip().splitlines()[-10:]:
                print(f"         {dim(line)}")


# ---------------------------------------------------------------------------
# Main orchestrator
# ---------------------------------------------------------------------------


def _parallel_workers_for_memory_guard(
    requested_workers: int,
    *,
    memory_limits: MemoryGuardLimits | None,
) -> int:
    requested = max(1, requested_workers)
    if memory_limits is None:
        memory_limits = default_memory_guard_limits()
    if not memory_limits.enabled:
        memory_limits = replace(memory_limits, enabled=True)
    current_limits = memory_limits.current_memory_limits()
    global_kb = (
        memory_limits.max_global_rss_kb
        if current_limits.max_global_rss_kb is None
        else current_limits.max_global_rss_kb
    )
    total_kb = (
        memory_limits.max_total_rss_kb
        if current_limits.max_total_rss_kb is None
        else current_limits.max_total_rss_kb
    )
    safe_workers = max(
        1,
        global_kb // max(1, total_kb),
    )
    return min(requested, safe_workers)


def run_gate(
    tiers: list[int],
    fail_fast: bool = False,
    parallel: bool = False,
    dry_run: bool = False,
    json_out: bool = False,
    verbose: bool = False,
    memory_limits: MemoryGuardLimits | None | object = _DEFAULT_MEMORY_LIMITS,
) -> list[CheckResult]:
    """Run all checks for the requested tiers and return results."""
    resolved_memory_limits = _resolve_memory_limits(memory_limits)
    all_checks = _build_checks()
    selected = [c for c in all_checks if c.tier in tiers]

    if not selected:
        print("No checks selected.")
        return []

    results: list[CheckResult] = []

    # Group by tier for ordered execution
    for tier in sorted(set(tiers)):
        tier_checks = [c for c in selected if c.tier == tier]
        if not tier_checks:
            continue

        if not json_out:
            print(f"\n{bold(f'=== Tier {tier} ===')} ({len(tier_checks)} checks)")

        if parallel and len(tier_checks) > 1:
            # Run checks within a tier concurrently
            requested_workers = min(4, len(tier_checks))
            max_workers = _parallel_workers_for_memory_guard(
                requested_workers,
                memory_limits=resolved_memory_limits,
            )
            if max_workers < requested_workers and not json_out:
                print(
                    "[MEMORY-GUARD] "
                    f"Clamping ci_gate workers from {requested_workers} to "
                    f"{max_workers} "
                    f"(global={resolved_memory_limits.max_global_rss_gb:.2f}GB "
                    f"tree={resolved_memory_limits.max_total_rss_gb:.2f}GB)."
                )
            with ThreadPoolExecutor(max_workers=max_workers) as pool:
                futures = {
                    pool.submit(
                        _run_check,
                        check,
                        dry_run,
                        resolved_memory_limits,
                    ): check
                    for check in tier_checks
                }
                for future in as_completed(futures):
                    result = future.result()
                    results.append(result)
                    if not json_out:
                        _print_result(result, verbose)
                    if (
                        fail_fast
                        and result.status in ("fail", "error")
                        and futures[future].required
                    ):
                        # Cancel remaining futures
                        for f in futures:
                            f.cancel()
                        break
        else:
            for check in tier_checks:
                result = _run_check(check, dry_run, resolved_memory_limits)
                results.append(result)
                if not json_out:
                    _print_result(result, verbose)
                if fail_fast and result.status in ("fail", "error") and check.required:
                    break

        # Check for fail-fast across tiers
        if fail_fast and any(
            r.status in ("fail", "error") for r in results if r.tier == tier
        ):
            required_failures = [
                r
                for r in results
                if r.tier == tier
                and r.status in ("fail", "error")
                and any(c.required for c in tier_checks if c.name == r.name)
            ]
            if required_failures:
                break

    return results


def _strip_background_flag(argv: Sequence[str]) -> list[str]:
    return [arg for arg in argv if arg != "--background"]


def _background_process_env_and_kwargs(
    env: Mapping[str, str],
    limits: MemoryGuardLimits | None | object = _DEFAULT_MEMORY_LIMITS,
) -> tuple[dict[str, str], dict[str, object]]:
    kwargs: dict[str, object] = {"start_new_session": True}
    resolved_limits = _resolve_memory_limits(limits)
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_CI_GATE",
        env,
        repo_root=ROOT,
        limits=resolved_limits,
    )
    if os.name == "posix":
        kwargs.update(context.process_group_kwargs())
    return dict(context.env), kwargs


def launch_background_gate(argv: Sequence[str]) -> BackgroundGateMetadata:
    """Launch this gate detached and write stdout/stderr to canonical logs."""
    LOG_ROOT.mkdir(parents=True, exist_ok=True)
    created_at = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    base = f"ci_gate_{stamp}_{os.getpid()}"
    log_path = LOG_ROOT / f"{base}.log"
    metadata_path = LOG_ROOT / f"{base}.json"
    env = os.environ.copy()
    env.setdefault("PYTHONUNBUFFERED", "1")
    env, popen_kwargs = _background_process_env_and_kwargs(env)
    command = [
        sys.executable,
        str(TOOLS / "guarded_exec.py"),
        "--prefix",
        "MOLT_CI_GATE",
        "--cwd",
        str(ROOT),
        "--",
        sys.executable,
        str(CI_GATE),
        *_strip_background_flag(argv),
    ]
    with log_path.open("ab") as log:
        proc = subprocess.Popen(
            command,
            cwd=str(ROOT),
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            **popen_kwargs,
        )
    metadata = BackgroundGateMetadata(
        pid=proc.pid,
        command=command,
        log_path=log_path,
        metadata_path=metadata_path,
        cwd=ROOT,
        created_at=created_at,
    )
    metadata_path.write_text(
        json.dumps(
            {
                "pid": metadata.pid,
                "command": metadata.command,
                "log_path": str(metadata.log_path),
                "metadata_path": str(metadata.metadata_path),
                "cwd": str(metadata.cwd),
                "created_at": metadata.created_at,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return metadata


def _results_to_dict(results: list[CheckResult]) -> dict[str, Any]:
    """Convert results to a JSON-serializable dict."""
    passed = sum(1 for r in results if r.status == "pass")
    failed = sum(1 for r in results if r.status == "fail")
    errored = sum(1 for r in results if r.status == "error")
    skipped = sum(1 for r in results if r.status == "skip")
    total_time = sum(r.duration_s for r in results)

    return {
        "summary": {
            "total": len(results),
            "passed": passed,
            "failed": failed,
            "errored": errored,
            "skipped": skipped,
            "total_time_s": round(total_time, 2),
            "success": failed == 0 and errored == 0,
        },
        "checks": [
            {
                "name": r.name,
                "tier": r.tier,
                "status": r.status,
                "duration_s": r.duration_s,
                "returncode": r.returncode,
                **({"skip_reason": r.skip_reason} if r.skip_reason else {}),
                **(
                    {"stderr_tail": r.stderr[-500:]}
                    if r.status in ("fail", "error") and r.stderr
                    else {}
                ),
            }
            for r in results
        ],
    }


def _memory_guard_limits_from_args(args: argparse.Namespace) -> MemoryGuardLimits:
    env = os.environ.copy()
    if args.max_rss_gb is not None:
        env["MOLT_CI_GATE_MAX_PROCESS_RSS_GB"] = str(args.max_rss_gb)
    if args.max_total_rss_gb is not None:
        env["MOLT_CI_GATE_MAX_TOTAL_RSS_GB"] = str(args.max_total_rss_gb)
    if args.max_global_rss_gb is not None:
        env["MOLT_CI_GATE_MAX_GLOBAL_RSS_GB"] = str(args.max_global_rss_gb)
    if args.child_rlimit_gb is not None:
        env["MOLT_CI_GATE_CHILD_RLIMIT_GB"] = str(args.child_rlimit_gb)
    env["MOLT_CI_GATE_MEMORY_GUARD_POLL_SEC"] = str(args.memory_poll_interval)
    return _resolve_memory_limits(
        harness_memory_guard.limits_from_env("MOLT_CI_GATE", env)
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Unified CI gate with tiered verification pipeline.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--tier",
        choices=["1", "2", "3", "all"],
        default="1",
        help="Which tier to run (default: 1). 'all' runs tiers 1-3.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output results as JSON",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop on first required-check failure",
    )
    parser.add_argument(
        "--parallel",
        action="store_true",
        help="Run independent checks within each tier concurrently",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would run without executing",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show stderr tail on failures",
    )
    parser.add_argument(
        "--background",
        action="store_true",
        help="Launch the selected gate in the background and write logs under logs/ci_gate/.",
    )
    parser.add_argument(
        "--max-rss-gb",
        type=float,
        default=None,
        help=(
            "Abort a check if any process exceeds this RSS; must be "
            f"<{memory_guard.DEFAULT_HARD_MAX_RSS_GB:g} "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--max-total-rss-gb",
        type=float,
        default=None,
        help=(
            "Abort a check if its process tree exceeds this aggregate RSS; must be "
            f"<{memory_guard.DEFAULT_HARD_MAX_RSS_GB:g} "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--max-global-rss-gb",
        type=float,
        default=None,
        help=(
            "Clamp parallel check workers so their process-tree RSS budgets fit "
            f"below this cumulative ceiling; must be "
            f"<{memory_guard.DEFAULT_HARD_MAX_GLOBAL_RSS_GB:g} "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--child-rlimit-gb",
        type=float,
        default=None,
        help=(
            "Apply this OS resource limit to each direct check child before exec; "
            "defaults to an adaptive virtual-memory clamp. Set <=0 to disable."
        ),
    )
    parser.add_argument(
        "--memory-poll-interval",
        type=float,
        default=memory_guard.DEFAULT_POLL_INTERVAL_SEC,
        help=(
            "Memory guard polling interval in seconds "
            f"(default: {memory_guard.DEFAULT_POLL_INTERVAL_SEC})."
        ),
    )
    args = parser.parse_args()

    if args.background:
        metadata = launch_background_gate(sys.argv[1:])
        print(
            json.dumps(
                {
                    "pid": metadata.pid,
                    "command": metadata.command,
                    "log_path": str(metadata.log_path),
                    "metadata_path": str(metadata.metadata_path),
                    "cwd": str(metadata.cwd),
                    "created_at": metadata.created_at,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return

    if args.tier == "all":
        tiers = [1, 2, 3]
    else:
        tier_num = int(args.tier)
        # Running tier N implies running all tiers <= N
        tiers = list(range(1, tier_num + 1))

    try:
        memory_limits = _memory_guard_limits_from_args(args)
        if memory_limits is not None:
            memory_limits.max_process_rss_kb
            memory_limits.max_total_rss_kb
            memory_limits.max_global_rss_kb
            memory_limits.child_rlimit_kb
            if memory_limits.poll_interval <= 0:
                raise ValueError("memory poll interval must be greater than 0")
    except ValueError as exc:
        print(f"ci_gate: {exc}", file=sys.stderr)
        sys.exit(2)

    if not args.json:
        tier_label = "all" if args.tier == "all" else args.tier
        mode_flags = []
        if args.fail_fast:
            mode_flags.append("fail-fast")
        if args.parallel:
            mode_flags.append("parallel")
        if args.dry_run:
            mode_flags.append("dry-run")
        mode_str = f" [{', '.join(mode_flags)}]" if mode_flags else ""
        print(bold(f"Molt CI Gate -- tier {tier_label}{mode_str}"))

    results = run_gate(
        tiers=tiers,
        fail_fast=args.fail_fast,
        parallel=args.parallel,
        dry_run=args.dry_run,
        json_out=args.json,
        verbose=args.verbose,
        memory_limits=memory_limits,
    )

    if args.json:
        output = _results_to_dict(results)
        print(json.dumps(output, indent=2))
    else:
        # Print summary
        passed = sum(1 for r in results if r.status == "pass")
        failed = sum(1 for r in results if r.status == "fail")
        errored = sum(1 for r in results if r.status == "error")
        skipped = sum(1 for r in results if r.status == "skip")
        total_time = sum(r.duration_s for r in results)

        print(f"\n{bold('Summary:')}")
        parts = []
        if passed:
            parts.append(green(f"{passed} passed"))
        if failed:
            parts.append(red(f"{failed} failed"))
        if errored:
            parts.append(red(f"{errored} errored"))
        if skipped:
            parts.append(yellow(f"{skipped} skipped"))
        print(f"  {', '.join(parts)} in {total_time:.1f}s")

        if failed > 0 or errored > 0:
            failures = [r for r in results if r.status in ("fail", "error")]
            print(f"\n{bold('Failures:')}")
            for r in failures:
                print(f"  {red(r.name)} (tier {r.tier}, rc={r.returncode})")

    # Exit code: 0 if no required failures
    all_checks = _build_checks()
    required_names = {c.name for c in all_checks if c.required}
    has_required_failure = any(
        r.status in ("fail", "error") and r.name in required_names for r in results
    )
    sys.exit(1 if has_required_failure else 0)


if __name__ == "__main__":
    main()
