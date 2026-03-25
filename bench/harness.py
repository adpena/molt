#!/usr/bin/env python3
"""
Molt unified benchmark and differential testing harness.

Modes:
  --bench    Run bench/*.py benchmarks, time Molt vs CPython
  --parity   Run tests/parity/*.py, diff outputs
  --diff     Run tests/differential/basic/*.py, diff outputs
  --full     Run all three modes

Options:
  --filter PATTERN   Only run tests matching glob/substring pattern
  --parallel N       Run up to N tests in parallel (default: 1)
  --update-baseline  Save results to bench/baseline.json
  --timeout SECS     Per-test timeout in seconds (default: 30)
  --molt PATH        Path to molt binary (default: ./target/release/molt)
  --python PATH      Path to CPython binary (default: python3)
  --output PATH      JSON output path (default: bench/results.json)
  --verbose          Show diffs and stderr on failure
  --no-color         Disable colored output

Examples:
  python3 bench/harness.py --bench
  python3 bench/harness.py --diff --filter "arith*" --parallel 8
  python3 bench/harness.py --full --update-baseline
  python3 bench/harness.py --bench --parity --parallel 4
"""

import argparse
import fnmatch
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
BENCH_DIR = REPO_ROOT / "bench"
PARITY_DIR = REPO_ROOT / "tests" / "parity"
DIFF_DIR = REPO_ROOT / "tests" / "differential" / "basic"
DEFAULT_OUTPUT = BENCH_DIR / "results.json"
DEFAULT_BASELINE = BENCH_DIR / "baseline.json"

# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclass
class TestResult:
    name: str
    suite: str  # "bench", "parity", "diff"
    status: str  # "pass", "fail", "error", "skip", "timeout"
    cpython_stdout: str = ""
    cpython_stderr: str = ""
    cpython_rc: int = 0
    cpython_time_s: Optional[float] = None
    molt_stdout: str = ""
    molt_stderr: str = ""
    molt_rc: int = 0
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

def run_cmd(cmd: list, timeout_s: float, cwd: Optional[Path] = None):
    """Run a command, capture output. Returns (stdout, stderr, rc, elapsed_s)."""
    start = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout_s,
            cwd=cwd or REPO_ROOT,
        )
        elapsed = time.monotonic() - start
        return proc.stdout, proc.stderr, proc.returncode, elapsed
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - start
        return "", f"TIMEOUT after {timeout_s}s", -1, elapsed
    except FileNotFoundError as e:
        elapsed = time.monotonic() - start
        return "", str(e), -2, elapsed


def compute_diff(a: str, b: str) -> str:
    """Return a unified diff snippet between two strings, or empty if equal."""
    if a == b:
        return ""
    a_lines = a.splitlines(keepends=True)
    b_lines = b.splitlines(keepends=True)
    import difflib
    diff = difflib.unified_diff(a_lines, b_lines, fromfile="cpython", tofile="molt", lineterm="")
    return "".join(list(diff)[:60])


# ---------------------------------------------------------------------------
# Test execution
# ---------------------------------------------------------------------------

def run_single_test(
    script: Path,
    suite: str,
    molt_cmd: list,
    python_cmd: str,
    timeout_s: float,
) -> TestResult:
    """Run one test script through CPython and Molt, compare outputs."""
    name = script.name
    result = TestResult(name=name, suite=suite, status="error")

    # --- CPython ---
    cp_out, cp_err, cp_rc, cp_time = run_cmd(
        [python_cmd, str(script)], timeout_s
    )
    result.cpython_stdout = cp_out
    result.cpython_stderr = cp_err
    result.cpython_rc = cp_rc
    result.cpython_time_s = cp_time

    if cp_rc == -1:
        result.status = "skip"
        result.error_msg = "CPython timed out"
        return result
    if cp_rc == -2:
        result.status = "skip"
        result.error_msg = f"CPython not found: {cp_err}"
        return result
    if cp_rc != 0 and suite == "diff":
        # For differential tests, skip if CPython itself fails
        result.status = "skip"
        result.error_msg = f"CPython exited {cp_rc}"
        return result

    # --- Molt ---
    molt_full_cmd = molt_cmd + [str(script)]
    m_out, m_err, m_rc, m_time = run_cmd(molt_full_cmd, timeout_s)
    result.molt_stdout = m_out
    result.molt_stderr = m_err
    result.molt_rc = m_rc
    result.molt_time_s = m_time

    if m_rc == -1:
        result.status = "timeout"
        result.error_msg = "Molt timed out"
        return result
    if m_rc == -2:
        result.status = "error"
        result.error_msg = f"Molt binary not found: {m_err}"
        return result

    # --- Compare ---
    result.output_match = (cp_out == m_out)

    if suite == "bench":
        # For benchmarks, compute speedup; pass if molt ran successfully
        if m_rc == 0 and cp_time > 0:
            result.molt_speedup = round(cp_time / m_time, 2) if m_time > 0 else None
        if m_rc == 0:
            result.status = "pass" if result.output_match else "fail"
        else:
            result.status = "error"
            result.error_msg = f"Molt exited {m_rc}"
    else:
        # For parity/diff tests, pass/fail based on output match
        if m_rc != 0 and cp_rc == 0:
            result.status = "error"
            result.error_msg = f"Molt exited {m_rc}"
        elif result.output_match:
            result.status = "pass"
        else:
            result.status = "fail"
            result.diff_snippet = compute_diff(cp_out, m_out)

    return result


# ---------------------------------------------------------------------------
# Suite collectors
# ---------------------------------------------------------------------------

def collect_bench_scripts(filter_pat: Optional[str] = None) -> list:
    scripts = sorted(BENCH_DIR.glob("bench_*.py"))
    if filter_pat:
        scripts = [s for s in scripts if fnmatch.fnmatch(s.name, filter_pat) or filter_pat in s.name]
    return scripts


def collect_parity_scripts(filter_pat: Optional[str] = None) -> list:
    scripts = sorted(PARITY_DIR.glob("test_*.py"))
    if filter_pat:
        scripts = [s for s in scripts if fnmatch.fnmatch(s.name, filter_pat) or filter_pat in s.name]
    return scripts


def collect_diff_scripts(filter_pat: Optional[str] = None) -> list:
    scripts = sorted(DIFF_DIR.glob("*.py"))
    if filter_pat:
        scripts = [s for s in scripts if fnmatch.fnmatch(s.name, filter_pat) or filter_pat in s.name]
    return scripts


# ---------------------------------------------------------------------------
# Regression detection
# ---------------------------------------------------------------------------

def detect_regressions(results: list, baseline_path: Path, threshold: float = 0.20):
    """
    Compare benchmark results against baseline.
    Returns list of regression dicts.
    A regression is flagged when:
      - A previously passing test now fails/errors
      - A benchmark slows down by more than `threshold` (20%)
    """
    if not baseline_path.exists():
        return []

    with open(baseline_path) as f:
        baseline = json.load(f)

    bl_benchmarks = baseline.get("benchmarks", {})
    regressions = []

    for r in results:
        bl = bl_benchmarks.get(r.name)
        if bl is None:
            continue

        # Status regression: was passing, now failing
        bl_ok = bl.get("molt_ok", False) or bl.get("status") == "pass"
        now_ok = r.status == "pass"
        if bl_ok and not now_ok:
            regressions.append({
                "name": r.name,
                "type": "status",
                "detail": f"was passing, now {r.status}",
            })
            continue

        # Performance regression (benchmarks only)
        if r.suite == "bench" and r.molt_time_s and bl.get("molt_time_s"):
            bl_time = bl["molt_time_s"]
            if bl_time > 0:
                slowdown = (r.molt_time_s - bl_time) / bl_time
                if slowdown > threshold:
                    regressions.append({
                        "name": r.name,
                        "type": "performance",
                        "detail": f"{slowdown:+.0%} slower ({bl_time:.4f}s -> {r.molt_time_s:.4f}s)",
                    })

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
    if r.suite == "bench" and r.molt_time_s is not None and r.cpython_time_s is not None:
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
        print(f"  {c.RED}{c.BOLD}REGRESSIONS DETECTED ({len(regressions)}):{c.RESET}")
        for reg in regressions:
            print(f"    {c.RED}- {reg['name']}: {reg['type']} -- {reg['detail']}{c.RESET}")
    elif summaries:
        print()
        print(f"  {c.GREEN}No regressions detected against baseline.{c.RESET}")

    print()


def build_json_report(all_results: list, summaries: list, regressions: list) -> dict:
    report = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
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
            bench_baseline[r.name] = {
                "molt_ok": r.status == "pass",
                "build_ok": r.status != "error" or "not found" not in (r.error_msg or ""),
                "molt_time_s": r.molt_time_s,
                "cpython_time_s": r.cpython_time_s,
                "molt_speedup": r.molt_speedup,
                "molt_cpython_ratio": r.molt_speedup,
                "output_match": r.output_match,
                "status": r.status,
            }
    if bench_baseline:
        report["benchmarks"] = bench_baseline

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
                    run_single_test, script, suite_name, molt_cmd, python_cmd, timeout_s
                )
                futures[fut] = script

            for fut in as_completed(futures):
                r = fut.result()
                results.append(r)
                print_result_line(colors, r, verbose)
    else:
        for script in scripts:
            r = run_single_test(script, suite_name, molt_cmd, python_cmd, timeout_s)
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
    parser = argparse.ArgumentParser(
        description="Molt unified benchmark and differential testing harness.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--bench", action="store_true", help="Run bench/*.py benchmarks")
    parser.add_argument("--parity", action="store_true", help="Run tests/parity/*.py parity checks")
    parser.add_argument("--diff", action="store_true", help="Run tests/differential/basic/*.py")
    parser.add_argument("--full", action="store_true", help="Run all suites")
    parser.add_argument("--filter", type=str, default=None, help="Glob/substring filter for test names")
    parser.add_argument("--parallel", type=int, default=1, help="Parallel test workers (default: 1)")
    parser.add_argument("--update-baseline", action="store_true", help="Save results as new baseline")
    parser.add_argument("--timeout", type=float, default=30.0, help="Per-test timeout in seconds")
    parser.add_argument("--molt", type=str, default=None, help="Path to molt binary")
    parser.add_argument("--python", type=str, default="python3", help="Path to CPython binary")
    parser.add_argument("--output", type=str, default=str(DEFAULT_OUTPUT), help="JSON output path")
    parser.add_argument("--baseline", type=str, default=str(DEFAULT_BASELINE), help="Baseline JSON path")
    parser.add_argument("--verbose", action="store_true", help="Show diffs and stderr on failure")
    parser.add_argument("--no-color", action="store_true", help="Disable colored output")

    args = parser.parse_args()

    # If no mode specified, show help
    if not (args.bench or args.parity or args.diff or args.full):
        parser.print_help()
        sys.exit(1)

    if args.full:
        args.bench = args.parity = args.diff = True

    colors = Colors(enabled=not args.no_color)

    # Resolve molt command
    if args.molt:
        molt_bin = args.molt
    else:
        # Try common locations
        candidates = [
            REPO_ROOT / "target" / "release" / "molt",
            REPO_ROOT / "target" / "release-fast" / "molt",
            REPO_ROOT / "target" / "debug" / "molt",
        ]
        molt_bin = None
        for c in candidates:
            if c.exists():
                molt_bin = str(c)
                break
        if molt_bin is None:
            # Fall back to PATH
            molt_bin = "molt"

    molt_cmd = [molt_bin, "run"]

    print(f"{colors.BOLD}Molt Test Harness{colors.RESET}")
    print(f"  molt:    {' '.join(molt_cmd)}")
    print(f"  python:  {args.python}")
    print(f"  timeout: {args.timeout}s")
    print(f"  parallel: {args.parallel}")
    if args.filter:
        print(f"  filter:  {args.filter}")

    all_results = []
    summaries = []

    # --- Benchmark suite ---
    if args.bench:
        scripts = collect_bench_scripts(args.filter)
        results, summary = run_suite(
            "bench", scripts, molt_cmd, args.python, args.timeout,
            args.parallel, colors, args.verbose,
        )
        all_results.extend(results)
        summaries.append(summary)

    # --- Parity suite ---
    if args.parity:
        scripts = collect_parity_scripts(args.filter)
        results, summary = run_suite(
            "parity", scripts, molt_cmd, args.python, args.timeout,
            args.parallel, colors, args.verbose,
        )
        all_results.extend(results)
        summaries.append(summary)

    # --- Differential suite ---
    if args.diff:
        scripts = collect_diff_scripts(args.filter)
        results, summary = run_suite(
            "diff", scripts, molt_cmd, args.python, args.timeout,
            args.parallel, colors, args.verbose,
        )
        all_results.extend(results)
        summaries.append(summary)

    # --- Regression detection ---
    baseline_path = Path(args.baseline)
    regressions = detect_regressions(all_results, baseline_path)

    # --- Summary ---
    print_summary_table(colors, summaries, regressions)

    # --- JSON output ---
    report = build_json_report(all_results, summaries, regressions)
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "w") as f:
        json.dump(report, f, indent=2, default=str)
    print(f"  JSON report written to: {output_path}")

    # --- Update baseline ---
    if args.update_baseline:
        with open(baseline_path, "w") as f:
            json.dump(report, f, indent=2, default=str)
        print(f"  Baseline updated: {baseline_path}")

    print()

    # Exit code: non-zero if any failures/errors or regressions
    has_failures = any(s.failed > 0 or s.errors > 0 for s in summaries)
    if regressions or has_failures:
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
