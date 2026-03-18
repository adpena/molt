"""Tests for IR-level determinism of the Molt compiler.

Ensures that:
- Compiling the same source twice produces byte-identical IR JSON.
- PYTHONHASHSEED does not affect compiler output.
- Compile order does not affect per-program IR output.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

ROOT = Path(__file__).resolve().parents[2]
BASIC_DIR = ROOT / "tests" / "differential" / "basic"
FRONTEND_INIT = ROOT / "src" / "molt" / "frontend" / "__init__.py"


def _compile_source_to_ir(source_text: str) -> str:
    """Compile *source_text* to IR JSON string using the in-process compiler.

    Returns the JSON string (not parsed) so we can do byte-level comparison.
    """
    # Import here so collection doesn't fail if molt isn't installed yet.
    from molt.frontend import compile_to_tir  # type: ignore[import-untyped]

    ir_dict = compile_to_tir(source_text)
    return json.dumps(ir_dict, sort_keys=True, indent=2)


def _compile_source_to_ir_subprocess(
    source_text: str,
    *,
    pythonhashseed: str = "0",
) -> str:
    """Compile via a subprocess to ensure full process isolation."""
    script = (
        "import json, sys; "
        "sys.path.insert(0, {src!r}); "
        "from molt.frontend import compile_to_tir; "
        "ir = compile_to_tir(sys.stdin.read()); "
        "print(json.dumps(ir, sort_keys=True, indent=2))"
    ).format(src=str(ROOT / "src"))

    env = os.environ.copy()
    env["PYTHONHASHSEED"] = pythonhashseed

    result = subprocess.run(
        [sys.executable, "-c", script],
        input=source_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    if result.returncode != 0:
        pytest.fail(
            f"Subprocess compilation failed (rc={result.returncode}):\n{result.stderr[:2000]}"
        )
    return result.stdout


# ---------------------------------------------------------------------------
# Collect programs
# ---------------------------------------------------------------------------


def _basic_programs() -> list[Path]:
    """Return the first 20 .py files in tests/differential/basic/."""
    if not BASIC_DIR.is_dir():
        return []
    files = sorted(BASIC_DIR.glob("*.py"))
    return files[:20]


BASIC_PROGRAMS = _basic_programs()


# ---------------------------------------------------------------------------
# Tests: compile-twice in-process
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "program",
    BASIC_PROGRAMS,
    ids=[p.name for p in BASIC_PROGRAMS],
)
def test_ir_determinism_in_process(program: Path) -> None:
    """Compiling the same source twice in the same process gives identical IR."""
    source = program.read_text()
    ir_a = _compile_source_to_ir(source)
    ir_b = _compile_source_to_ir(source)
    assert ir_a == ir_b, (
        f"IR differs for {program.name} between two in-process compilations"
    )


# ---------------------------------------------------------------------------
# Tests: cross-process determinism
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "program",
    BASIC_PROGRAMS[:5],  # limit to 5 to keep subprocess tests fast
    ids=[p.name for p in BASIC_PROGRAMS[:5]],
)
def test_ir_determinism_cross_process(program: Path) -> None:
    """Two separate Python processes produce identical IR for the same source."""
    source = program.read_text()
    ir_a = _compile_source_to_ir_subprocess(source, pythonhashseed="0")
    ir_b = _compile_source_to_ir_subprocess(source, pythonhashseed="0")
    assert ir_a == ir_b, (
        f"IR differs for {program.name} between two separate processes"
    )


# ---------------------------------------------------------------------------
# Tests: PYTHONHASHSEED independence
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "program",
    BASIC_PROGRAMS[:5],
    ids=[p.name for p in BASIC_PROGRAMS[:5]],
)
def test_ir_hashseed_independence(program: Path) -> None:
    """Different PYTHONHASHSEED values must not change compiler IR output."""
    source = program.read_text()
    seeds = ["0", "42", "12345", "99999"]
    ir_results = []
    for seed in seeds:
        ir = _compile_source_to_ir_subprocess(source, pythonhashseed=seed)
        ir_results.append(ir)

    reference = ir_results[0]
    for i, ir in enumerate(ir_results[1:], 1):
        assert ir == reference, (
            f"IR for {program.name} differs with PYTHONHASHSEED={seeds[i]} "
            f"vs PYTHONHASHSEED={seeds[0]}"
        )


# ---------------------------------------------------------------------------
# Tests: compile-order independence
# ---------------------------------------------------------------------------


def test_compile_order_independence() -> None:
    """Compiling programs in different order must not affect individual IR outputs.

    Compile A then B, versus B then A -- each program's IR should be identical
    regardless of order.
    """
    if len(BASIC_PROGRAMS) < 2:
        pytest.skip("Need at least 2 basic programs")

    prog_a = BASIC_PROGRAMS[0]
    prog_b = BASIC_PROGRAMS[1]
    src_a = prog_a.read_text()
    src_b = prog_b.read_text()

    # Order 1: A then B
    ir_a1 = _compile_source_to_ir(src_a)
    ir_b1 = _compile_source_to_ir(src_b)

    # Order 2: B then A
    ir_b2 = _compile_source_to_ir(src_b)
    ir_a2 = _compile_source_to_ir(src_a)

    assert ir_a1 == ir_a2, (
        f"IR for {prog_a.name} changed depending on compile order"
    )
    assert ir_b1 == ir_b2, (
        f"IR for {prog_b.name} changed depending on compile order"
    )
