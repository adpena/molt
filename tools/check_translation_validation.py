#!/usr/bin/env python3
"""Translation validation: verify Molt-compiled binaries match CPython output.

This is a paranoid verification mode that catches silent miscompilations by
compiling a Python program with Molt, running both the compiled binary and the
original program under CPython, and comparing their stdout, stderr, and exit
codes.

Unlike the full differential test harness (tests/molt_diff.py), this is a
lightweight standalone tool with no external dependencies beyond the Python
standard library. It is intended for quick spot-checks, CI smoke tests, and
ad-hoc validation of individual files or small directories.

Usage:
    python tools/check_translation_validation.py [OPTIONS] source.py
    python tools/check_translation_validation.py --batch DIR [OPTIONS]

Examples:
    # Validate a single file
    python tools/check_translation_validation.py examples/hello.py

    # Validate all .py files in a directory
    python tools/check_translation_validation.py --batch tests/differential/basic/

    # With custom build profile and timeout
    python tools/check_translation_validation.py --build-profile release --timeout 60 examples/hello.py

Exit codes:
    0 -- all validated files match CPython
    1 -- at least one file produced mismatched output
    2 -- usage error, missing file, or build infrastructure failure
"""

import argparse
import difflib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

TARGET_NATIVE = "native"
TARGET_RUST = "rust"
TARGET_LUAU = "luau"
TARGET_ALL = "all"
TARGET_CHOICES = (TARGET_NATIVE, TARGET_RUST, TARGET_LUAU, TARGET_ALL)
DEFAULT_EXTERNAL_ROOT = Path("/Volumes/APDataStore/Molt")


def _repo_root() -> Path:
    """Return the repository root (parent of tools/)."""
    return Path(__file__).resolve().parents[1]


def _make_env() -> dict[str, str]:
    """Build a clean environment for both CPython and Molt runs.

    Sets PYTHONPATH to include the Molt source tree and pins
    PYTHONHASHSEED=0 for reproducible hash ordering.
    """
    env = os.environ.copy()
    existing = env.get("PYTHONPATH", "")
    src_dir = str(_repo_root() / "src")
    if existing:
        # Prepend src if not already present
        parts = existing.split(os.pathsep)
        if src_dir not in parts:
            env["PYTHONPATH"] = src_dir + os.pathsep + existing
    else:
        env["PYTHONPATH"] = src_dir
    env["PYTHONHASHSEED"] = "0"
    ext_root_raw = env.get("MOLT_EXT_ROOT", "").strip()
    ext_root = Path(ext_root_raw).expanduser().resolve() if ext_root_raw else DEFAULT_EXTERNAL_ROOT
    if not ext_root.is_dir():
        raise RuntimeError(
            "External volume is required for translation validation artifacts. "
            f"Set MOLT_EXT_ROOT to a mounted external path (current: {ext_root})."
        )
    env.setdefault("MOLT_EXT_ROOT", str(ext_root))
    env.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    env.setdefault("TMPDIR", env["MOLT_DIFF_TMPDIR"])
    env.setdefault("MOLT_DEV_CARGO_PROFILE", "release-fast")
    env.setdefault("UV_NO_SYNC", "1")
    env.setdefault("UV_LINK_MODE", "copy")
    return env


def _temp_parent_from_env(env: dict[str, str]) -> str | None:
    """Return a writable temp parent path from env defaults."""
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR"):
        raw = env.get(key, "").strip()
        if not raw:
            continue
        path = Path(raw).expanduser().resolve()
        try:
            path.mkdir(parents=True, exist_ok=True)
        except OSError:
            continue
        if path.is_dir():
            return str(path)
    return None


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from Molt build JSON output.

    Handles the ``data`` envelope: ``d.get("data", d)`` then looks for
    the ``output`` key (or fallback keys used by various build versions).
    """
    data = build_json.get("data", build_json)
    if isinstance(data, dict):
        pass
    else:
        data = build_json
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


def _expand_targets(target: str) -> list[str]:
    """Expand the CLI target selector into concrete execution lanes."""
    if target == TARGET_ALL:
        return [TARGET_NATIVE, TARGET_RUST, TARGET_LUAU]
    return [target]


def _find_rustc() -> str | None:
    """Return a rustc executable path when available."""
    for candidate in ("rustc", os.path.expanduser("~/.cargo/bin/rustc")):
        resolved = shutil.which(candidate) if os.sep not in candidate else candidate
        if not resolved:
            continue
        try:
            probe = subprocess.run(
                [resolved, "--version"],
                capture_output=True,
                text=True,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        if probe.returncode == 0:
            return resolved
    return None


def _find_lune() -> str | None:
    """Return a Lune executable path when available."""
    override = os.environ.get("MOLT_LUNE", "").strip()
    candidates = [
        override,
        os.path.expanduser("~/.aftman/bin/lune"),
        "lune",
    ]
    for candidate in candidates:
        if not candidate:
            continue
        resolved = shutil.which(candidate) if os.sep not in candidate else candidate
        if not resolved or not Path(resolved).exists():
            continue
        try:
            probe = subprocess.run(
                [resolved, "--version"],
                capture_output=True,
                text=True,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        if probe.returncode == 0:
            return resolved
    return None


def run_cpython(
    source: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str, str, int]:
    """Run a Python file under CPython and return (stdout, stderr, exit_code)."""
    cmd = [sys.executable, source]
    if verbose:
        print(f"  CPython: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return "", f"CPython timed out after {timeout}s", -1
    return result.stdout, result.stderr, result.returncode


def build_molt(
    source: str,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str | None, str]:
    """Compile a Python file with Molt and return (binary_path, error_message).

    Returns (path, "") on success or (None, reason) on failure.
    """
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--json",
        "--capabilities",
        "fs,env,time,random",
        source,
    ]
    if verbose:
        print(f"  Build: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return None, f"Molt build timed out after {timeout}s"

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        return None, f"Molt build failed (exit {result.returncode}): {detail[:500]}"

    # Parse JSON from stdout -- the build may emit non-JSON lines before the
    # JSON payload, so try each line from the end.
    stdout = result.stdout.strip()
    build_info = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                build_info = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    if build_info is None:
        # Try the entire stdout as a single JSON blob
        try:
            build_info = json.loads(stdout)
        except json.JSONDecodeError:
            return None, f"Build produced no valid JSON. stdout: {stdout[:500]}"

    binary = _extract_binary(build_info)
    if binary is None:
        return None, (
            f"Cannot find binary in build JSON. Keys: {list(build_info.keys())}"
        )
    if not Path(binary).exists():
        return None, f"Binary path does not exist: {binary}"
    return binary, ""


def build_molt_native(
    source: str,
    profile: str,
    output: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[bool, str]:
    """Compile a Python file with Molt to an explicit native output path."""
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        source,
        "--profile",
        profile,
        "--capabilities",
        "fs,env,time,random",
        "--output",
        output,
    ]
    if verbose:
        print(f"  Build (native): {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return False, f"Molt build (native) timed out after {timeout}s"

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        return False, (
            f"Molt build (native) failed (exit {result.returncode}): {detail[:500]}"
        )
    if not Path(output).exists():
        return False, f"Build (native) produced no artifact: {output}"
    return True, ""


def build_molt_target(
    source: str,
    target: str,
    profile: str,
    output: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[bool, str]:
    """Build a non-native target artifact and return (ok, error_message)."""
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        source,
        "--target",
        target,
        "--profile",
        profile,
        "--capabilities",
        "fs,env,time,random",
        "--output",
        output,
    ]
    if verbose:
        print(f"  Build ({target}): {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return False, f"Molt build ({target}) timed out after {timeout}s"

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        return False, (
            f"Molt build ({target}) failed (exit {result.returncode}): {detail[:500]}"
        )

    if not Path(output).exists():
        return False, f"Build ({target}) produced no artifact: {output}"
    return True, ""


def compile_rust_artifact(
    rust_source: str,
    output_binary: str,
    timeout: float,
    verbose: bool = False,
) -> tuple[bool, str]:
    """Compile a generated Rust artifact with rustc."""
    rustc = _find_rustc()
    if rustc is None:
        return False, "rustc not found (required for --target rust validation)"
    cmd = [
        rustc,
        rust_source,
        "-o",
        output_binary,
        "--edition=2021",
    ]
    for lint in ("unused_mut", "unused_variables", "dead_code", "non_snake_case"):
        cmd.extend(["-A", lint])
    if verbose:
        print(f"  rustc: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return False, f"rustc compilation timed out after {timeout}s"

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        return False, f"rustc compilation failed (exit {result.returncode}): {detail[:500]}"

    if not Path(output_binary).exists():
        return False, f"rustc produced no binary: {output_binary}"
    return True, ""


def run_molt(
    binary: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str, str, int]:
    """Run a Molt-compiled binary and return (stdout, stderr, exit_code)."""
    cmd = [binary]
    if verbose:
        print(f"  Molt run: {binary}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return "", f"Molt binary timed out after {timeout}s", -1
    return result.stdout, result.stderr, result.returncode


def run_luau(
    luau_source: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str, str, int]:
    """Run a Luau artifact via Lune and return (stdout, stderr, exit_code)."""
    lune = _find_lune()
    if lune is None:
        return "", "lune not found (required for --target luau validation)", -2

    cmd = [lune, "run", luau_source]
    if verbose:
        print(f"  Lune run: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return "", f"Luau artifact timed out after {timeout}s", -1
    return result.stdout, result.stderr, result.returncode


def _unified_diff(label_a: str, label_b: str, text_a: str, text_b: str) -> str:
    """Return a unified diff between two strings, or empty if identical."""
    lines_a = text_a.splitlines(keepends=True)
    lines_b = text_b.splitlines(keepends=True)
    diff = difflib.unified_diff(lines_a, lines_b, fromfile=label_a, tofile=label_b)
    return "".join(diff)


class ValidationResult:
    """Result of validating a single file."""

    __slots__ = ("source", "target", "status", "detail", "elapsed", "mismatches", "timings")

    PASS = "pass"
    FAIL = "fail"
    ERROR = "error"
    SKIP = "skip"

    def __init__(
        self,
        source: str,
        target: str,
        status: str,
        detail: str = "",
        elapsed: float = 0.0,
        mismatches: list[str] | None = None,
        timings: dict[str, float] | None = None,
    ):
        self.source = source
        self.target = target
        self.status = status
        self.detail = detail
        self.elapsed = elapsed
        self.mismatches = list(mismatches or [])
        self.timings = dict(timings or {})

    def to_json(self) -> dict[str, object]:
        """Return a normalized JSON object for machine-readable reporting."""
        return {
            "source": self.source,
            "target": self.target,
            "status": self.status,
            "mismatches": self.mismatches,
            "timings": self.timings,
            "detail": self.detail,
        }


def validate_file(
    source: str,
    target: str,
    profile: str,
    timeout: float,
    verbose: bool = False,
) -> ValidationResult:
    """Validate a single Python file: compile with Molt, run both, compare.

    Returns a ValidationResult with status pass/fail/error.
    """
    t0 = time.monotonic()
    try:
        env = _make_env()
    except RuntimeError as exc:
        return ValidationResult(
            source,
            target,
            ValidationResult.ERROR,
            str(exc),
            0.0,
            timings={"total_sec": 0.0},
        )

    timings: dict[str, float] = {}

    # Step 1: Run under CPython
    step_t0 = time.monotonic()
    cpython_stdout, cpython_stderr, cpython_rc = run_cpython(
        source, timeout, env, verbose
    )
    timings["cpython_sec"] = time.monotonic() - step_t0
    if cpython_rc == -1:
        elapsed = time.monotonic() - t0
        timings["total_sec"] = elapsed
        return ValidationResult(
            source,
            target,
            ValidationResult.ERROR,
            f"CPython timed out after {timeout}s",
            elapsed,
            timings=timings,
        )

    # Step 2: Build with Molt
    run_label = "molt"
    if target == TARGET_NATIVE:
        tmp_parent = _temp_parent_from_env(env)
        with tempfile.TemporaryDirectory(
            prefix="molt-translation-native-",
            dir=tmp_parent,
        ) as tmpdir:
            binary_name = f"{Path(source).stem}_native"
            if os.name == "nt":
                binary_name += ".exe"
            run_path = str(Path(tmpdir) / binary_name)

            step_t0 = time.monotonic()
            ok, build_error = build_molt_native(
                source,
                profile,
                run_path,
                timeout,
                env,
                verbose,
            )
            timings["build_sec"] = time.monotonic() - step_t0
            if not ok:
                elapsed = time.monotonic() - t0
                timings["total_sec"] = elapsed
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.ERROR,
                    f"Build error: {build_error}",
                    elapsed,
                    timings=timings,
                )

            step_t0 = time.monotonic()
            molt_stdout, molt_stderr, molt_rc = run_molt(run_path, timeout, env, verbose)
            timings["run_sec"] = time.monotonic() - step_t0
            if molt_rc == -1 and "timed out" in molt_stderr:
                elapsed = time.monotonic() - t0
                timings["total_sec"] = elapsed
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.ERROR,
                    f"Molt binary timed out after {timeout}s",
                    elapsed,
                    timings=timings,
                )

            step_t0 = time.monotonic()
            mismatches: list[str] = []
            if cpython_stdout != molt_stdout:
                diff = _unified_diff(
                    "cpython/stdout", "molt/stdout", cpython_stdout, molt_stdout
                )
                mismatches.append(f"STDOUT MISMATCH:\n{diff}")
            if cpython_stderr != molt_stderr:
                diff = _unified_diff(
                    "cpython/stderr", "molt/stderr", cpython_stderr, molt_stderr
                )
                mismatches.append(f"STDERR MISMATCH:\n{diff}")
            if cpython_rc != molt_rc:
                mismatches.append(
                    f"EXIT CODE MISMATCH: cpython={cpython_rc}, molt={molt_rc}"
                )
            timings["compare_sec"] = time.monotonic() - step_t0
            elapsed = time.monotonic() - t0
            timings["total_sec"] = elapsed
            if mismatches:
                detail = "\n".join(mismatches)
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.FAIL,
                    detail,
                    elapsed,
                    mismatches=mismatches,
                    timings=timings,
                )
            return ValidationResult(
                source,
                target,
                ValidationResult.PASS,
                "",
                elapsed,
                mismatches=[],
                timings=timings,
            )
    else:
        suffix = ".rs" if target == TARGET_RUST else ".luau"
        with tempfile.TemporaryDirectory(
            prefix=f"molt-translation-{target}-",
            dir=_temp_parent_from_env(env),
        ) as tmpdir:
            artifact_path = str(Path(tmpdir) / f"{Path(source).stem}{suffix}")

            step_t0 = time.monotonic()
            ok, build_error = build_molt_target(
                source,
                target,
                profile,
                artifact_path,
                timeout,
                env,
                verbose,
            )
            timings["build_sec"] = time.monotonic() - step_t0
            if not ok:
                elapsed = time.monotonic() - t0
                timings["total_sec"] = elapsed
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.ERROR,
                    f"Build error: {build_error}",
                    elapsed,
                    timings=timings,
                )

            if target == TARGET_RUST:
                binary_name = f"{Path(source).stem}_rust"
                if os.name == "nt":
                    binary_name += ".exe"
                binary_path = str(Path(tmpdir) / binary_name)

                step_t0 = time.monotonic()
                ok, rustc_error = compile_rust_artifact(
                    artifact_path,
                    binary_path,
                    timeout,
                    verbose,
                )
                timings["rustc_sec"] = time.monotonic() - step_t0
                if not ok:
                    elapsed = time.monotonic() - t0
                    timings["total_sec"] = elapsed
                    return ValidationResult(
                        source,
                        target,
                        ValidationResult.ERROR,
                        f"rustc error: {rustc_error}",
                        elapsed,
                        timings=timings,
                    )

                run_label = "rust"
                run_path = binary_path
            else:
                run_label = "luau"
                run_path = artifact_path

            # Step 3: Run translated artifact
            step_t0 = time.monotonic()
            if target == TARGET_LUAU:
                molt_stdout, molt_stderr, molt_rc = run_luau(
                    run_path,
                    timeout,
                    env,
                    verbose,
                )
            else:
                molt_stdout, molt_stderr, molt_rc = run_molt(
                    run_path,
                    timeout,
                    env,
                    verbose,
                )
            timings["run_sec"] = time.monotonic() - step_t0

            if molt_rc == -1 and "timed out" in molt_stderr:
                elapsed = time.monotonic() - t0
                timings["total_sec"] = elapsed
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.ERROR,
                    f"{run_label} artifact timed out after {timeout}s",
                    elapsed,
                    timings=timings,
                )
            if molt_rc == -2:
                elapsed = time.monotonic() - t0
                timings["total_sec"] = elapsed
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.ERROR,
                    molt_stderr,
                    elapsed,
                    timings=timings,
                )

            # Step 4: Compare
            step_t0 = time.monotonic()
            mismatches: list[str] = []

            if cpython_stdout != molt_stdout:
                diff = _unified_diff(
                    "cpython/stdout",
                    f"{run_label}/stdout",
                    cpython_stdout,
                    molt_stdout,
                )
                mismatches.append(f"STDOUT MISMATCH:\n{diff}")

            if cpython_stderr != molt_stderr:
                diff = _unified_diff(
                    "cpython/stderr",
                    f"{run_label}/stderr",
                    cpython_stderr,
                    molt_stderr,
                )
                mismatches.append(f"STDERR MISMATCH:\n{diff}")

            if cpython_rc != molt_rc:
                mismatches.append(
                    f"EXIT CODE MISMATCH: cpython={cpython_rc}, {run_label}={molt_rc}"
                )

            timings["compare_sec"] = time.monotonic() - step_t0
            elapsed = time.monotonic() - t0
            timings["total_sec"] = elapsed

            if mismatches:
                detail = "\n".join(mismatches)
                return ValidationResult(
                    source,
                    target,
                    ValidationResult.FAIL,
                    detail,
                    elapsed,
                    mismatches=mismatches,
                    timings=timings,
                )

            return ValidationResult(
                source,
                target,
                ValidationResult.PASS,
                "",
                elapsed,
                mismatches=[],
                timings=timings,
            )

    return ValidationResult(
        source,
        target,
        ValidationResult.ERROR,
        f"Internal error: unsupported target lane {target}",
        0.0,
        timings={"total_sec": 0.0},
    )


def collect_sources(batch_dir: str) -> list[str]:
    """Collect all .py files in a directory, sorted for deterministic ordering."""
    root = Path(batch_dir)
    if not root.is_dir():
        return []
    sources = sorted(str(p) for p in root.rglob("*.py") if p.is_file())
    return sources


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "source",
        nargs="?",
        help="Python source file to validate (required unless --batch is used)",
    )
    parser.add_argument(
        "--batch",
        metavar="DIR",
        help="Validate all .py files in DIR (recursively)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=300.0,
        help="Timeout in seconds for each CPython/Molt run (default: 300)",
    )
    parser.add_argument(
        "--target",
        choices=TARGET_CHOICES,
        default=TARGET_NATIVE,
        help="Validation target lane: native|rust|luau|all (default: native)",
    )
    parser.add_argument(
        "--json-out",
        metavar="PATH",
        help="Write normalized per-file/per-target results to PATH as JSON",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print commands and intermediate details",
    )
    args = parser.parse_args()

    # Determine which files to validate
    if args.batch and args.source:
        print(
            "ERROR: Provide either a source file or --batch DIR, not both.",
            file=sys.stderr,
        )
        return 2
    if args.batch:
        batch_dir = args.batch
        if not Path(batch_dir).is_dir():
            print(
                f"ERROR: Batch directory not found: {batch_dir}",
                file=sys.stderr,
            )
            return 2
        sources = collect_sources(batch_dir)
        if not sources:
            print(
                f"ERROR: No .py files found in {batch_dir}",
                file=sys.stderr,
            )
            return 2
    elif args.source:
        if not Path(args.source).exists():
            print(
                f"ERROR: Source file not found: {args.source}",
                file=sys.stderr,
            )
            return 2
        sources = [args.source]
    else:
        print(
            "ERROR: Provide a source file or --batch DIR.",
            file=sys.stderr,
        )
        parser.print_usage(sys.stderr)
        return 2

    # Run validation
    targets = _expand_targets(args.target)
    results: list[ValidationResult] = []
    total = len(sources) * len(targets)

    print(
        "Translation validation: "
        f"{len(sources)} file(s), targets={','.join(targets)}, "
        f"profile={args.build_profile}, timeout={args.timeout}s"
    )
    print()

    counter = 0
    for source in sources:
        for target in targets:
            counter += 1
            label = f"{Path(source).name} [{target}]"
            if args.verbose:
                print(f"[{counter}/{total}] {source} [{target}]")
            else:
                print(f"[{counter}/{total}] {label} ... ", end="", flush=True)

            result = validate_file(
                source,
                target=target,
                profile=args.build_profile,
                timeout=args.timeout,
                verbose=args.verbose,
            )
            results.append(result)

            if not args.verbose:
                if result.status == ValidationResult.PASS:
                    print(f"PASS ({result.elapsed:.1f}s)")
                elif result.status == ValidationResult.FAIL:
                    print(f"FAIL ({result.elapsed:.1f}s)")
                elif result.status == ValidationResult.ERROR:
                    print(f"ERROR ({result.elapsed:.1f}s)")
                else:
                    print(f"SKIP ({result.elapsed:.1f}s)")

            # Show detail for failures and errors in both modes
            if result.status == ValidationResult.FAIL:
                for line in result.detail.splitlines():
                    print(f"    {line}")
            elif result.status == ValidationResult.ERROR and args.verbose:
                print(f"    {result.detail[:300]}")

    # Summary
    n_pass = sum(1 for r in results if r.status == ValidationResult.PASS)
    n_fail = sum(1 for r in results if r.status == ValidationResult.FAIL)
    n_error = sum(1 for r in results if r.status == ValidationResult.ERROR)
    n_skip = sum(1 for r in results if r.status == ValidationResult.SKIP)
    total_time = sum(r.elapsed for r in results)

    print()
    print(
        f"Results: {n_pass} pass, {n_fail} fail, {n_error} error, {n_skip} skip  ({total_time:.1f}s)"
    )

    if args.json_out:
        json_payload = {
            "sources": sources,
            "targets": targets,
            "build_profile": args.build_profile,
            "timeout_sec": args.timeout,
            "summary": {
                "pass": n_pass,
                "fail": n_fail,
                "error": n_error,
                "skip": n_skip,
                "total": len(results),
                "total_time_sec": total_time,
            },
            "results": [r.to_json() for r in results],
        }
        try:
            out_path = Path(args.json_out)
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(json.dumps(json_payload, indent=2), encoding="utf-8")
            if args.verbose:
                print(f"Wrote JSON report: {out_path}")
        except OSError as exc:
            print(f"ERROR: Unable to write --json-out file: {exc}", file=sys.stderr)
            return 2

    if n_fail > 0:
        print()
        print("Failed file/target pairs:")
        for r in results:
            if r.status == ValidationResult.FAIL:
                print(f"  {r.source} [{r.target}]")
        return 1

    if n_error > 0 and n_pass == 0:
        # All files errored (build infra issue) -- exit 2
        return 2

    return 0


if __name__ == "__main__":
    sys.exit(main())
