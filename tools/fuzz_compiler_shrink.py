from __future__ import annotations

import ast
import subprocess
import sys
import time
from pathlib import Path

from tools.fuzz_compiler_types import FuzzResult


def _harness_memory_guard():
    from tools import harness_memory_guard

    return harness_memory_guard


# ---------------------------------------------------------------------------
# Shrinking
# ---------------------------------------------------------------------------


def _validate_syntax(source: str) -> bool:
    """Return True if *source* parses as valid Python."""
    try:
        ast.parse(source)
        return True
    except SyntaxError:
        return False


def _shrink_program(
    result: FuzzResult,
    profile: str,
    timeout: float,
    env: dict[str, str],
    tmpdir: str,
) -> FuzzResult:
    """Try to minimize a failing program by removing top-level blocks.

    Returns a new FuzzResult with the smallest source that still reproduces
    the same failure category (mismatch, build_error, or molt_run_error).
    """
    source = result.source
    target_status = result.status
    best = source

    def _still_fails(candidate: str) -> bool:
        """Check if *candidate* still triggers the same failure category."""
        if not _validate_syntax(candidate):
            return False
        src_path = Path(tmpdir) / f"shrink_{result.seed}.py"
        src_path.write_text(candidate)
        probe_result = _fuzz_one_program(
            source=candidate,
            seed=result.seed,
            program_id=result.program_id,
            profile=profile,
            timeout=timeout,
            env=env,
            tmpdir=tmpdir,
        )
        return probe_result.status == target_status

    # Strategy 1: remove top-level statement blocks one at a time.
    improved = True
    while improved:
        improved = False
        lines = best.splitlines()
        # Identify top-level block start indices (non-empty, non-indented lines
        # that are not the final print).
        block_starts: list[int] = []
        for i, line in enumerate(lines):
            if not line.strip():
                continue
            # Top-level: first character is not whitespace.
            if line[0:1] in ("", " ", "\t"):
                continue
            block_starts.append(i)

        # Try removing each block (from end to start to preserve indices).
        for idx in reversed(block_starts):
            # Find the extent of this block (include indented continuation lines).
            end = idx + 1
            while end < len(lines):
                next_line = lines[end]
                if next_line.strip() == "":
                    end += 1
                    continue
                if next_line[0] in (" ", "\t"):
                    end += 1
                    continue
                break
            candidate_lines = lines[:idx] + lines[end:]
            candidate = "\n".join(candidate_lines) + "\n"
            if not candidate.strip():
                continue
            if _still_fails(candidate):
                best = candidate
                improved = True
                break  # Restart the outer loop with the smaller program.

    # Strategy 2: try simplifying individual lines by replacing complex
    # expressions with simple literals.  (Lightweight — just try removing
    # print args beyond the first.)
    # This is intentionally conservative to avoid long shrink times.

    shrunk = FuzzResult(
        program_id=result.program_id,
        seed=result.seed,
        source=best,
        status=result.status,
        cpython_stdout=result.cpython_stdout,
        cpython_stderr=result.cpython_stderr,
        cpython_rc=result.cpython_rc,
        molt_stdout=result.molt_stdout,
        molt_stderr=result.molt_stderr,
        molt_rc=result.molt_rc,
        elapsed_sec=result.elapsed_sec,
        error_detail=result.error_detail,
    )
    return shrunk


def _fuzz_one_program(
    source: str,
    seed: int,
    program_id: int,
    profile: str,
    timeout: float,
    env: dict[str, str],
    tmpdir: str,
) -> FuzzResult:
    """Run one source through CPython + Molt, return a FuzzResult.

    Factored out of fuzz_one_safe for reuse by the shrinking logic.
    """
    src_path = Path(tmpdir) / f"fuzz_{seed}.py"
    src_path.write_text(source)
    elapsed_start = time.monotonic()

    # CPython baseline
    guard = _harness_memory_guard()
    limits = guard.limits_from_env("MOLT_TEST_SUITE", env)
    try:
        cp_result = guard.guarded_completed_process(
            [sys.executable, str(src_path)],
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            limits=limits,
        )
        cp_out = cp_result.stdout
        cp_err = cp_result.stderr
        cp_rc = cp_result.returncode
    except subprocess.TimeoutExpired:
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="timeout",
            cpython_stdout="",
            cpython_stderr="TIMEOUT",
            cpython_rc=-1,
            molt_stdout="",
            molt_stderr="",
            molt_rc=-1,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail="cpython timeout",
        )

    if cp_rc != 0:
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="cpython_error",
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout="",
            molt_stderr="",
            molt_rc=-1,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail=cp_err[:200],
        )

    # Molt build
    repo_root = Path(__file__).resolve().parents[1]
    binary = Path(tmpdir) / f"fuzz_{seed}_molt"
    build_cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "-o",
        str(binary),
        str(src_path),
    ]
    build_env = dict(env)
    build_env.setdefault("PYTHONPATH", str(repo_root / "src"))
    build_limits = guard.limits_from_env("MOLT_TEST_SUITE", build_env)
    try:
        build_result = guard.guarded_completed_process(
            build_cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout * 2,
            env=build_env,
            cwd=str(repo_root),
            limits=build_limits,
        )
    except subprocess.TimeoutExpired:
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="timeout",
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout="",
            molt_stderr="BUILD TIMEOUT",
            molt_rc=-1,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail="molt build timeout",
        )

    if build_result.returncode != 0:
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="build_error",
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout=build_result.stdout,
            molt_stderr=build_result.stderr,
            molt_rc=build_result.returncode,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail=f"Molt build failed (rc={build_result.returncode}):\nstderr: {build_result.stderr[:300]}",
        )

    # Molt run
    if not binary.exists():
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="build_error",
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout="",
            molt_stderr="binary not found",
            molt_rc=-1,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail="binary not found after build",
        )

    try:
        run_result = guard.guarded_completed_process(
            [str(binary)],
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="timeout",
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout="",
            molt_stderr="RUN TIMEOUT",
            molt_rc=-1,
            elapsed_sec=time.monotonic() - elapsed_start,
            error_detail="molt run timeout",
        )

    elapsed = time.monotonic() - elapsed_start

    # Compare (direct string comparison, same as fuzz_one_safe)
    if cp_out != run_result.stdout or cp_rc != run_result.returncode:
        status = "mismatch" if run_result.returncode == 0 else "molt_run_error"
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status=status,
            cpython_stdout=cp_out,
            cpython_stderr=cp_err,
            cpython_rc=cp_rc,
            molt_stdout=run_result.stdout,
            molt_stderr=run_result.stderr,
            molt_rc=run_result.returncode,
            elapsed_sec=elapsed,
            error_detail=f"stdout differs (cp_rc={cp_rc}, molt_rc={run_result.returncode})",
        )

    return FuzzResult(
        program_id=program_id,
        seed=seed,
        source=source,
        status="pass",
        cpython_stdout=cp_out,
        cpython_stderr=cp_err,
        cpython_rc=cp_rc,
        molt_stdout=run_result.stdout,
        molt_stderr=run_result.stderr,
        molt_rc=run_result.returncode,
        elapsed_sec=elapsed,
        error_detail="",
    )
