#!/usr/bin/env python3
"""Translation validation for Molt compiler passes.

Validates that the compiled Molt pipeline preserves program semantics by:
1. Running through CPython -> output_cpython (ground truth)
2. Compiling and running through Molt -> output_molt
3. Comparing both outputs for equivalence

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

    # Explicit Python target custody
    uv run --python 3.14 python3 tools/translation_validate.py --python-version 3.14 examples/hello.py

    # Skip CPython ground-truth check; require Molt build/run success only
    uv run --python 3.12 python3 tools/translation_validate.py --no-cpython examples/hello.py
"""

from __future__ import annotations

import argparse
import functools
import json
import os
import shlex
import shutil
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
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools import harness_memory_guard  # noqa: E402

_SRC_DIR = _REPO_ROOT / "src"
if str(_SRC_DIR) not in sys.path:
    sys.path.insert(0, str(_SRC_DIR))

import molt.cli as molt_cli  # noqa: E402
from molt.cli import build_inputs as cli_build_inputs  # noqa: E402
from molt.dx import cargo_target_dir_for_artifact_root  # noqa: E402

_DEFAULT_TIMEOUT = int(os.environ.get("MOLT_TV_TIMEOUT", "60"))
_DEFAULT_BUILD_PROFILE = os.environ.get("MOLT_TV_BUILD_PROFILE", "dev")
_DEFAULT_JOBS = int(os.environ.get("MOLT_TV_JOBS", "4"))
_PYTHON_VERSION_PROBE = (
    "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}', end='')"
)


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
    return cargo_target_dir_for_artifact_root(
        _artifact_root(env_view),
        env_view.get("MOLT_SESSION_ID") or f"translation-validate-{os.getpid()}",
    )


def _resolve_target_python(
    explicit: str | None,
    *,
    project_root: Path = _REPO_ROOT,
) -> molt_cli.TargetPythonVersion:
    return molt_cli._resolve_target_python_version(
        explicit=explicit,
        build_config=molt_cli._resolve_build_config(
            cli_build_inputs._load_molt_config(project_root)
        ),
        project_root=project_root,
    )


def _target_python_command_candidates(
    target_python: molt_cli.TargetPythonVersion,
    *,
    override: str | None,
) -> list[list[str]]:
    """Return explicit candidate commands for a target CPython minor."""
    if override is not None and override.strip():
        explicit = override.strip()
        if Path(explicit).expanduser().exists():
            return [[explicit]]
        parsed = shlex.split(explicit, posix=os.name != "nt")
        return [parsed if parsed else [explicit]]
    candidates: list[list[str]] = []
    if os.name == "nt":
        candidates.append(["py", f"-{target_python.short}"])
    candidates.append([f"python{target_python.short}"])
    return candidates


def _verify_target_python_command(
    command: list[str],
    *,
    target_python: molt_cli.TargetPythonVersion,
    env: Mapping[str, str],
) -> tuple[bool, str]:
    probe_env = dict(env)
    probe_env["PYTHONHASHSEED"] = "0"
    stdout, stderr, rc = _run_subprocess(
        [*command, "-c", _PYTHON_VERSION_PROBE],
        timeout=10,
        env=probe_env,
        cwd=_REPO_ROOT,
    )
    if rc != 0:
        detail = (stderr or stdout).strip()
        return False, detail or f"returncode={rc}"
    actual = stdout.strip()
    if actual != target_python.short:
        return False, f"reported Python {actual or '<empty>'}"
    return True, ""


@functools.lru_cache(maxsize=16)
def _target_python_command_cached(
    target_python_short: str,
    override: str,
) -> tuple[str, ...]:
    target_python = molt_cli._parse_target_python_version(target_python_short)
    env = os.environ.copy()
    failures: list[str] = []
    if not override:
        uv = shutil.which("uv")
        if uv is not None:
            stdout, stderr, rc = _run_subprocess(
                [uv, "python", "find", target_python.short],
                timeout=10,
                env=env,
                cwd=_REPO_ROOT,
            )
            if rc == 0 and stdout.strip():
                candidate = [stdout.splitlines()[0].strip()]
                ok, detail = _verify_target_python_command(
                    candidate,
                    target_python=target_python,
                    env=env,
                )
                if ok:
                    return tuple(candidate)
                failures.append(f"{' '.join(candidate)}: {detail}")
            else:
                detail = (stderr or stdout).strip()
                failures.append(
                    f"{uv} python find {target_python.short}: "
                    f"{detail or f'returncode={rc}'}"
                )
    for candidate in _target_python_command_candidates(
        target_python,
        override=override or None,
    ):
        ok, detail = _verify_target_python_command(
            candidate,
            target_python=target_python,
            env=env,
        )
        if ok:
            return tuple(candidate)
        failures.append(f"{' '.join(candidate)}: {detail}")
    attempted = "; ".join(failures) if failures else "no candidates"
    raise RuntimeError(
        f"no verified CPython {target_python.short} command available for "
        f"translation validation ({attempted})"
    )


def _target_python_command(
    target_python: molt_cli.TargetPythonVersion,
) -> list[str]:
    return list(
        _target_python_command_cached(
            target_python.short,
            os.environ.get("MOLT_TV_PYTHON", "").strip(),
        )
    )


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
    mode: str  # "cpython", "molt"

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
    molt: RunResult | None = None
    match_molt_vs_cpython: bool | None = None
    error: str | None = None
    skipped: bool = False
    skip_reason: str | None = None

    @property
    def all_match(self) -> bool:
        if self.molt is None or not self.molt.ok:
            return False
        if self.cpython is not None and not self.cpython.ok:
            return False
        if self.match_molt_vs_cpython is not None:
            return self.match_molt_vs_cpython
        if self.cpython is not None:
            return False
        return True


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


# ---------------------------------------------------------------------------
# Execution helpers
# ---------------------------------------------------------------------------


def _run_subprocess(
    cmd: list[str],
    *,
    timeout: int,
    env: dict[str, str] | None = None,
    cwd: Path | str | None = None,
) -> tuple[str, str, int]:
    """Run a subprocess safely with timeout. Returns (stdout, stderr, rc)."""
    limits = harness_memory_guard.limits_from_env("MOLT_CONFORMANCE", env)
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_CONFORMANCE",
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=cwd,
            limits=limits,
        )
        return proc.stdout, proc.stderr, proc.returncode
    except subprocess.TimeoutExpired:
        return "", "TIMEOUT", -1
    except Exception as exc:
        return "", str(exc), -1


def _run_cpython(
    source_path: str,
    *,
    timeout: int,
    target_python: molt_cli.TargetPythonVersion,
) -> RunResult:
    """Run a Python file through CPython."""
    t0 = time.perf_counter()
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"
    stdout, stderr, rc = _run_subprocess(
        [*_target_python_command(target_python), source_path],
        timeout=timeout,
        env=env,
        cwd=_REPO_ROOT,
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
    target_python: molt_cli.TargetPythonVersion,
) -> RunResult:
    """Build and run a Python file through Molt."""
    mode = "molt"
    t0 = time.perf_counter()
    temp_root = _temp_root()
    temp_root.mkdir(parents=True, exist_ok=True)
    tmp_dir = tempfile.mkdtemp(prefix="molt_tv_", dir=str(temp_root))
    try:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(_SRC_DIR)
        env["PYTHONHASHSEED"] = "0"
        # Route build artifacts and per-run temps to the canonical temp root.
        env.setdefault("MOLT_CACHE", os.path.join(tmp_dir, "cache"))
        env.setdefault("TMPDIR", str(tmp_dir))
        env.setdefault("CARGO_TARGET_DIR", str(_cargo_target_root(env)))

        stem = Path(source_path).stem
        output_binary = os.path.join(tmp_dir, f"{stem}_molt")

        # Build
        build_cmd = [
            *_target_python_command(target_python),
            "-m",
            "molt.cli",
            "build",
            source_path,
            "--python-version",
            target_python.short,
            "--profile",
            build_profile,
            "--output",
            output_binary,
        ]
        build_stdout, build_stderr, build_rc = _run_subprocess(
            build_cmd,
            timeout=timeout,
            env=env,
            cwd=_REPO_ROOT,
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
            cwd=_REPO_ROOT,
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
    target_python: molt_cli.TargetPythonVersion = molt_cli._DEFAULT_TARGET_PYTHON_VERSION,
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
        # Run the compiled pipeline and optional CPython ground truth.
        if include_cpython:
            result.cpython = _run_cpython(
                source_path,
                timeout=timeout,
                target_python=target_python,
            )

        result.molt = _run_molt(
            source_path,
            timeout=timeout,
            build_profile=build_profile,
            target_python=target_python,
        )

        # Compare outputs
        if include_cpython and result.cpython:
            if result.cpython.ok and result.molt and result.molt.ok:
                result.match_molt_vs_cpython = (
                    result.cpython.output_key == result.molt.output_key
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
    target_python: molt_cli.TargetPythonVersion = molt_cli._DEFAULT_TARGET_PYTHON_VERSION,
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
                target_python=target_python,
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
                    target_python=target_python,
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
        if r.molt is not None and not r.molt.ok:
            mismatches.append("molt failed")
        if r.match_molt_vs_cpython is False:
            mismatches.append("molt != cpython")
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
            if verbose and r.cpython and r.molt and r.match_molt_vs_cpython is False:
                print("    --- cpython ---")
                for line in r.cpython.stdout.splitlines()[:10]:
                    print(f"    > {line}")
                print("    --- molt ---")
                for line in r.molt.stdout.splitlines()[:10]:
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


def summary_to_json(
    summary: ValidationSummary,
    *,
    memory_guard: dict[str, object] | None = None,
) -> dict[str, Any]:
    """Convert summary to JSON-serializable dict."""
    results_json: list[dict[str, Any]] = []
    for r in summary.results:
        entry: dict[str, Any] = {
            "source_path": r.source_path,
            "status": _result_status(r),
        }
        if r.skipped:
            entry["skip_reason"] = r.skip_reason
        if r.error:
            entry["error"] = r.error
        if r.match_molt_vs_cpython is not None:
            entry["match_molt_vs_cpython"] = r.match_molt_vs_cpython
        for attr in ("cpython", "molt"):
            run = getattr(r, attr)
            if run is not None:
                entry[attr] = {
                    "returncode": run.returncode,
                    "elapsed_ms": run.elapsed_ms,
                    "stdout_lines": len(run.stdout.splitlines()),
                }
        results_json.append(entry)

    payload: dict[str, Any] = {
        "total": summary.total,
        "passed": summary.passed,
        "mismatches": summary.mismatches,
        "errors": summary.errors,
        "skipped": summary.skipped,
        "elapsed_ms": summary.elapsed_ms,
        "results": results_json,
    }
    if memory_guard is not None:
        payload["memory_guard"] = memory_guard
    return payload


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Translation validation for Molt compiler passes.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            Validation strategy:
              For each input program, compile and run two ways:
              1. CPython (ground truth)
              2. Molt compiled pipeline

            Environment variables:
              MOLT_TV_TIMEOUT        Per-file timeout in seconds (default: 60)
              MOLT_TV_BUILD_PROFILE  Build profile: dev or release (default: dev)
              MOLT_TV_JOBS           Parallel jobs (default: 4)
              MOLT_TV_PYTHON         Explicit target CPython command override
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
        help="Skip CPython ground-truth comparison; require Molt build/run success.",
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
        "--python-version",
        default=None,
        help=(
            "Target Python semantics and CPython baseline version "
            "(3.12, 3.13, or 3.14). Defaults from [tool.molt.build] or "
            "project.requires-python."
        ),
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
    try:
        target_python = _resolve_target_python(args.python_version)
    except ValueError as exc:
        parser.error(str(exc))

    guard_env = os.environ.copy()
    limits = harness_memory_guard.limits_from_env("MOLT_CONFORMANCE", guard_env)
    with harness_memory_guard.guarded_harness_scope(
        prefix="MOLT_CONFORMANCE",
        repo_root=_REPO_ROOT,
        artifact_root=_artifact_root(guard_env) / "tmp" / "translation_validate",
        label="translation_validate",
        env=guard_env,
        limits=limits,
    ) as guard_scope:
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
                    target_python=target_python,
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
                    target_python=target_python,
                )
                all_summary.results.extend(sub.results)
            else:
                print(f"WARNING: {target} not found, skipping", file=sys.stderr)

        all_summary.results.sort(key=lambda r: r.source_path)
        all_summary.elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 3)
        memory_guard_summary = guard_scope.memory_guard
        memory_guard_status = harness_memory_guard.limits_status_line(
            guard_scope.limits
        )

    if args.json_output:
        print(
            json.dumps(
                summary_to_json(
                    all_summary,
                    memory_guard=memory_guard_summary,
                ),
                indent=2,
            )
        )
    else:
        if not args.verbose:
            # Print compact result line for each non-skipped file
            for r in all_summary.results:
                if not r.skipped:
                    _print_result_line(r)
        print(memory_guard_status)
        print_summary(all_summary, verbose=args.verbose)

    return 0 if all_summary.mismatches == 0 and all_summary.errors == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
