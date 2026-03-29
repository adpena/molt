"""Run Monty conformance suite through Molt compilation.

Unlike run_monty_conformance.py (which runs against CPython), this runner
compiles each .py file via `molt build` and runs the resulting binary.

Usage:
    python3 tests/harness/run_molt_conformance.py [--limit N] [--category PREFIX] [--verbose]
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

# Ensure the harness directory is on sys.path so the import works
# regardless of the caller's working directory.
_HARNESS_DIR = Path(__file__).resolve().parent
if str(_HARNESS_DIR) not in sys.path:
    sys.path.insert(0, str(_HARNESS_DIR))

from run_monty_conformance import parse_expectation  # noqa: E402

CORPUS_DIR = _HARNESS_DIR / "corpus" / "monty_compat"

COMPILE_TIMEOUT = 30   # seconds per file (after warmup)
WARMUP_TIMEOUT = 300   # seconds for the very first build (may trigger Rust recompile)
RUN_TIMEOUT = 5        # seconds per binary


# ---------------------------------------------------------------------------
# Result tracking
# ---------------------------------------------------------------------------

@dataclass
class Stats:
    passed: int = 0
    failed: int = 0
    compile_error: int = 0
    timeout: int = 0
    skipped: int = 0
    failures: list[tuple[str, str]] = field(default_factory=list)
    compile_errors: list[tuple[str, str]] = field(default_factory=list)
    timeouts: list[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def find_molt() -> str | None:
    """Return the path to the molt CLI, or None."""
    if os.environ.get("MOLT_BIN"):
        return os.environ["MOLT_BIN"]
    for candidate in ("molt", "/opt/homebrew/bin/molt", "/usr/local/bin/molt"):
        found = shutil.which(candidate)
        if found:
            return found
    return None


def _pick_preflight_files(corpus: Path, n: int = 5) -> list[Path]:
    """Choose files with 'success' expectations for the preflight check.

    We specifically avoid files that are *expected* to fail at compile
    time (e.g. args__* which test CPython error paths), because Molt
    legitimately rejects those at compile time.
    """
    success_files: list[Path] = []
    for f in sorted(corpus.glob("*.py")):
        kind, _ = parse_expectation(f)
        if kind in ("success", "refcount"):
            success_files.append(f)
            if len(success_files) >= n:
                break
    return success_files


def preflight(molt: str, corpus: Path, tmpdir: Path) -> bool:
    """Compile a handful of trivial files to verify Molt works.

    The very first compilation may trigger a full Rust recompile of the
    runtime library, so we use a generous timeout for the warmup build.
    """
    candidates = _pick_preflight_files(corpus)
    if not candidates:
        print("ERROR: no success-expectation files found for preflight",
              file=sys.stderr)
        return False

    ok = 0
    for i, f in enumerate(candidates):
        timeout = WARMUP_TIMEOUT if i == 0 else COMPILE_TIMEOUT
        out = tmpdir / f"preflight_{f.stem}"
        try:
            t0 = time.monotonic()
            r = subprocess.run(
                [molt, "build", str(f), "--output", str(out)],
                capture_output=True, text=True, timeout=timeout,
            )
            elapsed = time.monotonic() - t0
            if r.returncode == 0 and out.exists():
                ok += 1
                print(f"  preflight [{i+1}/{len(candidates)}] "
                      f"{f.name}: OK ({elapsed:.1f}s)")
            else:
                detail = (r.stderr or r.stdout or "").strip()[-200:]
                print(f"  preflight [{i+1}/{len(candidates)}] "
                      f"{f.name}: FAIL ({elapsed:.1f}s) {detail}")
        except subprocess.TimeoutExpired:
            print(f"  preflight [{i+1}/{len(candidates)}] "
                  f"{f.name}: TIMEOUT ({timeout}s)")
        finally:
            out.unlink(missing_ok=True)

    print(f"Preflight: {ok}/{len(candidates)} compiled successfully")
    return ok > 0


def compile_file(molt: str, src: Path, out: Path) -> tuple[bool, str]:
    """Compile *src* to a native binary at *out*. Returns (success, detail)."""
    try:
        r = subprocess.run(
            [molt, "build", str(src), "--output", str(out)],
            capture_output=True, text=True, timeout=COMPILE_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return False, "compile timeout"
    if r.returncode != 0:
        detail = (r.stderr or r.stdout or "").strip()[-300:]
        return False, f"compile failed (rc={r.returncode}): {detail}"
    if not out.exists():
        return False, "binary not produced"
    return True, ""


def run_binary(binary: Path) -> tuple[int | None, str, str]:
    """Run *binary* and return (returncode, stdout, stderr).

    returncode is None on timeout.
    """
    try:
        r = subprocess.run(
            [str(binary)],
            capture_output=True, text=True, timeout=RUN_TIMEOUT,
        )
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return None, "", "run timeout"


def check_result(
    filepath: Path, rc: int | None, stdout: str, stderr: str,
) -> tuple[bool | None, str]:
    """Compare actual output against the expectation for *filepath*."""
    kind, expected = parse_expectation(filepath)

    if kind == "skip":
        return None, expected

    if rc is None:
        return False, "run timeout (5s)"

    if kind == "raise":
        if rc == 0:
            return False, f"expected {expected}, but exited 0"
        if expected in stderr:
            return True, f"correctly raised {expected}"
        return False, f"expected {expected}, got stderr: {stderr.strip()[-200:]}"

    if kind in ("success", "refcount"):
        if rc == 0:
            return True, "exited 0"
        return False, f"expected exit 0, got rc={rc}: {stderr.strip()[:200]}"

    return False, f"unknown expectation kind: {kind}"


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--limit", type=int, default=0,
                        help="Only test the first N files (0 = all)")
    parser.add_argument("--category", type=str, default="",
                        help="Only test files whose name starts with PREFIX")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    molt = find_molt()
    if molt is None:
        print("ERROR: molt CLI not found. Install Molt or set MOLT_BIN.",
              file=sys.stderr)
        return 1
    print(f"Using Molt at: {molt}")

    if not CORPUS_DIR.exists():
        print(f"ERROR: corpus not found: {CORPUS_DIR}", file=sys.stderr)
        return 1

    # Collect test files
    test_files = sorted(CORPUS_DIR.glob("*.py"))
    if args.category:
        test_files = [f for f in test_files if f.name.startswith(args.category)]
    if args.limit > 0:
        test_files = test_files[:args.limit]

    if not test_files:
        print("No test files match the selection criteria.", file=sys.stderr)
        return 1

    print(f"Selected {len(test_files)} test files\n")

    with tempfile.TemporaryDirectory(prefix="molt_conform_") as tmpdir:
        tmp = Path(tmpdir)

        # Preflight -- also warms up the runtime build cache
        print("Running preflight (first build may take minutes if runtime "
              "needs recompilation)...")
        if not preflight(molt, CORPUS_DIR, tmp):
            print("ERROR: preflight failed -- Molt cannot compile any files.",
                  file=sys.stderr)
            return 1
        print()

        stats = Stats()
        t0 = time.monotonic()

        for i, filepath in enumerate(test_files, 1):
            kind, _ = parse_expectation(filepath)
            if kind == "skip":
                stats.skipped += 1
                if args.verbose:
                    print(f"  [{i:3d}] SKIP   {filepath.name}")
                continue

            binary = tmp / filepath.stem
            ok, detail = compile_file(molt, filepath, binary)

            if not ok:
                if "timeout" in detail:
                    stats.timeout += 1
                    stats.timeouts.append(filepath.name)
                    if args.verbose:
                        print(f"  [{i:3d}] TMOUT  {filepath.name}: {detail}")
                else:
                    stats.compile_error += 1
                    stats.compile_errors.append((filepath.name, detail))
                    if args.verbose:
                        print(f"  [{i:3d}] CERR   {filepath.name}: {detail}")
                continue

            rc, stdout, stderr = run_binary(binary)
            passed, detail = check_result(filepath, rc, stdout, stderr)

            if passed is None:
                stats.skipped += 1
                if args.verbose:
                    print(f"  [{i:3d}] SKIP   {filepath.name}: {detail}")
            elif passed:
                stats.passed += 1
                if args.verbose:
                    print(f"  [{i:3d}] PASS   {filepath.name}: {detail}")
            else:
                if rc is None:
                    stats.timeout += 1
                    stats.timeouts.append(filepath.name)
                    if args.verbose:
                        print(f"  [{i:3d}] TMOUT  {filepath.name}: {detail}")
                else:
                    stats.failed += 1
                    stats.failures.append((filepath.name, detail))
                    if args.verbose:
                        print(f"  [{i:3d}] FAIL   {filepath.name}: {detail}")

            # Clean up binary between runs to save disk space
            binary.unlink(missing_ok=True)

    elapsed = time.monotonic() - t0
    total_run = stats.passed + stats.failed
    pct = (stats.passed / total_run * 100) if total_run > 0 else 0

    print(f"\n{'='*60}")
    print(f"Molt conformance results  ({elapsed:.1f}s)")
    print(f"{'='*60}")
    print(f"  Passed:        {stats.passed:4d}")
    print(f"  Failed:        {stats.failed:4d}")
    print(f"  Compile error: {stats.compile_error:4d}")
    print(f"  Timeout:       {stats.timeout:4d}")
    print(f"  Skipped:       {stats.skipped:4d}")
    if total_run > 0:
        print(f"  Pass rate:     {pct:.0f}% "
              f"({stats.passed}/{total_run} of those that compiled & ran)")

    if stats.failures:
        print(f"\nFailed ({len(stats.failures)}):")
        for name, detail in stats.failures:
            print(f"  {name}: {detail}")

    if stats.compile_errors and args.verbose:
        print(f"\nCompile errors ({len(stats.compile_errors)}):")
        for name, detail in stats.compile_errors:
            print(f"  {name}: {detail}")

    if stats.timeouts:
        print(f"\nTimeouts ({len(stats.timeouts)}):")
        for name in stats.timeouts:
            print(f"  {name}")

    return 0 if stats.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
