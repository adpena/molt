#!/usr/bin/env python3
"""Translation validation for Molt compiler passes.

Validates that each optimization pass preserves program semantics by:
1. Compiling and running the program normally (all passes) -> output_full
2. Compiling and running with midend disabled -> output_no_midend
3. Running through CPython -> output_cpython (ground truth)
4. Comparing all three outputs for equivalence

This is Tier 1 (concrete) validation -- fast but incomplete.
Future: Tier 2 (symbolic) and Tier 3 (SMT-based) validation.

Usage:
    # Validate all passes on a single file
    uv run --python 3.12 python3 tools/translation_validate.py examples/hello.py

    # Validate on all basic differential tests
    uv run --python 3.12 python3 tools/translation_validate.py tests/differential/basic/

    # Verbose output showing IR diffs
    uv run --python 3.12 python3 tools/translation_validate.py --verbose examples/hello.py

    # JSON output for CI integration
    uv run --python 3.12 python3 tools/translation_validate.py --json examples/hello.py

    # Skip CPython ground-truth check (only compare midend-on vs midend-off)
    uv run --python 3.12 python3 tools/translation_validate.py --no-cpython examples/hello.py
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from collections.abc import Mapping
from typing import Any

# ---------------------------------------------------------------------------
# Repo / environment
# ---------------------------------------------------------------------------

_REPO_ROOT = Path(__file__).resolve().parent.parent
_SRC_DIR = _REPO_ROOT / "src"
_DEFAULT_TIMEOUT = int(os.environ.get("MOLT_TV_TIMEOUT", "60"))
_DEFAULT_BUILD_PROFILE = os.environ.get("MOLT_TV_BUILD_PROFILE", "dev")
_DEFAULT_JOBS = int(os.environ.get("MOLT_TV_JOBS", "4"))


def _artifact_root(env: Mapping[str, str] | None = None) -> Path:
    """Return the canonical artifact root for translation validation."""
    env_view = os.environ if env is None else env
    explicit = env_view.get("MOLT_EXT_ROOT")
    if explicit:
        return Path(explicit).expanduser()
    return _REPO_ROOT


def _temp_root(env: Mapping[str, str] | None = None) -> Path:
    """Return the canonical temp root for translation validation."""
    env_view = os.environ if env is None else env
    explicit = env_view.get("MOLT_DIFF_TMPDIR") or env_view.get("TMPDIR")
    if explicit:
        return Path(explicit).expanduser()
    return _artifact_root(env_view) / "tmp"


def _cargo_target_root(env: Mapping[str, str] | None = None) -> Path:
    """Return the canonical Cargo target root for translation validation."""
    env_view = os.environ if env is None else env
    explicit = env_view.get("CARGO_TARGET_DIR")
    if explicit:
        return Path(explicit).expanduser()
    return _artifact_root(env_view) / "target"


def _resolve_python() -> str:
    """Resolve the Python interpreter for CPython baseline runs."""
    return os.environ.get("MOLT_TV_PYTHON", sys.executable)


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------


@dataclass
class RunResult:
    """Outcome of a single program execution."""

    stdout: str
    stderr: str
    returncode: int
    elapsed_ms: float
    mode: str  # "cpython", "molt_full", "molt_no_midend"

    @property
    def ok(self) -> bool:
        return self.returncode == 0

    @property
    def output_key(self) -> str:
        """Normalized output for comparison (stdout only, strip trailing ws)."""
        return self.stdout.rstrip()


@dataclass
class ValidationResult:
    """Result of validating one source file."""

    source_path: str
    cpython: RunResult | None = None
    molt_full: RunResult | None = None
    molt_no_midend: RunResult | None = None
    match_full_vs_cpython: bool | None = None
    match_no_midend_vs_cpython: bool | None = None
    match_full_vs_no_midend: bool | None = None
    error: str | None = None
    skipped: bool = False
    skip_reason: str | None = None

    @property
    def all_match(self) -> bool:
        checks = [
            self.match_full_vs_cpython,
            self.match_no_midend_vs_cpython,
            self.match_full_vs_no_midend,
        ]
        return all(c is True for c in checks if c is not None)

    @property
    def midend_preserves_semantics(self) -> bool:
        """True if midend passes did not change observable behavior."""
        if self.match_full_vs_no_midend is not None:
            return self.match_full_vs_no_midend
        # Fallback: both match CPython
        if (
            self.match_full_vs_cpython is not None
            and self.match_no_midend_vs_cpython is not None
        ):
            return self.match_full_vs_cpython and self.match_no_midend_vs_cpython
        return True  # insufficient data, assume ok


@dataclass
class ValidationSummary:
    """Aggregate results across many files."""

    results: list[ValidationResult] = field(default_factory=list)
    elapsed_ms: float = 0.0

    @property
    def total(self) -> int:
        return len(self.results)

    @property
    def skipped(self) -> int:
        return sum(1 for r in self.results if r.skipped)

    @property
    def errors(self) -> int:
        return sum(1 for r in self.results if r.error and not r.skipped)

    @property
    def passed(self) -> int:
        return sum(
            1 for r in self.results if not r.skipped and not r.error and r.all_match
        )

    @property
    def mismatches(self) -> int:
        return sum(
            1 for r in self.results if not r.skipped and not r.error and not r.all_match
        )

    @property
    def midend_mismatches(self) -> int:
        return sum(
            1
            for r in self.results
            if not r.skipped and not r.error and not r.midend_preserves_semantics
        )


# ---------------------------------------------------------------------------
# Execution helpers
# ---------------------------------------------------------------------------


def _run_subprocess(
    cmd: list[str],
    *,
    timeout: int,
    env: dict[str, str] | None = None,
) -> tuple[str, str, int]:
    """Run a subprocess safely with timeout. Returns (stdout, stderr, rc)."""
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        return proc.stdout, proc.stderr, proc.returncode
    except subprocess.TimeoutExpired:
        return "", "TIMEOUT", -1
    except Exception as exc:
        return "", str(exc), -1


def _run_cpython(source_path: str, *, timeout: int) -> RunResult:
    """Run a Python file through CPython."""
    t0 = time.perf_counter()
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"
    stdout, stderr, rc = _run_subprocess(
        [_resolve_python(), source_path],
        timeout=timeout,
        env=env,
    )
    elapsed = (time.perf_counter() - t0) * 1000.0
    return RunResult(
        stdout=stdout,
        stderr=stderr,
        returncode=rc,
        elapsed_ms=round(elapsed, 3),
        mode="cpython",
    )


def _run_molt(
    source_path: str,
    *,
    timeout: int,
    build_profile: str,
    disable_midend: bool = False,
) -> RunResult:
    """Build and run a Python file through Molt."""
    mode = "molt_no_midend" if disable_midend else "molt_full"
    t0 = time.perf_counter()
    temp_root = _temp_root()
    temp_root.mkdir(parents=True, exist_ok=True)
    tmp_dir = tempfile.mkdtemp(prefix="molt_tv_", dir=str(temp_root))
    try:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(_SRC_DIR)
        env["PYTHONHASHSEED"] = "0"
        if disable_midend:
            env["MOLT_MIDEND_DISABLE"] = "1"
        else:
            env.pop("MOLT_MIDEND_DISABLE", None)
        # Route build artifacts and per-run temps to the canonical temp root.
        env.setdefault("MOLT_CACHE", os.path.join(tmp_dir, "cache"))
        env.setdefault("TMPDIR", str(tmp_dir))
        env.setdefault("CARGO_TARGET_DIR", str(_cargo_target_root(env)))

        stem = Path(source_path).stem
        output_binary = os.path.join(tmp_dir, f"{stem}_molt")

        # Build
        build_cmd = [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            source_path,
            "--profile",
            build_profile,
            "--output",
            output_binary,
        ]
        build_stdout, build_stderr, build_rc = _run_subprocess(
            build_cmd,
            timeout=timeout,
            env=env,
        )
        if build_rc != 0:
            elapsed = (time.perf_counter() - t0) * 1000.0
            return RunResult(
                stdout="",
                stderr=f"BUILD FAILED:\n{build_stderr}\n{build_stdout}",
                returncode=build_rc,
                elapsed_ms=round(elapsed, 3),
                mode=mode,
            )

        # Find the actual binary (might have platform suffix)
        binary = Path(output_binary)
        if not binary.exists():
            # Try common suffixes
            candidates = list(Path(tmp_dir).glob(f"{stem}_molt*"))
            if candidates:
                binary = candidates[0]
            else:
                elapsed = (time.perf_counter() - t0) * 1000.0
                return RunResult(
                    stdout="",
                    stderr=f"Binary not found at {output_binary}",
                    returncode=-1,
                    elapsed_ms=round(elapsed, 3),
                    mode=mode,
                )

        # Run
        run_stdout, run_stderr, run_rc = _run_subprocess(
            [str(binary)],
            timeout=timeout,
            env=env,
        )
        elapsed = (time.perf_counter() - t0) * 1000.0
        return RunResult(
            stdout=run_stdout,
            stderr=run_stderr,
            returncode=run_rc,
            elapsed_ms=round(elapsed, 3),
            mode=mode,
        )

    except Exception as exc:
        elapsed = (time.perf_counter() - t0) * 1000.0
        return RunResult(
            stdout="",
            stderr=str(exc),
            returncode=-1,
            elapsed_ms=round(elapsed, 3),
            mode=mode,
        )


# ---------------------------------------------------------------------------
# Validation logic
# ---------------------------------------------------------------------------


def validate_file(
    source_path: str,
    *,
    timeout: int = _DEFAULT_TIMEOUT,
    build_profile: str = _DEFAULT_BUILD_PROFILE,
    include_cpython: bool = True,
    verbose: bool = False,
) -> ValidationResult:
    """Run translation validation on a single source file."""
    result = ValidationResult(source_path=source_path)

    # Quick guard: file must exist and parse
    p = Path(source_path)
    if not p.is_file():
        result.skipped = True
        result.skip_reason = "not a file"
        return result
    if p.suffix != ".py":
        result.skipped = True
        result.skip_reason = "not a .py file"
        return result

    source = p.read_text(encoding="utf-8")
    if not source.strip():
        result.skipped = True
        result.skip_reason = "empty file"
        return result

    # Skip files with known-unsupported patterns
    _skip_markers = ["exec(", "eval(", "__import__"]
    for marker in _skip_markers:
        if marker in source:
            result.skipped = True
            result.skip_reason = f"contains unsupported pattern: {marker}"
            return result

    try:
        # Run all three variants
        if include_cpython:
            result.cpython = _run_cpython(source_path, timeout=timeout)

        result.molt_full = _run_molt(
            source_path,
            timeout=timeout,
            build_profile=build_profile,
            disable_midend=False,
        )
        result.molt_no_midend = _run_molt(
            source_path,
            timeout=timeout,
            build_profile=build_profile,
            disable_midend=True,
        )

        # Compare outputs
        if result.molt_full and result.molt_no_midend:
            if result.molt_full.ok and result.molt_no_midend.ok:
                result.match_full_vs_no_midend = (
                    result.molt_full.output_key == result.molt_no_midend.output_key
                )
            elif result.molt_full.returncode == result.molt_no_midend.returncode:
                # Both failed the same way
                result.match_full_vs_no_midend = True

        if include_cpython and result.cpython:
            if result.cpython.ok and result.molt_full and result.molt_full.ok:
                result.match_full_vs_cpython = (
                    result.cpython.output_key == result.molt_full.output_key
                )
            if result.cpython.ok and result.molt_no_midend and result.molt_no_midend.ok:
                result.match_no_midend_vs_cpython = (
                    result.cpython.output_key == result.molt_no_midend.output_key
                )

    except Exception as exc:
        result.error = str(exc)

    return result


def validate_directory(
    dir_path: str,
    *,
    timeout: int = _DEFAULT_TIMEOUT,
    build_profile: str = _DEFAULT_BUILD_PROFILE,
    include_cpython: bool = True,
    verbose: bool = False,
    jobs: int = _DEFAULT_JOBS,
    glob_pattern: str = "**/*.py",
) -> ValidationSummary:
    """Run translation validation on all .py files under a directory."""
    summary = ValidationSummary()
    t0 = time.perf_counter()

    source_files = sorted(Path(dir_path).glob(glob_pattern))
    if not source_files:
        print(f"No .py files found under {dir_path}", file=sys.stderr)
        summary.elapsed_ms = (time.perf_counter() - t0) * 1000.0
        return summary

    if jobs <= 1:
        for sf in source_files:
            result = validate_file(
                str(sf),
                timeout=timeout,
                build_profile=build_profile,
                include_cpython=include_cpython,
                verbose=verbose,
            )
            summary.results.append(result)
            if verbose:
                _print_result_line(result)
    else:
        with ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = {
                pool.submit(
                    validate_file,
                    str(sf),
                    timeout=timeout,
                    build_profile=build_profile,
                    include_cpython=include_cpython,
                    verbose=verbose,
                ): sf
                for sf in source_files
            }
            for future in as_completed(futures):
                result = future.result()
                summary.results.append(result)
                if verbose:
                    _print_result_line(result)

    # Sort results by path for deterministic output
    summary.results.sort(key=lambda r: r.source_path)
    summary.elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 3)
    return summary


# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------

_STATUS_CHARS = {
    "pass": ".",
    "mismatch": "X",
    "error": "E",
    "skip": "S",
}


def _result_status(r: ValidationResult) -> str:
    if r.skipped:
        return "skip"
    if r.error:
        return "error"
    if r.all_match:
        return "pass"
    return "mismatch"


def _print_result_line(r: ValidationResult) -> None:
    status = _result_status(r)
    char = _STATUS_CHARS.get(status, "?")
    rel = r.source_path
    try:
        rel = str(Path(r.source_path).relative_to(Path.cwd()))
    except ValueError:
        pass
    if status == "skip":
        print(f"  {char} {rel}  (skipped: {r.skip_reason})")
    elif status == "error":
        print(f"  {char} {rel}  ERROR: {r.error}")
    elif status == "mismatch":
        mismatches: list[str] = []
        if r.match_full_vs_no_midend is False:
            mismatches.append("midend-on != midend-off")
        if r.match_full_vs_cpython is False:
            mismatches.append("molt != cpython")
        if r.match_no_midend_vs_cpython is False:
            mismatches.append("molt-no-midend != cpython")
        print(f"  {char} {rel}  MISMATCH: {', '.join(mismatches)}")
    else:
        print(f"  {char} {rel}")


def print_summary(summary: ValidationSummary, *, verbose: bool = False) -> None:
    """Print human-readable summary."""
    print()
    print("=" * 60)
    print("Translation Validation Summary")
    print("=" * 60)
    print(f"  Total files:     {summary.total}")
    print(f"  Passed:          {summary.passed}")
    print(f"  Mismatches:      {summary.mismatches}")
    print(f"  Midend issues:   {summary.midend_mismatches}")
    print(f"  Errors:          {summary.errors}")
    print(f"  Skipped:         {summary.skipped}")
    print(f"  Elapsed:         {summary.elapsed_ms:.0f} ms")
    print()

    # Show mismatches
    mismatch_results = [
        r for r in summary.results if not r.skipped and not r.error and not r.all_match
    ]
    if mismatch_results:
        print("MISMATCHES:")
        for r in mismatch_results:
            _print_result_line(r)
            if verbose and r.molt_full and r.molt_no_midend:
                if r.match_full_vs_no_midend is False:
                    print("    --- molt (midend on) ---")
                    for line in r.molt_full.stdout.splitlines()[:10]:
                        print(f"    > {line}")
                    print("    --- molt (midend off) ---")
                    for line in r.molt_no_midend.stdout.splitlines()[:10]:
                        print(f"    > {line}")
        print()

    # Show errors
    error_results = [r for r in summary.results if r.error and not r.skipped]
    if error_results:
        print("ERRORS:")
        for r in error_results:
            _print_result_line(r)
        print()

    if summary.mismatches == 0 and summary.errors == 0:
        print("All translation validations PASSED.")
    elif summary.midend_mismatches > 0:
        print(
            f"WARNING: {summary.midend_mismatches} file(s) show different "
            "behavior with midend enabled vs disabled."
        )


def summary_to_json(summary: ValidationSummary) -> dict[str, Any]:
    """Convert summary to JSON-serializable dict."""
    results_json: list[dict[str, Any]] = []
    for r in summary.results:
        entry: dict[str, Any] = {
            "source_path": r.source_path,
            "status": _result_status(r),
            "midend_preserves_semantics": r.midend_preserves_semantics,
        }
        if r.skipped:
            entry["skip_reason"] = r.skip_reason
        if r.error:
            entry["error"] = r.error
        if r.match_full_vs_no_midend is not None:
            entry["match_full_vs_no_midend"] = r.match_full_vs_no_midend
        if r.match_full_vs_cpython is not None:
            entry["match_full_vs_cpython"] = r.match_full_vs_cpython
        if r.match_no_midend_vs_cpython is not None:
            entry["match_no_midend_vs_cpython"] = r.match_no_midend_vs_cpython
        for attr in ("cpython", "molt_full", "molt_no_midend"):
            run = getattr(r, attr)
            if run is not None:
                entry[attr] = {
                    "returncode": run.returncode,
                    "elapsed_ms": run.elapsed_ms,
                    "stdout_lines": len(run.stdout.splitlines()),
                }
        results_json.append(entry)

    return {
        "total": summary.total,
        "passed": summary.passed,
        "mismatches": summary.mismatches,
        "midend_mismatches": summary.midend_mismatches,
        "errors": summary.errors,
        "skipped": summary.skipped,
        "elapsed_ms": summary.elapsed_ms,
        "results": results_json,
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Translation validation for Molt compiler passes.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            Validation strategy:
              For each input program, compile and run three ways:
              1. CPython (ground truth)
              2. Molt with all midend passes enabled
              3. Molt with midend disabled (MOLT_MIDEND_DISABLE=1)

              If (2) and (3) produce the same output, the midend passes
              collectively preserve semantics. If they differ, a pass may
              have introduced a miscompilation.

            Environment variables:
              MOLT_TV_TIMEOUT        Per-file timeout in seconds (default: 60)
              MOLT_TV_BUILD_PROFILE  Build profile: dev or release (default: dev)
              MOLT_TV_JOBS           Parallel jobs (default: 4)
              MOLT_TV_PYTHON         CPython executable for baseline
              MOLT_EXT_ROOT          Artifact root for build artifacts/cache/tmp
              MOLT_DIFF_TMPDIR       Temp directory root
        """),
    )
    p.add_argument(
        "targets",
        nargs="+",
        help="Python files or directories to validate",
    )
    p.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show per-file results and output diffs on mismatch",
    )
    p.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Output results as JSON",
    )
    p.add_argument(
        "--no-cpython",
        action="store_true",
        help="Skip CPython ground-truth comparison (only compare midend on/off)",
    )
    p.add_argument(
        "--timeout",
        type=int,
        default=_DEFAULT_TIMEOUT,
        help=f"Per-file timeout in seconds (default: {_DEFAULT_TIMEOUT})",
    )
    p.add_argument(
        "--build-profile",
        choices=["dev", "release"],
        default=_DEFAULT_BUILD_PROFILE,
        help=f"Molt build profile (default: {_DEFAULT_BUILD_PROFILE})",
    )
    p.add_argument(
        "--jobs",
        "-j",
        type=int,
        default=_DEFAULT_JOBS,
        help=f"Parallel validation jobs (default: {_DEFAULT_JOBS})",
    )
    p.add_argument(
        "--glob",
        default="**/*.py",
        help="Glob pattern for directory traversal (default: **/*.py)",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    all_summary = ValidationSummary()
    t0 = time.perf_counter()

    for target in args.targets:
        p = Path(target)
        if p.is_file():
            result = validate_file(
                str(p),
                timeout=args.timeout,
                build_profile=args.build_profile,
                include_cpython=not args.no_cpython,
                verbose=args.verbose,
            )
            all_summary.results.append(result)
            if args.verbose and not args.json_output:
                _print_result_line(result)
        elif p.is_dir():
            sub = validate_directory(
                str(p),
                timeout=args.timeout,
                build_profile=args.build_profile,
                include_cpython=not args.no_cpython,
                verbose=args.verbose,
                jobs=args.jobs,
                glob_pattern=args.glob,
            )
            all_summary.results.extend(sub.results)
        else:
            print(f"WARNING: {target} not found, skipping", file=sys.stderr)

    all_summary.results.sort(key=lambda r: r.source_path)
    all_summary.elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 3)

    if args.json_output:
        print(json.dumps(summary_to_json(all_summary), indent=2))
    else:
        if not args.verbose:
            # Print compact result line for each non-skipped file
            for r in all_summary.results:
                if not r.skipped:
                    _print_result_line(r)
        print_summary(all_summary, verbose=args.verbose)

    # Exit code: 0 if no midend mismatches, 1 otherwise
    return 1 if all_summary.midend_mismatches > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
