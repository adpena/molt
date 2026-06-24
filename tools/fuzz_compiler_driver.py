from __future__ import annotations

import os
import tempfile
from pathlib import Path
from random import Random

from tools.fuzz_compiler_core import fuzz_one_reject, fuzz_one_safe
from tools.fuzz_compiler_execution import _build_env
from tools.fuzz_compiler_reporting import _log, _print_diff_snippet, _save_failure
from tools.fuzz_compiler_safe import SafeProgramGenerator
from tools.fuzz_compiler_shrink import _validate_syntax
from tools.fuzz_compiler_types import FuzzSummary

# ---------------------------------------------------------------------------
# Generate-only mode
# ---------------------------------------------------------------------------


def run_generate_only(
    count: int,
    seed: int,
    output_dir: Path,
    max_stmts: int = 25,
) -> int:
    """Generate *count* programs to *output_dir* without running them.

    Returns the number of valid Python programs written.
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    valid = 0
    for i in range(count):
        program_seed = seed + i
        rng = Random(program_seed)
        gen = SafeProgramGenerator(rng=rng, max_stmts=max_stmts)
        source = gen.generate()
        if not _validate_syntax(source):
            _log(f"  [WARN] seed={program_seed} produced invalid Python -- skipping")
            continue
        path = output_dir / f"fuzz_{program_seed}.py"
        path.write_text(source)
        valid += 1
    _log(f"Generated {valid}/{count} valid programs in {output_dir}")
    return valid


# ---------------------------------------------------------------------------
# Main driver
# ---------------------------------------------------------------------------


def run_safe_fuzzer(
    count: int,
    seed: int,
    output_dir: Path | None,
    profile: str,
    timeout: float,
    verbose: bool,
) -> FuzzSummary:
    summary = FuzzSummary()
    env = _build_env()

    ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
    tmpdir_base = (
        ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
    )

    _log(f"Safe-mode fuzzer: {count} programs, seed={seed}, profile={profile}")
    _log(f"  timeout={timeout}s, tmpdir={tmpdir_base}")
    if output_dir:
        _log(f"  output_dir={output_dir}")
    _log("")

    with tempfile.TemporaryDirectory(prefix="molt_fuzz_", dir=tmpdir_base) as tmpdir:
        for i in range(count):
            program_seed = seed + i
            rng = Random(program_seed)
            result = fuzz_one_safe(
                program_id=i,
                seed=program_seed,
                rng=rng,
                profile=profile,
                timeout=timeout,
                env=env,
                verbose=verbose,
                tmpdir=tmpdir,
            )
            summary.total += 1
            if result.status == "pass":
                summary.passed += 1
                if verbose:
                    _log(f"  [#{i:4d}] PASS ({result.elapsed_sec:.1f}s)")
            elif result.status == "mismatch":
                summary.mismatches += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] MISMATCH (seed={program_seed})")
                if verbose:
                    _print_diff_snippet(result)
                if output_dir:
                    saved = _save_failure(result, output_dir)
                    _log(f"         saved: {saved}")
            elif result.status == "build_error":
                summary.build_errors += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] BUILD_ERROR (seed={program_seed})")
                if verbose:
                    _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "molt_run_error":
                summary.molt_run_errors += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] MOLT_RUN_ERROR (seed={program_seed})")
                if verbose:
                    _print_diff_snippet(result)
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "cpython_error":
                summary.cpython_errors += 1
                if verbose:
                    _log(f"  [#{i:4d}] CPYTHON_ERROR (skipped, seed={program_seed})")
            elif result.status == "timeout":
                summary.timeouts += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] TIMEOUT (seed={program_seed})")
                if output_dir:
                    _save_failure(result, output_dir)

    return summary


def run_reject_fuzzer(
    count: int,
    seed: int,
    output_dir: Path | None,
    profile: str,
    timeout: float,
    verbose: bool,
) -> FuzzSummary:
    summary = FuzzSummary()
    env = _build_env()

    ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
    tmpdir_base = (
        ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
    )

    _log(f"Reject-mode fuzzer: {count} programs, seed={seed}, profile={profile}")
    _log(f"  timeout={timeout}s, tmpdir={tmpdir_base}")
    if output_dir:
        _log(f"  output_dir={output_dir}")
    _log("")

    with tempfile.TemporaryDirectory(
        prefix="molt_fuzz_reject_", dir=tmpdir_base
    ) as tmpdir:
        for i in range(count):
            program_seed = seed + i
            rng = Random(program_seed)
            result = fuzz_one_reject(
                program_id=i,
                seed=program_seed,
                rng=rng,
                profile=profile,
                timeout=timeout,
                env=env,
                verbose=verbose,
                tmpdir=tmpdir,
            )
            summary.total += 1
            if result.status == "reject_pass":
                summary.reject_pass += 1
                if verbose:
                    _log(f"  [#{i:4d}] REJECT_PASS ({result.error_detail})")
            elif result.status == "reject_fail":
                summary.reject_fail += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] REJECT_FAIL (seed={program_seed})")
                _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "reject_crash":
                summary.reject_fail += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] REJECT_CRASH (seed={program_seed})")
                _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "timeout":
                summary.timeouts += 1
                _log(f"  [#{i:4d}] TIMEOUT (seed={program_seed})")

    return summary
