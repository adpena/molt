"""Run Monty conformance suite through Molt compilation.

Unlike run_monty_conformance.py (which runs against CPython), this runner
compiles each .py file via `molt build` and runs the resulting binary.

Usage:
    python3 tests/harness/run_molt_conformance.py [--limit N] [--category PREFIX] [--verbose]
"""

from __future__ import annotations

import argparse
import os
import shlex
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
REPO_ROOT = _HARNESS_DIR.parent.parent
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.harness_conformance import (  # noqa: E402
    build_molt_conformance_env,
    conformance_exit_code,
    ensure_molt_conformance_dirs,
    load_molt_conformance_suite,
    write_molt_conformance_summary,
)
from run_monty_conformance import parse_expectation  # noqa: E402

CORPUS_DIR = _HARNESS_DIR / "corpus" / "monty_compat"
SMOKE_MANIFEST = CORPUS_DIR / "SMOKE.txt"

COMPILE_TIMEOUT = 30  # seconds per file (after warmup)
WARMUP_TIMEOUT = 300  # seconds for the very first build (may trigger Rust recompile)
RUN_TIMEOUT = 5  # seconds per binary


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


def _normalize_molt_cmd(cmd: str | list[str]) -> list[str]:
    if isinstance(cmd, list):
        return cmd
    return shlex.split(cmd)


def find_molt() -> list[str] | None:
    """Return the command used to invoke the Molt CLI, or None."""
    if os.environ.get("MOLT_BIN"):
        return _normalize_molt_cmd(os.environ["MOLT_BIN"])
    if (SRC_ROOT / "molt" / "cli.py").exists():
        return [sys.executable, "-m", "molt.cli"]
    for candidate in ("molt", "/opt/homebrew/bin/molt", "/usr/local/bin/molt"):
        found = shutil.which(candidate)
        if found:
            return [found]
    return None


def _molt_build_env(repo_root: Path = REPO_ROOT) -> dict[str, str]:
    """Return canonical build env defaults for conformance runs."""
    env = os.environ.copy()
    session_id = env.get("MOLT_SESSION_ID") or "monty-conformance"
    env.update(build_molt_conformance_env(repo_root, session_id))
    return env


def _ensure_build_dirs(env: dict[str, str]) -> None:
    ensure_molt_conformance_dirs(env)


def _exit_code_for_stats(stats: Stats) -> int:
    return conformance_exit_code(
        {
            "failed": stats.failed,
            "compile_error": stats.compile_error,
            "timeout": stats.timeout,
        }
    )


def _selected_test_files(
    *,
    suite: str,
    category: str,
    limit: int,
    corpus_dir: Path = CORPUS_DIR,
    smoke_manifest: Path = SMOKE_MANIFEST,
) -> list[Path]:
    test_files = load_molt_conformance_suite(corpus_dir, suite, smoke_manifest)
    if category:
        test_files = [f for f in test_files if f.name.startswith(category)]
    if limit > 0:
        test_files = test_files[:limit]
    return test_files


def _stats_to_summary(
    stats: Stats,
    *,
    suite: str,
    manifest_path: Path | None,
    corpus_root: Path,
    duration_s: float,
) -> dict[str, object]:
    return {
        "suite": suite,
        "manifest_path": str(manifest_path) if manifest_path is not None else None,
        "corpus_root": str(corpus_root),
        "duration_s": duration_s,
        "total": (
            stats.passed
            + stats.failed
            + stats.compile_error
            + stats.timeout
            + stats.skipped
        ),
        "passed": stats.passed,
        "failed": stats.failed,
        "compile_error": stats.compile_error,
        "timeout": stats.timeout,
        "skipped": stats.skipped,
        "failures": [
            {"path": path, "detail": detail} for path, detail in stats.failures
        ],
        "compile_errors": [
            {"path": path, "detail": detail} for path, detail in stats.compile_errors
        ],
        "timeouts": list(stats.timeouts),
    }


def _pick_preflight_files(test_files: list[Path], n: int = 5) -> list[Path]:
    """Choose files with 'success' expectations for the preflight check.

    We specifically avoid files that are *expected* to fail at compile
    time (e.g. args__* which test CPython error paths), because Molt
    legitimately rejects those at compile time.
    """
    success_files: list[Path] = []
    for f in test_files:
        kind, _ = parse_expectation(f)
        if kind in ("success", "refcount"):
            success_files.append(f)
            if len(success_files) >= n:
                break
    return success_files


def preflight(molt_cmd: list[str], test_files: list[Path], tmpdir: Path) -> bool:
    """Compile a handful of trivial files to verify Molt works.

    The very first compilation may trigger a full Rust recompile of the
    runtime library, so we use a generous timeout for the warmup build.
    """
    candidates = _pick_preflight_files(test_files)
    if not candidates:
        print(
            "ERROR: no success-expectation files found for preflight", file=sys.stderr
        )
        return False

    ok = 0
    env = _molt_build_env()
    _ensure_build_dirs(env)
    for i, f in enumerate(candidates):
        timeout = WARMUP_TIMEOUT if i == 0 else COMPILE_TIMEOUT
        out = tmpdir / f"preflight_{f.stem}"
        try:
            t0 = time.monotonic()
            r = subprocess.run(
                [*molt_cmd, "build", str(f), "--output", str(out)],
                capture_output=True,
                text=True,
                timeout=timeout,
                env=env,
            )
            elapsed = time.monotonic() - t0
            if r.returncode == 0 and out.exists():
                ok += 1
                print(
                    f"  preflight [{i + 1}/{len(candidates)}] "
                    f"{f.name}: OK ({elapsed:.1f}s)"
                )
            else:
                detail = (r.stderr or r.stdout or "").strip()[-200:]
                print(
                    f"  preflight [{i + 1}/{len(candidates)}] "
                    f"{f.name}: FAIL ({elapsed:.1f}s) {detail}"
                )
        except subprocess.TimeoutExpired:
            print(
                f"  preflight [{i + 1}/{len(candidates)}] "
                f"{f.name}: TIMEOUT ({timeout}s)"
            )
        finally:
            out.unlink(missing_ok=True)

    print(f"Preflight: {ok}/{len(candidates)} compiled successfully")
    return ok > 0


def compile_file(molt_cmd: list[str], src: Path, out: Path) -> tuple[bool, str]:
    """Compile *src* to a native binary at *out*. Returns (success, detail)."""
    env = _molt_build_env()
    _ensure_build_dirs(env)
    try:
        r = subprocess.run(
            [*molt_cmd, "build", str(src), "--output", str(out)],
            capture_output=True,
            text=True,
            timeout=COMPILE_TIMEOUT,
            env=env,
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
            capture_output=True,
            text=True,
            timeout=RUN_TIMEOUT,
        )
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return None, "", "run timeout"


def check_result(
    filepath: Path,
    rc: int | None,
    stdout: str,
    stderr: str,
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suite",
        choices=("smoke", "full"),
        default="full",
        help="Which committed conformance suite to run.",
    )
    parser.add_argument(
        "--limit", type=int, default=0, help="Only test the first N files (0 = all)"
    )
    parser.add_argument(
        "--category",
        type=str,
        default="",
        help="Only test files whose name starts with PREFIX",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Write the canonical JSON summary artifact to this path.",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args(argv)

    molt = find_molt()
    if molt is None:
        print(
            "ERROR: molt CLI not found. Install Molt or set MOLT_BIN.", file=sys.stderr
        )
        return 1
    molt_cmd = _normalize_molt_cmd(molt)
    print(f"Using Molt at: {shlex.join(molt_cmd)}")

    if not CORPUS_DIR.exists():
        print(f"ERROR: corpus not found: {CORPUS_DIR}", file=sys.stderr)
        return 1

    # Collect test files
    test_files = _selected_test_files(
        suite=args.suite,
        category=args.category,
        limit=args.limit,
        corpus_dir=CORPUS_DIR,
        smoke_manifest=SMOKE_MANIFEST,
    )

    if not test_files:
        print("No test files match the selection criteria.", file=sys.stderr)
        return 1

    print(f"Selected {len(test_files)} test files\n")

    tmp_root = Path(
        build_molt_conformance_env(REPO_ROOT, "monty-conformance")["TMPDIR"]
    )
    tmp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="molt_conform_", dir=tmp_root) as tmpdir:
        tmp = Path(tmpdir)

        # Preflight -- also warms up the runtime build cache
        print(
            "Running preflight (first build may take minutes if runtime "
            "needs recompilation)..."
        )
        if not preflight(molt_cmd, test_files, tmp):
            print(
                "ERROR: preflight failed -- Molt cannot compile any files.",
                file=sys.stderr,
            )
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
            ok, detail = compile_file(molt_cmd, filepath, binary)

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

    print(f"\n{'=' * 60}")
    print(f"Molt conformance results  ({elapsed:.1f}s)")
    print(f"{'=' * 60}")
    print(f"  Passed:        {stats.passed:4d}")
    print(f"  Failed:        {stats.failed:4d}")
    print(f"  Compile error: {stats.compile_error:4d}")
    print(f"  Timeout:       {stats.timeout:4d}")
    print(f"  Skipped:       {stats.skipped:4d}")
    if total_run > 0:
        print(
            f"  Pass rate:     {pct:.0f}% "
            f"({stats.passed}/{total_run} of those that compiled & ran)"
        )

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

    summary = _stats_to_summary(
        stats,
        suite=args.suite,
        manifest_path=SMOKE_MANIFEST if args.suite == "smoke" else None,
        corpus_root=CORPUS_DIR,
        duration_s=elapsed,
    )
    if args.json_out is not None:
        write_molt_conformance_summary(args.json_out, summary)

    return _exit_code_for_stats(stats)


if __name__ == "__main__":
    sys.exit(main())
