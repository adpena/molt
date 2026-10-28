#!/usr/bin/env python3
"""
Molt unified benchmark and differential testing harness.

Modes:
  --bench    Run bench/*.py benchmarks, time Molt vs CPython
  --diff     Run tests/differential/basic/*.py, diff outputs
  --full     Run both modes

Options:
  --filter PATTERN   Only run tests matching glob/substring pattern
  --parallel N       Run up to N tests in parallel (default: 1)
  --update-baseline  Save results to bench/results/harness-baseline.json
  --timeout SECS     Per-test timeout in seconds (default: 30)
  --molt PATH        Path to molt binary (default: ./.venv/bin/molt or PATH)
  --molt-profile     Molt run build profile: dev or release (default: dev)
  --python PATH      Path to CPython binary (default: python3)
  --output PATH      JSON output path (default: bench/results/harness.json)
  --verbose          Show diffs and stderr on failure
  --no-color         Disable colored output

Examples:
  python3 bench/harness.py --bench
  python3 bench/harness.py --diff --filter "arith*" --parallel 8
  python3 bench/harness.py --full --update-baseline
  python3 bench/harness.py --bench --diff --parallel 4
"""

import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Optional
from collections.abc import Mapping
from contextlib import ExitStack

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
BENCH_DIR = REPO_ROOT / "bench"
DIFF_DIR = REPO_ROOT / "tests" / "differential" / "basic"
DEFAULT_RESULTS_DIR = BENCH_DIR / "results"
DEFAULT_OUTPUT = DEFAULT_RESULTS_DIR / "harness.json"
DEFAULT_BASELINE = DEFAULT_RESULTS_DIR / "harness-baseline.json"


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import harness_memory_guard  # noqa: E402
from tools import perf_authority, bench_suites  # noqa: E402
from tools.compat.comparison import Outputs, compare_outputs  # noqa: E402
from tools.safe_run import RunOutcome, observed_run_outcome  # noqa: E402
from tools.bench_evidence import (  # noqa: E402
    AdmittedBenchmarkWorkload,
    BenchmarkWorkloadRecord,
    admit_benchmark_workload,
    workload_identity_problems,
)
from tools.bench_reporting import write_json  # noqa: E402
from molt.dx import development_artifact_env, scratch_dir  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402

BENCH_MEMORY_PREFIX = "MOLT_BENCH"

# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


@dataclass
class TestResult:
    name: str
    suite: str  # "bench", "diff"
    status: str  # "pass", "fail", "error", "skip", "timeout"
    workload: BenchmarkWorkloadRecord | None
    cpython_stdout: str = ""
    cpython_stderr: str = ""
    cpython_rc: int | None = None
    cpython_time_s: Optional[float] = None
    molt_stdout: str = ""
    molt_stderr: str = ""
    molt_rc: int | None = None
    molt_time_s: Optional[float] = None
    molt_speedup: Optional[float] = None
    output_match: Optional[bool] = None
    diff_snippet: str = ""
    error_msg: str = ""

    def to_dict(self):
        """Compact dict for JSON serialization."""
        d = {
            "name": self.name,
            "suite": self.suite,
            "status": self.status,
        }
        if self.workload is not None:
            d["workload"] = self.workload
        if self.cpython_time_s is not None:
            d["cpython_time_s"] = round(self.cpython_time_s, 6)
        if self.molt_time_s is not None:
            d["molt_time_s"] = round(self.molt_time_s, 6)
        if self.molt_speedup is not None:
            d["molt_speedup"] = round(self.molt_speedup, 2)
        if self.output_match is not None:
            d["output_match"] = self.output_match
        if self.error_msg:
            d["error_msg"] = self.error_msg
        if self.diff_snippet:
            d["diff_snippet"] = self.diff_snippet[:500]
        if self.status != "pass":
            d.update(
                cpython_stdout=self.cpython_stdout,
                cpython_stderr=self.cpython_stderr,
                cpython_rc=self.cpython_rc,
                molt_stdout=self.molt_stdout,
                molt_stderr=self.molt_stderr,
                molt_rc=self.molt_rc,
            )
        return d


@dataclass
class SuiteSummary:
    suite: str
    total: int = 0
    passed: int = 0
    failed: int = 0
    errors: int = 0
    skipped: int = 0
    timeouts: int = 0


# ---------------------------------------------------------------------------
# Colors
# ---------------------------------------------------------------------------


class Colors:
    def __init__(self, enabled: bool):
        if enabled and sys.stdout.isatty():
            self.GREEN = "\033[0;32m"
            self.RED = "\033[0;31m"
            self.YELLOW = "\033[0;33m"
            self.BLUE = "\033[0;34m"
            self.CYAN = "\033[0;36m"
            self.BOLD = "\033[1m"
            self.DIM = "\033[2m"
            self.RESET = "\033[0m"
        else:
            self.GREEN = self.RED = self.YELLOW = self.BLUE = ""
            self.CYAN = self.BOLD = self.DIM = self.RESET = ""

    def status(self, s: str) -> str:
        m = {
            "pass": self.GREEN,
            "fail": self.RED,
            "error": self.RED,
            "skip": self.YELLOW,
            "timeout": self.YELLOW,
        }
        color = m.get(s, "")
        return f"{color}{s.upper()}{self.RESET}"


# ---------------------------------------------------------------------------
# Runner helpers
# ---------------------------------------------------------------------------


def run_cmd(
    cmd: list,
    timeout_s: float,
    cwd: Optional[Path] = None,
    *,
    env: Mapping[str, str] | None = None,
) -> RunOutcome:
    """Retain the shared guard's output, completion and child timing evidence."""
    env = _base_env() if env is None else env
    limits = harness_memory_guard.limits_from_env(BENCH_MEMORY_PREFIX, env)
    timeout = harness_memory_guard.timeout_from_env(
        BENCH_MEMORY_PREFIX,
        env,
        explicit=timeout_s,
    )
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=BENCH_MEMORY_PREFIX,
            capture_output=True,
            text=False,
            encoding="utf-8",
            errors="surrogateescape",
            timeout=timeout,
            cwd=cwd or REPO_ROOT,
            env=env,
            limits=limits,
        )
        return observed_run_outcome(proc, captured_stderr=proc.child_stderr)
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout
        stderr = error.stderr
        return RunOutcome(
            False,
            None,
            None,
            "timeout",
            None,
            stdout=(
                stdout.decode("utf-8", "surrogateescape")
                if isinstance(stdout, bytes)
                else stdout
            ),
            stderr=(
                stderr.decode("utf-8", "surrogateescape")
                if isinstance(stderr, bytes)
                else stderr
            ),
        )
    except FileNotFoundError as e:
        return RunOutcome(False, None, None, "not_found", None, stderr=str(e))


def _base_env() -> ProcessEnvironment:
    return development_artifact_env(
        REPO_ROOT,
        os.environ,
        session_prefix="bench-harness",
        session_id=os.environ.get("MOLT_SESSION_ID") or f"bench-harness-{os.getpid()}",
        create_dirs=True,
    )


def resolve_molt_cli(explicit_path: str | None) -> str:
    """Return the Molt CLI path for the legacy harness."""
    if explicit_path:
        return explicit_path
    repo_local = REPO_ROOT / ".venv" / "bin" / "molt"
    if repo_local.is_file():
        return str(repo_local)
    found = shutil.which("molt")
    return found if found is not None else "molt"


def compute_diff(a: str, b: str) -> str:
    """Return a unified diff snippet between two strings, or empty if equal."""
    if a == b:
        return ""
    a_lines = a.splitlines(keepends=True)
    b_lines = b.splitlines(keepends=True)
    import difflib

    diff = difflib.unified_diff(
        a_lines, b_lines, fromfile="cpython", tofile="molt", lineterm=""
    )
    # Captured output keeps undecodable bytes as surrogate escapes; show them
    # as \xNN so a printed snippet never fails and distinct bytes stay distinct.
    return (
        "".join(list(diff)[:60])
        .encode("utf-8", "surrogateescape")
        .decode("utf-8", "backslashreplace")
    )


# ---------------------------------------------------------------------------
# Test execution
# ---------------------------------------------------------------------------


def run_single_test(
    script: Path,
    suite: str,
    molt_cmd: list,
    python_cmd: str,
    timeout_s: float,
    *,
    workload: AdmittedBenchmarkWorkload | None,
) -> TestResult:
    """Run one test script through CPython and Molt, compare outputs."""
    name = script.name
    if suite == "bench" and workload is None:
        raise ValueError("benchmark execution requires an admitted workload")
    if workload is not None:
        if script.expanduser().absolute() != workload.script:
            raise ValueError("benchmark source differs from admitted workload")
        workload.verify()
        if workload.record["molt_args"] != molt_cmd[1:]:
            raise ValueError("benchmark command differs from admitted Molt arguments")
    result = TestResult(
        name=name,
        suite=suite,
        status="error",
        workload=workload.record if workload is not None else None,
    )

    # --- CPython ---
    if workload is None:
        reference_run = run_cmd([python_cmd, str(script)], timeout_s)
    else:
        reference = workload.reference_invocation([python_cmd], _base_env())
        reference_run = run_cmd(
            list(reference.command),
            timeout_s,
            cwd=reference.cwd,
            env=reference.environment,
        )
        if reference_run.ok:
            workload.verify_reference_output(
                reference_run.stdout or "", reference_run.stderr or ""
            )
    if workload is not None:
        workload.verify()
    cp_out = result.cpython_stdout = reference_run.stdout or ""
    cp_err = result.cpython_stderr = reference_run.stderr or ""
    cp_rc = result.cpython_rc = reference_run.exit_code
    cp_time = result.cpython_time_s = reference_run.elapsed_s

    if not reference_run.ok:
        # A failed reference remains a skip only for the documented diff
        # policy or absent/timeout reference; incomplete custody is an error.
        result.status = (
            "skip"
            if reference_run.status in {"timeout", "not_found"}
            or (suite == "diff" and reference_run.status == "nonzero")
            else "error"
        )
        result.error_msg = (
            f"CPython {reference_run.status} (rc={reference_run.exit_code})"
        )
        return result

    # --- Molt ---
    molt_full_cmd = molt_cmd + [str(script)]
    if workload is not None:
        workload.verify()
    if workload is None:
        guest_run = run_cmd(molt_full_cmd, timeout_s)
    else:
        prefix = [*molt_cmd, str(workload.execution_script)]
        if workload.record["run_args"]:
            prefix.append("--")
        guest = workload.guest_invocation(
            prefix, workload.compiler_environment(_base_env())
        )
        guest_run = run_cmd(
            list(guest.command), timeout_s, cwd=guest.cwd, env=guest.environment
        )
    if workload is not None:
        workload.verify()
    m_out = result.molt_stdout = guest_run.stdout or ""
    m_err = result.molt_stderr = guest_run.stderr or ""
    m_rc = result.molt_rc = guest_run.exit_code
    m_time = result.molt_time_s = guest_run.elapsed_s

    if not guest_run.ok:
        result.status = "timeout" if guest_run.status == "timeout" else "error"
        result.error_msg = f"Molt {guest_run.status} (rc={guest_run.exit_code})"
        return result

    # --- Compare ---
    result.output_match = (
        compare_outputs(
            Outputs(cp_out, cp_err, cp_rc),
            Outputs(m_out, m_err, m_rc),
            stderr_mode="exact",
        ).equal
        if suite == "bench"
        else cp_out == m_out
    )

    if suite == "bench":
        # Only complete successful observations can reach ratio calculation.
        if result.output_match:
            speedup = perf_authority.safe_speedup(cp_time, m_time)
            result.molt_speedup = round(speedup, 2) if speedup is not None else None
        result.status = "pass" if result.output_match else "fail"
    else:
        # For diff tests, pass/fail based on output match
        if result.output_match:
            result.status = "pass"
        else:
            result.status = "fail"
            result.diff_snippet = compute_diff(cp_out, m_out)

    return result


# ---------------------------------------------------------------------------
# Suite collectors
# ---------------------------------------------------------------------------


def collect_bench_scripts(
    filter_pat: Optional[str] = None, *, applications: bool = False
) -> list:
    scripts = (
        [REPO_ROOT / script for script in bench_suites.APPLICATION_BENCHMARKS]
        if applications
        else sorted(BENCH_DIR.glob("bench_*.py"))
    )
    if filter_pat:
        scripts = [
            s
            for s in scripts
            if fnmatch.fnmatch(s.name, filter_pat) or filter_pat in s.name
        ]
    return scripts


def collect_diff_scripts(filter_pat: Optional[str] = None) -> list:
    scripts = sorted(DIFF_DIR.glob("*.py"))
    if filter_pat:
        scripts = [
            s
            for s in scripts
            if fnmatch.fnmatch(s.name, filter_pat) or filter_pat in s.name
        ]
    return scripts


# ---------------------------------------------------------------------------
# Regression detection
# ---------------------------------------------------------------------------


def detect_regressions(results: list, baseline_path: Path):
    """
    Compare benchmark results against baseline.
    Returns list of regression dicts.
    Retain status regressions independently of performance evidence.
    This legacy `molt run` lane does not retain a build observation, so a
    timing comparison is an explicit incompatibility, never a measured change.
    """
    if not baseline_path.exists():
        return []

    with open(baseline_path, encoding="utf-8") as f:
        baseline = json.load(f)

    # Status belongs to the harness's existing suite/name result records.
    # Flat benchmark keys now carry source identity and serve metric readers.
    bl_results = baseline.get("results", {})
    regressions = []

    for r in results:
        bl = bl_results.get(r.suite, {}).get(r.name)
        if bl is None:
            continue

        # Status regression: was passing, now failing
        bl_ok = bl.get("molt_ok", False) or bl.get("status") == "pass"
        now_ok = r.status == "pass"
        if bl_ok and not now_ok:
            regressions.append(
                {
                    "name": r.name,
                    "type": "status",
                    "detail": f"was passing, now {r.status}",
                }
            )
            continue

        # Raw timings remain available, but this lane has no producer-bound
        # physical compiler/runtime identity. A request label (including a
        # historical release-fast stamp) cannot authorize a timing comparison.
        if r.suite == "bench" and r.molt_time_s and bl.get("molt_time_s"):
            regressions.append(
                {
                    "name": r.name,
                    "type": "incompatible_timing",
                    "detail": (
                        "timing comparison refused: current selected build profiles "
                        "unbound; use tools/bench.py with retained build observations"
                    ),
                }
            )

    return regressions


# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------


def print_suite_header(c: Colors, suite_name: str, count: int):
    print()
    print(f"{c.BOLD}{'=' * 70}{c.RESET}")
    print(f"{c.BOLD}  {suite_name} ({count} tests){c.RESET}")
    print(f"{c.BOLD}{'=' * 70}{c.RESET}")


def print_result_line(c: Colors, r: TestResult, verbose: bool = False):
    status_str = c.status(r.status)
    line = f"  {status_str:>18s}  {r.name}"

    extras = []
    if (
        r.suite == "bench"
        and r.molt_time_s is not None
        and r.cpython_time_s is not None
    ):
        extras.append(f"molt={r.molt_time_s:.3f}s")
        extras.append(f"cpython={r.cpython_time_s:.3f}s")
        if r.molt_speedup is not None:
            extras.append(f"{r.molt_speedup:.1f}x")
    if extras:
        line += f"  {c.DIM}({', '.join(extras)}){c.RESET}"
    if r.error_msg and r.status in ("error", "timeout", "skip"):
        line += f"  {c.DIM}-- {r.error_msg[:80]}{c.RESET}"

    print(line)

    if verbose and r.diff_snippet and r.status == "fail":
        for dl in r.diff_snippet.splitlines()[:20]:
            print(f"      {dl}")


def print_summary_table(c: Colors, summaries: list, regressions: list):
    print()
    print(f"{c.BOLD}{'=' * 70}{c.RESET}")
    print(f"{c.BOLD}  SUMMARY{c.RESET}")
    print(f"{c.BOLD}{'=' * 70}{c.RESET}")
    print()

    header = f"  {'Suite':<16s} {'Total':>7s} {'Pass':>7s} {'Fail':>7s} {'Error':>7s} {'Skip':>7s} {'T/O':>7s} {'Rate':>8s}"
    print(header)
    print(f"  {'-' * 64}")

    for s in summaries:
        tested = s.passed + s.failed
        rate = f"{100 * s.passed / tested:.1f}%" if tested > 0 else "N/A"
        pass_c = c.GREEN if s.failed == 0 and s.errors == 0 else ""
        fail_c = c.RED if s.failed > 0 or s.errors > 0 else ""
        reset = c.RESET if pass_c or fail_c else ""
        print(
            f"  {s.suite:<16s} {s.total:>7d} "
            f"{pass_c}{s.passed:>7d}{reset} "
            f"{fail_c}{s.failed:>7d}{reset} "
            f"{fail_c}{s.errors:>7d}{reset} "
            f"{s.skipped:>7d} {s.timeouts:>7d} {rate:>8s}"
        )

    if regressions:
        print()
        print(f"  {c.RED}{c.BOLD}BASELINE FINDINGS ({len(regressions)}):{c.RESET}")
        for reg in regressions:
            print(
                f"    {c.RED}- {reg['name']}: {reg['type']} -- {reg['detail']}{c.RESET}"
            )
    elif summaries:
        print()
        print(f"  {c.GREEN}No regressions detected against baseline.{c.RESET}")

    print()


def build_json_report(
    all_results: list,
    summaries: list,
    regressions: list,
    *,
    memory_guard: dict[str, object] | None = None,
    molt_profile: str = "dev",
) -> dict:
    # bench/harness.py is the dev/correctness differential harness - a
    # NON-CANONICAL perf lane (it runs `molt run` at dev/release, not the
    # release-fast scoreboard gate). Stamp the report so its numbers
    # self-identify and are never cited as the perf contract.
    report = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "provenance": perf_authority.non_canonical_provenance(
            requested_profile=molt_profile,
            source="bench/harness.py",
        ),
        "summaries": {},
        "regressions": regressions,
        "results": {},
    }
    for s in summaries:
        report["summaries"][s.suite] = {
            "total": s.total,
            "passed": s.passed,
            "failed": s.failed,
            "errors": s.errors,
            "skipped": s.skipped,
            "timeouts": s.timeouts,
        }
    for r in all_results:
        suite_key = r.suite
        if suite_key not in report["results"]:
            report["results"][suite_key] = {}
        report["results"][suite_key][r.name] = r.to_dict()

    # Also produce baseline-compatible "benchmarks" section
    bench_baseline = {}
    for r in all_results:
        if r.suite == "bench":
            # molt_cpython_ratio is molt_time / cpython_time (lower = faster),
            # the SAME definition tools/bench.py and its baseline consumer use.
            # Previously this field was (wrongly) set to molt_speedup
            # (cpython_time / molt_time, higher = faster) - an inverted value
            # that broke cross-tool regression comparison. It is now produced by
            # the SINGLE guarded authority (perf_authority.signed_ratio) in the
            # MOLT_OVER_BASELINE direction, so a missing/None/0/NaN time yields
            # None (never a finite ratio) and the field carries explicit
            # direction metadata (audit meta-bug item 2).
            cpython_ratio_block = perf_authority.signed_ratio(
                r.molt_time_s,
                r.cpython_time_s,
                direction=perf_authority.RatioDirection.MOLT_OVER_BASELINE,
            )
            problems = workload_identity_problems(r.workload)
            if problems:
                raise ValueError("; ".join(problems))
            assert r.workload is not None
            bench_baseline[r.workload["entry"]] = {
                "workload": r.workload,
                "requested_profile": molt_profile,
                "build_observation": None,
                "molt_ok": r.status == "pass",
                "build_ok": r.status != "error"
                or "not found" not in (r.error_msg or ""),
                "molt_time_s": r.molt_time_s,
                "cpython_time_s": r.cpython_time_s,
                "molt_speedup": r.molt_speedup,
                "molt_cpython_ratio": cpython_ratio_block["value"],
                "ratio_directions": {
                    "molt_speedup": perf_authority.RatioDirection.SPEEDUP.value,
                    "molt_cpython_ratio": cpython_ratio_block["direction"],
                },
                "output_match": r.output_match,
                "status": r.status,
            }
    if bench_baseline:
        report["benchmarks"] = bench_baseline
    if memory_guard is not None:
        report["memory_guard"] = memory_guard

    return report


# ---------------------------------------------------------------------------
# Main execution
# ---------------------------------------------------------------------------


def run_suite(
    suite_name: str,
    scripts: list,
    molt_cmd: list,
    python_cmd: str,
    timeout_s: float,
    parallel: int,
    colors: Colors,
    verbose: bool,
    *,
    workloads: Mapping[Path, AdmittedBenchmarkWorkload],
) -> tuple:
    """Run a test suite. Returns (results_list, SuiteSummary)."""
    if not scripts:
        return [], SuiteSummary(suite=suite_name)

    print_suite_header(colors, suite_name, len(scripts))

    results = []
    summary = SuiteSummary(suite=suite_name, total=len(scripts))

    if parallel > 1:
        futures = {}
        with ThreadPoolExecutor(max_workers=parallel) as pool:
            for script in scripts:
                fut = pool.submit(
                    run_single_test,
                    script,
                    suite_name,
                    molt_cmd,
                    python_cmd,
                    timeout_s,
                    workload=workloads[script] if suite_name == "bench" else None,
                )
                futures[fut] = script

            for fut in as_completed(futures):
                r = fut.result()
                results.append(r)
                print_result_line(colors, r, verbose)
    else:
        for script in scripts:
            r = run_single_test(
                script,
                suite_name,
                molt_cmd,
                python_cmd,
                timeout_s,
                workload=workloads[script] if suite_name == "bench" else None,
            )
            results.append(r)
            print_result_line(colors, r, verbose)

    # Sort results by name for deterministic output
    results.sort(key=lambda r: r.name)

    for r in results:
        if r.status == "pass":
            summary.passed += 1
        elif r.status == "fail":
            summary.failed += 1
        elif r.status == "error":
            summary.errors += 1
        elif r.status == "skip":
            summary.skipped += 1
        elif r.status == "timeout":
            summary.timeouts += 1

    return results, summary


def main():
    with ExitStack() as workload_custody:
        return _main(workload_custody)


def _main(workload_custody: ExitStack):
    parser = argparse.ArgumentParser(
        description="Molt unified benchmark and differential testing harness.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--bench", action="store_true", help="Run bench/*.py benchmarks"
    )
    parser.add_argument(
        "--diff", action="store_true", help="Run tests/differential/basic/*.py"
    )
    parser.add_argument("--full", action="store_true", help="Run all suites")
    parser.add_argument(
        "--filter", type=str, default=None, help="Glob/substring filter for test names"
    )
    parser.add_argument(
        "--parallel", type=int, default=1, help="Parallel test workers (default: 1)"
    )
    parser.add_argument(
        "--update-baseline", action="store_true", help="Save results as new baseline"
    )
    parser.add_argument(
        "--timeout", type=float, default=30.0, help="Per-test timeout in seconds"
    )
    parser.add_argument("--molt", type=str, default=None, help="Path to molt binary")
    parser.add_argument(
        "--molt-profile",
        choices=["dev", "release"],
        default="dev",
        help="Build profile passed through `molt run`.",
    )
    parser.add_argument(
        "--python", type=str, default="python3", help="Path to CPython binary"
    )
    parser.add_argument(
        "--output", type=str, default=str(DEFAULT_OUTPUT), help="JSON output path"
    )
    parser.add_argument(
        "--baseline", type=str, default=str(DEFAULT_BASELINE), help="Baseline JSON path"
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Show diffs and stderr on failure"
    )
    parser.add_argument(
        "--no-color", action="store_true", help="Disable colored output"
    )

    parser.add_argument(
        "--applications",
        action="store_true",
        help="Run sealed application benchmarks only",
    )
    args = parser.parse_args()
    if args.applications:
        args.bench = True

    # If no mode specified, show help
    if not (args.bench or args.diff or args.full):
        parser.print_help()
        sys.exit(1)

    if args.full:
        args.bench = args.diff = True

    colors = Colors(enabled=not args.no_color)
    run_env = _base_env()
    limits = harness_memory_guard.limits_from_env(BENCH_MEMORY_PREFIX, run_env)

    # Resolve molt command
    molt_bin = resolve_molt_cli(args.molt)
    molt_cmd = [molt_bin, "run", "--profile", args.molt_profile]

    print(f"{colors.BOLD}Molt Test Harness{colors.RESET}")
    print(f"  molt:    {' '.join(molt_cmd)}")
    print(f"  python:  {args.python}")
    print(f"  timeout: {args.timeout}s")
    print(f"  parallel: {args.parallel}")
    if args.filter:
        print(f"  filter:  {args.filter}")

    all_results = []
    summaries = []
    workloads = {}

    def finish_workload_custody() -> None:
        for workload in workloads.values():
            workload.verify()
        workload_custody.close()

    with harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=scratch_dir(REPO_ROOT, "bench/harness"),
        label="bench_harness",
        limits=limits,
    ):
        # --- Benchmark suite ---
        if args.bench:
            scripts = collect_bench_scripts(args.filter, applications=args.applications)
            bench_suites.validate_benchmark_selection(scripts)
            workloads = {
                script: workload_custody.enter_context(
                    admit_benchmark_workload(
                        script,
                        project_root=REPO_ROOT,
                        run_args=bench_suites.application_run_args(script),
                        molt_args=molt_cmd[1:],
                    )
                )
                for script in scripts
            }
            results, summary = run_suite(
                "bench",
                scripts,
                molt_cmd,
                args.python,
                args.timeout,
                args.parallel,
                colors,
                args.verbose,
                workloads=workloads,
            )
            all_results.extend(results)
            summaries.append(summary)

        # --- Differential suite ---
        if args.diff:
            scripts = collect_diff_scripts(args.filter)
            results, summary = run_suite(
                "diff",
                scripts,
                molt_cmd,
                args.python,
                args.timeout,
                args.parallel,
                colors,
                args.verbose,
                workloads={},
            )
            all_results.extend(results)
            summaries.append(summary)

    # --- Regression detection ---
    baseline_path = Path(args.baseline)
    regressions = detect_regressions(all_results, baseline_path)

    # --- Summary ---
    print_summary_table(colors, summaries, regressions)

    # --- JSON output ---
    report = build_json_report(
        all_results,
        summaries,
        regressions,
        memory_guard=harness_memory_guard.limits_summary(limits),
        molt_profile=args.molt_profile,
    )
    output_path = Path(args.output)
    write_json(output_path, report, validate_inputs=finish_workload_custody)
    print(f"  JSON report written to: {output_path}")

    # --- Update baseline ---
    if args.update_baseline:
        write_json(baseline_path, report)
        print(f"  Baseline updated: {baseline_path}")

    print()

    # Exit code: non-zero if any failures/errors or regressions
    has_failures = any(s.failed > 0 or s.errors > 0 for s in summaries)
    if regressions or has_failures:
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
