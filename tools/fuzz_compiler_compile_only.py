from __future__ import annotations

import os
import tempfile
from pathlib import Path
from random import Random

from tools.fuzz_compiler_execution import _build_env, compile_molt
from tools.fuzz_compiler_reporting import _log, _save_failure
from tools.fuzz_compiler_types import FuzzResult, FuzzSummary

# ---------------------------------------------------------------------------
# Compile-only fuzzer (hypothesmith)
# ---------------------------------------------------------------------------


class CompileOnlyFuzzer:
    """Uses hypothesmith for compile-only crash testing."""

    def run(
        self,
        count: int,
        seed: int,
        profile: str,
        timeout: float,
        verbose: bool,
        output_dir: Path | None,
    ) -> FuzzSummary:
        try:
            from hypothesis import HealthCheck, find, settings
            from hypothesmith import from_grammar
        except ImportError:
            _log(
                "hypothesmith not installed; run: uv add --dev hypothesmith hypothesis"
            )
            summary = FuzzSummary()
            summary.total = 0
            return summary

        env = _build_env()
        summary = FuzzSummary()
        ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
        tmpdir_base = (
            ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
        )

        _log(f"Compile-only fuzzer: {count} programs, seed={seed}, profile={profile}")

        with tempfile.TemporaryDirectory(
            prefix="molt_fuzz_co_", dir=tmpdir_base
        ) as tmpdir:
            for i in range(count):
                program_seed = seed + i
                summary.total += 1

                # Generate using hypothesmith via hypothesis's find()
                try:
                    source = find(
                        from_grammar(),
                        lambda x: True,
                        settings=settings(
                            max_examples=1,
                            database=None,
                            suppress_health_check=list(HealthCheck),
                        ),
                        random=Random(program_seed),
                    )
                except Exception as e:
                    if verbose:
                        _log(f"  [#{i:4d}] GENERATE_ERROR: {type(e).__name__}: {e}")
                    continue

                source_path = os.path.join(tmpdir, f"fuzz_co_{i:06d}.py")
                Path(source_path).write_text(source)

                try:
                    binary, build_error = compile_molt(
                        source_path, profile, timeout, env
                    )
                    if binary is not None:
                        summary.compile_only_ok += 1
                        if verbose:
                            _log(f"  [#{i:4d}] OK (compiled)")
                    else:
                        # Non-zero exit with clean error is fine
                        if "timed out" in build_error.lower():
                            summary.timeouts += 1
                            if verbose:
                                _log(f"  [#{i:4d}] TIMEOUT")
                        else:
                            summary.compile_only_ok += 1
                            if verbose:
                                _log(f"  [#{i:4d}] OK (rejected cleanly)")
                except Exception as e:
                    # Actual crash
                    summary.compile_only_crash += 1
                    _log(f"  [#{i:4d}] CRASH: {type(e).__name__}: {e}")
                    if output_dir:
                        result = FuzzResult(
                            program_id=i,
                            seed=program_seed,
                            source=source,
                            status="crash",
                            error_detail=str(e),
                        )
                        _save_failure(result, output_dir)
                finally:
                    try:
                        Path(source_path).unlink(missing_ok=True)
                    except OSError:
                        pass

        return summary
