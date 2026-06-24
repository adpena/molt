from __future__ import annotations

import os
import time
from pathlib import Path
from random import Random

from tools.fuzz_compiler_execution import compile_molt, run_cpython, run_molt_binary
from tools.fuzz_compiler_reject import RejectProgramGenerator
from tools.fuzz_compiler_reporting import _log
from tools.fuzz_compiler_safe import SafeProgramGenerator
from tools.fuzz_compiler_types import FuzzResult

# ---------------------------------------------------------------------------
# Fuzzer core: safe mode
# ---------------------------------------------------------------------------


def fuzz_one_safe(
    program_id: int,
    seed: int,
    rng: Random,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool,
    tmpdir: str,
) -> FuzzResult:
    t0 = time.monotonic()
    gen = SafeProgramGenerator(rng, max_depth=3, max_stmts=15)
    source = gen.generate()

    source_path = os.path.join(tmpdir, f"fuzz_{program_id:06d}.py")
    Path(source_path).write_text(source)

    try:
        cp_stdout, cp_stderr, cp_rc = run_cpython(source_path, timeout)

        if cp_rc is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                error_detail="CPython execution timed out",
                elapsed_sec=time.monotonic() - t0,
            )

        if cp_rc != 0:
            if verbose:
                _log(f"  [#{program_id}] CPython error (rc={cp_rc}), skipping")
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="cpython_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                error_detail=f"CPython exited with rc={cp_rc}",
                elapsed_sec=time.monotonic() - t0,
            )

        binary, build_error = compile_molt(source_path, profile, timeout, env)
        if binary is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="build_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                error_detail=build_error,
                elapsed_sec=time.monotonic() - t0,
            )

        molt_stdout, molt_stderr, molt_rc = run_molt_binary(binary, timeout, env)

        if molt_rc is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                error_detail="Molt binary execution timed out",
                elapsed_sec=time.monotonic() - t0,
            )

        if cp_stdout == molt_stdout:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="pass",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                elapsed_sec=time.monotonic() - t0,
            )

        if molt_rc != 0:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="molt_run_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                error_detail=f"Molt binary exited with rc={molt_rc}",
                elapsed_sec=time.monotonic() - t0,
            )

        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="mismatch",
            cpython_stdout=cp_stdout,
            cpython_stderr=cp_stderr,
            molt_stdout=molt_stdout,
            molt_stderr=molt_stderr,
            elapsed_sec=time.monotonic() - t0,
        )
    finally:
        try:
            Path(source_path).unlink(missing_ok=True)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Fuzzer core: reject mode
# ---------------------------------------------------------------------------


def fuzz_one_reject(
    program_id: int,
    seed: int,
    rng: Random,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool,
    tmpdir: str,
) -> FuzzResult:
    t0 = time.monotonic()
    gen = RejectProgramGenerator(rng)
    source, reason = gen.generate()

    source_path = os.path.join(tmpdir, f"fuzz_reject_{program_id:06d}.py")
    Path(source_path).write_text(source)

    try:
        binary, build_error = compile_molt(source_path, profile, timeout, env)

        if binary is not None:
            # Molt accepted a program it should have rejected
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="reject_fail",
                error_detail=f"Molt should have rejected ({reason}) but compiled successfully",
                elapsed_sec=time.monotonic() - t0,
            )

        # Non-zero exit is expected; check it was clean (not a crash)
        if "timed out" in build_error.lower():
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                error_detail=build_error,
                elapsed_sec=time.monotonic() - t0,
            )

        # Check for crash indicators
        crash_indicators = [
            "signal",
            "segfault",
            "assertion failed",
            "panic",
            "abort",
            "core dumped",
        ]
        build_error_lower = build_error.lower()
        is_crash = any(ind in build_error_lower for ind in crash_indicators)

        if is_crash:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="reject_crash",
                error_detail=f"Molt crashed while rejecting ({reason}): {build_error[:300]}",
                elapsed_sec=time.monotonic() - t0,
            )

        # Clean rejection -- good
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="reject_pass",
            error_detail=f"Correctly rejected ({reason})",
            elapsed_sec=time.monotonic() - t0,
        )
    finally:
        try:
            Path(source_path).unlink(missing_ok=True)
        except OSError:
            pass
