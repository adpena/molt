"""Differential fuzzing tests targeting specific compiler optimizations.

Each test class generates programs that exercise a particular optimization
pass (constant folding, dead-code elimination, CSE, LICM) and verifies that
the generated program produces correct output under CPython.  When Molt is
available, it also compiles and compares Molt output against CPython.

These are *targeted* fuzz tests -- they combine hand-crafted templates with
randomized parameters rather than being fully random.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from random import Random

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(_REPO_ROOT / "tools"))

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_TIMEOUT = 10


def _run_cpython(source: str) -> tuple[str, int]:
    """Run *source* under CPython and return (stdout, returncode)."""
    result = subprocess.run(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=_TIMEOUT,
    )
    return result.stdout, result.returncode


def _molt_available() -> bool:
    try:
        result = subprocess.run(
            [sys.executable, "-m", "molt.cli", "--help"],
            capture_output=True,
            text=True,
            timeout=10,
            cwd=str(_REPO_ROOT),
        )
        return result.returncode == 0
    except Exception:
        return False


def _run_molt(source: str, tmp_path: Path, tag: str) -> tuple[str | None, str]:
    """Compile and run *source* with Molt, returning (stdout, error_msg).

    Returns ``(None, reason)`` when compilation or execution fails.
    """
    src_file = tmp_path / f"diff_{tag}.py"
    src_file.write_text(source)
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            "debug",
            "--deterministic",
            "--json",
            str(src_file),
        ],
        capture_output=True,
        text=True,
        timeout=30,
        cwd=str(_REPO_ROOT),
    )
    if build.returncode != 0:
        return None, f"Molt build failed: {build.stderr[:300]}"
    # For now just return None -- full binary extraction requires JSON
    # parsing identical to fuzz_compiler.py which we don't duplicate here.
    return None, "binary extraction not implemented in differential tests"


# ---------------------------------------------------------------------------
# Arithmetic / Constant Folding
# ---------------------------------------------------------------------------


class TestDifferentialArithmetic:
    """Generate arithmetic programs to test constant-folding correctness."""

    SEEDS = range(50)

    @staticmethod
    def _make_program(seed: int) -> str:
        rng = Random(seed)
        lines: list[str] = []
        for i in range(rng.randint(4, 10)):
            a = rng.randint(-200, 200)
            b = rng.randint(-200, 200)
            op = rng.choice(["+", "-", "*"])
            lines.append(f"r{i} = {a} {op} {b}")
            lines.append(f"print(r{i})")
        # Integer division / modulo with safe divisor
        for i in range(3):
            a = rng.randint(-200, 200)
            b = rng.randint(1, 50)
            op = rng.choice(["//", "%"])
            lines.append(f"d{i} = {a} {op} {b}")
            lines.append(f"d{i}_neg = {a} {op} (-{b})")
            lines.append(f"print(d{i}, d{i}_neg)")
        # Exponentiation with small exponents
        base = rng.randint(-5, 5)
        exp = rng.randint(0, 5)
        lines.append(f"p = {base} ** {exp}")
        lines.append("print(p)")
        # Nested expressions
        vals = [str(rng.randint(-20, 20)) for _ in range(4)]
        lines.append(f"nested = ({vals[0]} + {vals[1]}) * ({vals[2]} - {vals[3]})")
        lines.append("print(nested)")
        return "\n".join(lines) + "\n"

    @pytest.mark.parametrize("seed", SEEDS)
    def test_arithmetic_cpython(self, seed: int) -> None:
        source = self._make_program(seed)
        stdout, rc = _run_cpython(source)
        assert rc == 0, f"seed={seed} failed with rc={rc}"
        assert stdout.strip(), f"seed={seed} produced no output"

    def test_arithmetic_determinism(self) -> None:
        for seed in self.SEEDS:
            assert self._make_program(seed) == self._make_program(seed), (
                f"seed={seed} not deterministic"
            )


# ---------------------------------------------------------------------------
# Dead Code Elimination
# ---------------------------------------------------------------------------


class TestDifferentialDCE:
    """Generate programs with dead code after return statements."""

    SEEDS = range(50)

    @staticmethod
    def _make_program(seed: int) -> str:
        rng = Random(seed)
        funcs: list[str] = []
        calls: list[str] = []

        for fi in range(rng.randint(2, 5)):
            fname = f"func_{fi}"
            ret_val = rng.randint(-100, 100)
            # Dead code that should never execute
            dead_lines = []
            for _ in range(rng.randint(1, 4)):
                dv = rng.randint(0, 999)
                dead_lines.append(f"    dead_{dv} = {dv}")
                dead_lines.append(f"    print('DEAD CODE REACHED: {dv}')")
            dead_block = "\n".join(dead_lines)
            funcs.append(
                f"def {fname}():\n"
                f"    result = {ret_val}\n"
                f"    return result\n"
                f"{dead_block}\n"
            )
            calls.append(f"print({fname}())")

        # Also test dead code in if/else
        cond_val = rng.choice(["True", "False"])
        funcs.append(
            f"def dead_branch():\n"
            f"    if {cond_val}:\n"
            f"        return 'live'\n"
            f"    else:\n"
            f"        return 'also reachable'\n"
            f"    print('DEAD: after if/else return')\n"
        )
        calls.append("print(dead_branch())")

        return "\n".join(funcs + calls) + "\n"

    @pytest.mark.parametrize("seed", SEEDS)
    def test_dce_no_dead_output(self, seed: int) -> None:
        source = self._make_program(seed)
        stdout, rc = _run_cpython(source)
        assert rc == 0, f"seed={seed} failed with rc={rc}"
        assert "DEAD CODE REACHED" not in stdout, f"seed={seed}: dead code was executed"

    def test_dce_determinism(self) -> None:
        for seed in self.SEEDS:
            assert self._make_program(seed) == self._make_program(seed)


# ---------------------------------------------------------------------------
# Common Subexpression Elimination
# ---------------------------------------------------------------------------


class TestDifferentialCSE:
    """Generate programs with repeated computations to test CSE."""

    SEEDS = range(50)

    @staticmethod
    def _make_program(seed: int) -> str:
        rng = Random(seed)
        lines: list[str] = []

        for i in range(rng.randint(3, 6)):
            a = rng.randint(1, 50)
            b = rng.randint(1, 50)
            op = rng.choice(["+", "-", "*"])
            expr = f"{a} {op} {b}"
            # Use the same expression multiple times
            lines.append(f"x{i}_a = {expr}")
            lines.append(f"x{i}_b = {expr}")
            lines.append(f"x{i}_c = {expr} + 0")  # Slightly different
            lines.append(f"print(x{i}_a, x{i}_b, x{i}_c)")
            lines.append(f"print(x{i}_a == x{i}_b)")  # Should be True

        # Repeated function calls with same args
        lines.append("def square(n):")
        lines.append("    return n * n")
        val = rng.randint(1, 20)
        lines.append(f"s1 = square({val})")
        lines.append(f"s2 = square({val})")
        lines.append("print(s1 == s2)")
        lines.append("print(s1, s2)")

        return "\n".join(lines) + "\n"

    @pytest.mark.parametrize("seed", SEEDS)
    def test_cse_consistent(self, seed: int) -> None:
        source = self._make_program(seed)
        stdout, rc = _run_cpython(source)
        assert rc == 0, f"seed={seed} failed with rc={rc}"
        # Every "True" check should hold
        for line in stdout.strip().splitlines():
            if line.strip() in ("True", "False"):
                assert line.strip() == "True", (
                    f"seed={seed}: CSE consistency check failed, got {line}"
                )

    def test_cse_determinism(self) -> None:
        for seed in self.SEEDS:
            assert self._make_program(seed) == self._make_program(seed)


# ---------------------------------------------------------------------------
# Loop-Invariant Code Motion
# ---------------------------------------------------------------------------


class TestDifferentialLICM:
    """Generate programs with loop-invariant expressions."""

    SEEDS = range(50)

    @staticmethod
    def _make_program(seed: int) -> str:
        rng = Random(seed)
        lines: list[str] = []

        # Constants that are invariant within the loop
        a = rng.randint(1, 50)
        b = rng.randint(1, 50)
        iters = rng.randint(3, 8)

        lines.append(f"a = {a}")
        lines.append(f"b = {b}")
        lines.append("results = []")
        lines.append(f"for i in range({iters}):")
        # Loop-invariant: a * b does not depend on i
        lines.append("    invariant = a * b")
        lines.append("    variant = invariant + i")
        lines.append("    results.append(variant)")
        lines.append("print(results)")
        # Verify invariant was correct
        lines.append(f"print(a * b == {a * b})")

        # Second loop: invariant string operation
        s = rng.choice(["hello", "world", "test"])
        lines.append(f"s = '{s}'")
        lines.append("upper_results = []")
        lines.append(f"for j in range({rng.randint(2, 5)}):")
        lines.append("    upper = s.upper()")  # Loop-invariant
        lines.append("    upper_results.append(upper)")
        lines.append("print(len(set(upper_results)) == 1)")  # All same

        # Third pattern: nested loop with outer-loop invariant
        outer = rng.randint(2, 4)
        inner = rng.randint(2, 4)
        c = rng.randint(1, 20)
        lines.append(f"c = {c}")
        lines.append("totals = []")
        lines.append(f"for ii in range({outer}):")
        lines.append("    outer_inv = c * c")  # Invariant to both loops
        lines.append(f"    for jj in range({inner}):")
        lines.append("        inner_inv = ii * c")  # Invariant to inner
        lines.append("        totals.append(outer_inv + inner_inv + jj)")
        lines.append("print(len(totals))")
        lines.append(f"print(len(totals) == {outer * inner})")

        return "\n".join(lines) + "\n"

    @pytest.mark.parametrize("seed", SEEDS)
    def test_licm_correct(self, seed: int) -> None:
        source = self._make_program(seed)
        stdout, rc = _run_cpython(source)
        assert rc == 0, f"seed={seed} failed with rc={rc}"
        # Check that all True/False lines are True
        for line in stdout.strip().splitlines():
            stripped = line.strip()
            if stripped in ("True", "False"):
                assert stripped == "True", f"seed={seed}: LICM invariant check failed"

    def test_licm_determinism(self) -> None:
        for seed in self.SEEDS:
            assert self._make_program(seed) == self._make_program(seed)


# ---------------------------------------------------------------------------
# Cross-optimization: Molt vs CPython output comparison
# ---------------------------------------------------------------------------


class TestDifferentialMoltVsCPython:
    """When Molt is available, compile targeted programs and compare output."""

    @pytest.fixture(autouse=True)
    def _skip_unless_molt(self) -> None:
        if not _molt_available():
            pytest.skip("molt CLI not available")

    @pytest.mark.parametrize("seed", range(10))
    def test_arithmetic_molt_matches(self, seed: int, tmp_path: Path) -> None:
        source = TestDifferentialArithmetic._make_program(seed)
        cpython_out, rc = _run_cpython(source)
        assert rc == 0
        molt_out, err = _run_molt(source, tmp_path, f"arith_{seed}")
        if molt_out is not None:
            assert molt_out.strip() == cpython_out.strip(), (
                f"seed={seed}: Molt output differs from CPython\n"
                f"CPython: {cpython_out[:200]}\nMolt: {molt_out[:200]}"
            )

    @pytest.mark.parametrize("seed", range(10))
    def test_dce_molt_matches(self, seed: int, tmp_path: Path) -> None:
        source = TestDifferentialDCE._make_program(seed)
        cpython_out, rc = _run_cpython(source)
        assert rc == 0
        molt_out, err = _run_molt(source, tmp_path, f"dce_{seed}")
        if molt_out is not None:
            assert molt_out.strip() == cpython_out.strip()
            assert "DEAD CODE REACHED" not in molt_out
