from __future__ import annotations

import argparse
import os
import tempfile
import textwrap
import time
from pathlib import Path

from tools.fuzz_compiler_compile_only import CompileOnlyFuzzer
from tools.fuzz_compiler_driver import run_generate_only, run_reject_fuzzer, run_safe_fuzzer
from tools.fuzz_compiler_execution import _build_env
from tools.fuzz_compiler_reporting import _log
from tools.fuzz_compiler_shrink import _shrink_program
from tools.fuzz_compiler_types import FuzzSummary

def _print_summary(summary: FuzzSummary, mode: str) -> None:
    _log("")
    _log("=" * 60)
    _log(f"FUZZ SUMMARY (mode={mode})")
    _log("=" * 60)
    _log(f"  Total programs:    {summary.total}")

    if mode == "safe":
        _log(f"  Passed:            {summary.passed}")
        _log(f"  Mismatches:        {summary.mismatches}")
        _log(f"  Build errors:      {summary.build_errors}")
        _log(f"  Molt run errors:   {summary.molt_run_errors}")
        _log(f"  CPython errors:    {summary.cpython_errors} (skipped)")
        _log(f"  Timeouts:          {summary.timeouts}")
        _log("")
        effective = summary.total - summary.cpython_errors
        if effective > 0:
            pass_rate = summary.passed / effective * 100
            _log(f"  Pass rate: {summary.passed}/{effective} ({pass_rate:.1f}%)")
        else:
            _log("  Pass rate: N/A (no effective test programs)")
        if summary.cpython_errors > 0:
            cpython_clean = (
                (summary.total - summary.cpython_errors) / summary.total * 100
            )
            _log(
                f"  CPython clean rate: {summary.total - summary.cpython_errors}/{summary.total} ({cpython_clean:.1f}%)"
            )
        if summary.mismatches > 0:
            _log("")
            _log(f"  {summary.mismatches} MISMATCH(ES) FOUND")
            for r in summary.failures:
                if r.status == "mismatch":
                    _log(f"    seed={r.seed}")
        if summary.build_errors > 0:
            _log("")
            _log(f"  {summary.build_errors} BUILD ERROR(S)")
            for r in summary.failures:
                if r.status == "build_error":
                    _log(f"    seed={r.seed}: {r.error_detail[:120]}")

    elif mode == "reject":
        _log(f"  Correctly rejected: {summary.reject_pass}")
        _log(f"  Reject failures:    {summary.reject_fail}")
        _log(f"  Timeouts:           {summary.timeouts}")
        if summary.total > 0:
            reject_rate = summary.reject_pass / summary.total * 100
            _log(f"  Rejection rate: {reject_rate:.1f}%")

    elif mode == "compile-only":
        _log(f"  OK (compiled or cleanly rejected): {summary.compile_only_ok}")
        _log(f"  Crashes:                           {summary.compile_only_crash}")
        _log(f"  Timeouts:                          {summary.timeouts}")
        if summary.total > 0:
            ok_rate = (summary.compile_only_ok) / summary.total * 100
            _log(f"  OK rate: {ok_rate:.1f}%")

    _log("=" * 60)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Comprehensive compiler fuzzer for Molt.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            examples:
              python tools/fuzz_compiler.py --mode safe --count 100 --seed 42
              python tools/fuzz_compiler.py --mode reject --count 50
              python tools/fuzz_compiler.py --mode compile-only --count 200
              python tools/fuzz_compiler.py --count 10 --seed 0 --verbose
              python tools/fuzz_compiler.py --count 500 --output-dir /tmp/fuzz_failures
        """),
    )
    parser.add_argument(
        "--mode",
        choices=["safe", "reject", "compile-only"],
        default="safe",
        help="Fuzzing mode (default: safe)",
    )
    parser.add_argument("--count", "-n", type=int, default=100)
    parser.add_argument("--seed", "-s", type=int, default=None)
    parser.add_argument("--output-dir", "--out-dir", "-o", type=str, default=None)
    parser.add_argument("--build-profile", type=str, default="dev")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument(
        "--generate-only",
        action="store_true",
        help="Only generate programs to --output-dir, do not run differential tests.",
    )
    parser.add_argument(
        "--shrink",
        action="store_true",
        help="Attempt to minimize failing programs after the run.",
    )

    args = parser.parse_args()
    if args.seed is None:
        args.seed = int(time.time()) % (2**31)
        _log(f"Using auto-generated seed: {args.seed}")

    output_dir = Path(args.output_dir) if args.output_dir else None

    # --generate-only: write programs to disk without running them.
    if args.generate_only:
        if output_dir is None:
            output_dir = Path("/tmp/fuzz_molt")
        run_generate_only(count=args.count, seed=args.seed, output_dir=output_dir)
        return 0

    if args.mode == "safe":
        summary = run_safe_fuzzer(
            count=args.count,
            seed=args.seed,
            output_dir=output_dir,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
        )
        _print_summary(summary, "safe")

        # Shrink failures if requested.
        if args.shrink and summary.failures:
            env = _build_env()
            ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
            tmpdir_base = (
                ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
            )
            _log("")
            _log(f"Shrinking {len(summary.failures)} failure(s)...")
            with tempfile.TemporaryDirectory(
                prefix="molt_fuzz_shrink_", dir=tmpdir_base
            ) as tmpdir:
                for r in summary.failures:
                    if r.status not in ("mismatch", "build_error", "molt_run_error"):
                        continue
                    original_lines = len(r.source.splitlines())
                    shrunk = _shrink_program(
                        result=r,
                        profile=args.build_profile,
                        timeout=args.timeout,
                        env=env,
                        tmpdir=tmpdir,
                    )
                    new_lines = len(shrunk.source.splitlines())
                    _log(f"  seed={r.seed}: {original_lines} -> {new_lines} lines")
                    if output_dir:
                        min_path = output_dir / f"fuzz_{r.program_id:06d}_minimized.py"
                        min_path.write_text(shrunk.source)
                        _log(f"    -> {min_path}")

        if summary.mismatches > 0 or summary.molt_run_errors > 0:
            return 1
        return 0

    elif args.mode == "reject":
        summary = run_reject_fuzzer(
            count=args.count,
            seed=args.seed,
            output_dir=output_dir,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
        )
        _print_summary(summary, "reject")
        if summary.reject_fail > 0:
            return 1
        return 0

    elif args.mode == "compile-only":
        fuzzer = CompileOnlyFuzzer()
        summary = fuzzer.run(
            count=args.count,
            seed=args.seed,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
            output_dir=output_dir,
        )
        _print_summary(summary, "compile-only")
        if summary.compile_only_crash > 0:
            return 1
        return 0

    return 0
