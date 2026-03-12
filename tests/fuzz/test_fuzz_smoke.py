"""Smoke tests for the grammar-based Python program fuzzer.

Generates a small batch of programs and verifies they are all syntactically
valid Python (parseable by ast.parse) and deterministic (same seed produces
same output).
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path
from random import Random

# Allow importing tools/fuzz_compiler.py from the repo root.
_REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(_REPO_ROOT / "tools"))

from fuzz_compiler import SafeProgramGenerator  # noqa: E402


def _generate(seed: int, max_stmts: int = 15) -> str:
    rng = Random(seed)
    gen = SafeProgramGenerator(rng, max_depth=3, max_stmts=max_stmts)
    return gen.generate()


class TestFuzzSmokeValid:
    """Every generated program must be syntactically valid Python."""

    def test_ten_seeds_parse(self) -> None:
        for seed in range(10):
            source = _generate(seed)
            try:
                ast.parse(source)
            except SyntaxError as exc:
                raise AssertionError(
                    f"seed={seed} produced invalid Python:\n  {exc}\n\n{source}"
                ) from exc

    def test_hundred_seeds_parse(self) -> None:
        failures: list[int] = []
        for seed in range(100, 200):
            source = _generate(seed)
            try:
                ast.parse(source)
            except SyntaxError:
                failures.append(seed)
        assert not failures, f"Invalid Python for seeds: {failures}"


class TestFuzzSmokeDeterminism:
    """Same seed must always produce the exact same program."""

    def test_deterministic_output(self) -> None:
        for seed in range(10):
            a = _generate(seed)
            b = _generate(seed)
            assert a == b, f"seed={seed} produced different output on second call"


class TestFuzzSmokePrintsOutput:
    """Every generated program must contain at least one print() call."""

    def test_has_print(self) -> None:
        for seed in range(20):
            source = _generate(seed)
            assert "print(" in source, f"seed={seed} has no print() call:\n{source}"


class TestFuzzSmokeTerminates:
    """Every generated program must terminate (no infinite loops).

    We verify this by running programs under CPython with a short timeout.
    This is a best-effort check -- it does not prove termination, but it
    catches unbounded loops in the generator.
    """

    def test_programs_terminate(self) -> None:
        import subprocess

        for seed in range(5):
            source = _generate(seed)
            result = subprocess.run(
                [sys.executable, "-c", source],
                capture_output=True,
                text=True,
                timeout=10,
            )
            # We don't require rc==0 (some programs may have runtime errors
            # due to edge cases), but they must not hang.
            assert result.returncode is not None, f"seed={seed} did not terminate"
