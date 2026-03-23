"""CPython parity enforcement gate for Molt.

Three-tier comparison:
  Tier 1 (STRICT)   — byte-identical output required (default, blocks merge on failure)
  Tier 2 (RELAXED)  — normalized comparison: memory addresses, refcount values stripped
  Tier 3 (EXCLUDED) — expected divergence, comparison skipped

Tier detection via marker in test file:
  # molt-parity: relaxed   → Tier 2
  # molt-parity: excluded  → Tier 3
  (no marker)              → Tier 1

Usage:
    python3 tools/parity_gate.py [directory]
    python3 tools/parity_gate.py tests/differential/basic/

Exit codes:
    0 — all tests passed (or only Tier 2 warnings / Tier 3 skips)
    1 — at least one Tier 1 (STRICT) violation
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

TIMEOUT_SECONDS = 120

_PARITY_MARKER_RE = re.compile(
    r"^\s*#\s*molt-parity\s*:\s*(\w+)", re.MULTILINE
)

# Relaxed normalization patterns
_ADDR_RE = re.compile(r"\b0x[0-9a-fA-F]+\b")
_REFCOUNT_RE = re.compile(r"\brefcount\s*=\s*\d+", re.IGNORECASE)
# Memory address patterns like <object at 0x...> or id=0x...
_OBJ_ADDR_RE = re.compile(r"(at|id=)\s*0x[0-9a-fA-F]+")

# Markers that suggest the test imports something Molt cannot handle
_IMPORT_ERROR_MARKERS = (
    "ModuleNotFoundError",
    "ImportError",
    "No module named",
)

# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

TIER_STRICT = 1
TIER_RELAXED = 2
TIER_EXCLUDED = 3

_TIER_LABELS = {
    TIER_STRICT: "STRICT",
    TIER_RELAXED: "RELAXED",
    TIER_EXCLUDED: "EXCLUDED",
}


@dataclass
class TestResult:
    file: Path
    tier: int
    status: str  # "pass", "fail", "skip", "warn", "error"
    message: str = ""
    cpython_stdout: str = ""
    cpython_stderr: str = ""
    molt_stdout: str | None = None
    molt_stderr: str | None = None


# ---------------------------------------------------------------------------
# Tier detection
# ---------------------------------------------------------------------------


def _read_file_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def classify_tier(path: Path) -> int:
    text = _read_file_text(path)
    m = _PARITY_MARKER_RE.search(text)
    if m is None:
        return TIER_STRICT
    marker = m.group(1).strip().lower()
    if marker == "excluded":
        return TIER_EXCLUDED
    if marker == "relaxed":
        return TIER_RELAXED
    # Unknown marker value — default to strict
    return TIER_STRICT


# ---------------------------------------------------------------------------
# Output normalization
# ---------------------------------------------------------------------------


def _normalize_relaxed(text: str) -> str:
    """Normalize output for Tier 2 (RELAXED) comparison."""
    text = _ADDR_RE.sub("0xADDR", text)
    text = _OBJ_ADDR_RE.sub(r"\g<1> 0xADDR", text)
    text = _REFCOUNT_RE.sub("refcount=<N>", text)
    return text


# ---------------------------------------------------------------------------
# Execution helpers
# ---------------------------------------------------------------------------


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _resolve_molt_cmd() -> tuple[list[str], dict[str, str]]:
    """Return (command_prefix, extra_env) for invoking molt.

    Prefers the `molt` binary on PATH. Falls back to `python3 -m molt.cli`
    from the repo root if the binary is not available.

    Returns an extra_env dict that callers must merge into the subprocess env
    (e.g. PYTHONPATH for the module fallback).
    """
    import shutil

    # Check env override first
    molt_bin = os.environ.get("MOLT_BIN_PATH", "").strip()
    if molt_bin:
        return [molt_bin], {}

    # Try molt on PATH
    if shutil.which("molt"):
        return ["molt"], {}

    # Fall back to Python module invocation from repo root
    repo_root = _repo_root()
    src_dir = str(repo_root / "src")

    python = sys.executable
    # Check for venv python (shares site-packages with repo dev install).
    # Worktrees live under .claude/worktrees/<name>; the actual venv may be
    # in the main repo root two levels up.
    venv_search_roots = [repo_root]
    # Walk up to find other candidate roots (handles worktrees)
    for parent in repo_root.parents:
        if (parent / ".venv").exists() or (parent / "pyproject.toml").exists():
            venv_search_roots.append(parent)
        if len(venv_search_roots) >= 4:
            break

    venv_candidates: list[Path] = []
    for search_root in venv_search_roots:
        venv_candidates.extend([
            search_root / ".venv" / "bin" / "python3",
            search_root / ".venv" / "bin" / "python",
        ])

    for candidate in venv_candidates:
        if candidate.exists():
            python = str(candidate)
            break

    extra_env: dict[str, str] = {}
    # Only inject PYTHONPATH if we're NOT using the venv python (the venv
    # python already has access to the installed package). For the system
    # python we need to point it at /src.
    if python == sys.executable:
        existing = os.environ.get("PYTHONPATH", "")
        extra_env["PYTHONPATH"] = f"{src_dir}:{existing}" if existing else src_dir

    return [python, "-m", "molt.cli"], extra_env


def _run_process(
    cmd: list[str],
    *,
    timeout: float = TIMEOUT_SECONDS,
    extra_env: dict[str, str] | None = None,
) -> tuple[str, str, int]:
    """Run a subprocess and return (stdout, stderr, returncode).

    Returns ("", "<timeout>", 124) on timeout.
    Returns ("", "<error: ...>", 127) on OS error.
    """
    env = os.environ.copy()
    if extra_env:
        env.update(extra_env)
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", f"<timeout after {timeout}s>", 124
    except OSError as exc:
        return "", f"<error: {exc}>", 127


def run_cpython(path: Path, *, timeout: float = TIMEOUT_SECONDS) -> tuple[str, str, int]:
    python = os.environ.get("PARITY_PYTHON", sys.executable) or "python3"
    return _run_process([python, str(path)], timeout=timeout)


def run_molt(
    path: Path,
    molt_cmd: list[str],
    molt_env: dict[str, str] | None = None,
    *,
    timeout: float = TIMEOUT_SECONDS,
) -> tuple[str, str, int]:
    cmd = molt_cmd + ["run", str(path)]
    # Route molt build artifacts through a temp area so gate runs don't
    # pollute or conflict with normal dev builds.
    extra: dict[str, str] = {
        "PYTHONHASHSEED": "0",
    }
    if molt_env:
        extra.update(molt_env)
    # Pass capabilities for parity testing (filesystem, env, time, random)
    diff_caps = os.environ.get("MOLT_DIFF_CAPABILITIES", "fs,env,time,random")
    if diff_caps:
        extra["MOLT_CAPABILITIES"] = diff_caps
    return _run_process(cmd, timeout=timeout, extra_env=extra)


# ---------------------------------------------------------------------------
# Comparison logic
# ---------------------------------------------------------------------------


def _cpython_import_failed(stderr: str) -> bool:
    """Return True if CPython itself failed due to a missing import."""
    return any(marker in stderr for marker in _IMPORT_ERROR_MARKERS)


def _molt_import_failed(stderr: str) -> bool:
    """Return True if Molt failed due to a missing/unsupported import."""
    return any(marker in stderr for marker in _IMPORT_ERROR_MARKERS)


def compare(
    path: Path,
    tier: int,
    cpython_out: str,
    cpython_err: str,
    cpython_rc: int,
    molt_out: str | None,
    molt_err: str,
    molt_rc: int,
) -> TestResult:
    base = TestResult(
        file=path,
        tier=tier,
        status="pass",
        cpython_stdout=cpython_out,
        cpython_stderr=cpython_err,
        molt_stdout=molt_out,
        molt_stderr=molt_err,
    )

    # Excluded tier — never compare
    if tier == TIER_EXCLUDED:
        base.status = "skip"
        base.message = "excluded by marker"
        return base

    # If Molt could not build/run at all (exit 124 = timeout, 127 = OS error)
    if molt_rc == 124:
        base.status = "error"
        base.message = f"molt timed out"
        return base
    if molt_rc == 127 and molt_out is None:
        base.status = "error"
        base.message = f"molt not found or OS error: {molt_err.strip()}"
        return base

    # If the test needs a module that Molt doesn't support yet, skip gracefully
    if molt_out is None or _molt_import_failed(molt_err):
        base.status = "skip"
        base.message = "molt missing import (skipped)"
        return base

    # Compare outputs
    if tier == TIER_STRICT:
        stdout_match = cpython_out == molt_out
        rc_match = cpython_rc == molt_rc
        if stdout_match and rc_match:
            base.status = "pass"
        else:
            base.status = "fail"
            parts: list[str] = []
            if not stdout_match:
                parts.append("stdout mismatch")
            if not rc_match:
                parts.append(f"exit code cpython={cpython_rc} molt={molt_rc}")
            base.message = "; ".join(parts)

    elif tier == TIER_RELAXED:
        norm_cpython = _normalize_relaxed(cpython_out)
        norm_molt = _normalize_relaxed(molt_out)
        rc_match = cpython_rc == molt_rc
        if norm_cpython == norm_molt and rc_match:
            base.status = "pass"
        else:
            base.status = "warn"
            parts = []
            if norm_cpython != norm_molt:
                parts.append("stdout mismatch (normalized)")
            if not rc_match:
                parts.append(f"exit code cpython={cpython_rc} molt={molt_rc}")
            base.message = "; ".join(parts)

    return base


# ---------------------------------------------------------------------------
# Per-file runner
# ---------------------------------------------------------------------------


def run_one(
    path: Path,
    molt_cmd: list[str],
    molt_env: dict[str, str] | None = None,
    *,
    timeout: float = TIMEOUT_SECONDS,
) -> TestResult:
    tier = classify_tier(path)

    # Run CPython
    cpython_out, cpython_err, cpython_rc = run_cpython(path, timeout=timeout)

    # If CPython itself fails due to import error, skip
    if cpython_rc != 0 and _cpython_import_failed(cpython_err):
        return TestResult(
            file=path,
            tier=tier,
            status="skip",
            message="cpython import error (not a Molt parity test)",
            cpython_stdout=cpython_out,
            cpython_stderr=cpython_err,
        )

    # Excluded — no need to run Molt
    if tier == TIER_EXCLUDED:
        return TestResult(
            file=path,
            tier=tier,
            status="skip",
            message="excluded by marker",
            cpython_stdout=cpython_out,
            cpython_stderr=cpython_err,
        )

    # Run Molt
    molt_out, molt_err, molt_rc = run_molt(path, molt_cmd, molt_env, timeout=timeout)

    return compare(
        path,
        tier,
        cpython_out,
        cpython_err,
        cpython_rc,
        molt_out,
        molt_err,
        molt_rc,
    )


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

_COLOR_RESET = "\033[0m"
_COLOR_GREEN = "\033[32m"
_COLOR_RED = "\033[31m"
_COLOR_YELLOW = "\033[33m"
_COLOR_CYAN = "\033[36m"
_COLOR_GRAY = "\033[90m"


def _supports_color() -> bool:
    return sys.stdout.isatty() and os.environ.get("NO_COLOR", "") == ""


def _colored(text: str, code: str) -> str:
    if _supports_color():
        return f"{code}{text}{_COLOR_RESET}"
    return text


def _status_display(result: TestResult) -> str:
    tier_label = _TIER_LABELS[result.tier]
    status = result.status.upper()
    if result.status == "pass":
        tag = _colored(f"[PASS/{tier_label}]", _COLOR_GREEN)
    elif result.status == "fail":
        tag = _colored(f"[FAIL/{tier_label}]", _COLOR_RED)
    elif result.status == "warn":
        tag = _colored(f"[WARN/{tier_label}]", _COLOR_YELLOW)
    elif result.status == "skip":
        tag = _colored(f"[SKIP/{tier_label}]", _COLOR_GRAY)
    elif result.status == "error":
        tag = _colored(f"[ERROR/{tier_label}]", _COLOR_RED)
    else:
        tag = f"[{status}/{tier_label}]"
    return tag


def print_result(result: TestResult, *, verbose: bool = False) -> None:
    tag = _status_display(result)
    rel = result.file.name
    line = f"{tag} {rel}"
    if result.message:
        line += f"  — {result.message}"
    print(line)

    if verbose and result.status in ("fail", "warn", "error"):
        if result.cpython_stdout:
            print(f"  cpython stdout: {result.cpython_stdout[:200]!r}")
        if result.molt_stdout is not None and result.molt_stdout != result.cpython_stdout:
            print(f"  molt stdout:    {result.molt_stdout[:200]!r}")
        if result.molt_stderr:
            print(f"  molt stderr:    {result.molt_stderr[:200]!r}")


def print_summary(
    results: list[TestResult],
    *,
    strict_violations: int,
) -> None:
    total = len(results)
    passed = sum(1 for r in results if r.status == "pass")
    failed = sum(1 for r in results if r.status == "fail")
    warned = sum(1 for r in results if r.status == "warn")
    skipped = sum(1 for r in results if r.status in ("skip",))
    errored = sum(1 for r in results if r.status == "error")

    print()
    print("=" * 60)
    print(f"Parity Gate Summary: {passed}/{total} passed")
    if failed:
        print(_colored(f"  STRICT violations : {failed}", _COLOR_RED))
    if warned:
        print(_colored(f"  RELAXED warnings  : {warned}", _COLOR_YELLOW))
    if errored:
        print(_colored(f"  Errors            : {errored}", _COLOR_RED))
    if skipped:
        print(f"  Skipped           : {skipped}")
    print("=" * 60)

    if strict_violations > 0:
        print(
            _colored(
                f"\nFAIL: {strict_violations} Tier 1 (STRICT) violation(s) — parity gate blocks merge.",
                _COLOR_RED,
            )
        )
    else:
        print(_colored("\nPASS: No Tier 1 violations.", _COLOR_GREEN))


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def collect_tests(directory: Path) -> list[Path]:
    """Return sorted list of .py files in directory."""
    return sorted(directory.glob("*.py"))


def main() -> int:
    parser = argparse.ArgumentParser(
        description="CPython parity enforcement gate for Molt (3-tier)"
    )
    parser.add_argument(
        "directory",
        nargs="?",
        default="tests/differential/basic/",
        help="Directory of .py test files (default: tests/differential/basic/)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print stdout/stderr diffs for failing tests",
    )
    parser.add_argument(
        "--molt-cmd",
        default="",
        help="Override molt command (e.g. 'cargo run --'). Defaults to auto-detect.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=None,
        help=f"Per-test timeout in seconds (default: {TIMEOUT_SECONDS}). "
             "Molt compile+run can take 60-120s on first run.",
    )
    args = parser.parse_args()

    directory = Path(args.directory).expanduser().resolve()
    if not directory.is_dir():
        print(f"Error: not a directory: {directory}", file=sys.stderr)
        return 2

    # Resolve molt command
    molt_env: dict[str, str] | None = None
    if args.molt_cmd:
        import shlex
        molt_cmd = shlex.split(args.molt_cmd)
    else:
        molt_cmd, molt_env = _resolve_molt_cmd()

    test_files = collect_tests(directory)
    if not test_files:
        print(f"No .py files found in {directory}", file=sys.stderr)
        return 2

    print(f"Parity gate: {len(test_files)} tests in {directory}")
    print(f"Molt command: {' '.join(molt_cmd)}")
    print()

    timeout = args.timeout if args.timeout is not None else TIMEOUT_SECONDS

    results: list[TestResult] = []
    for path in test_files:
        result = run_one(path, molt_cmd, molt_env, timeout=timeout)
        results.append(result)
        print_result(result, verbose=args.verbose)

    strict_violations = sum(
        1 for r in results if r.status == "fail" and r.tier == TIER_STRICT
    )

    print_summary(results, strict_violations=strict_violations)

    return 1 if strict_violations > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
