"""Tests for IR-level determinism of the Molt compiler.

Ensures that:
- Compiling the same source twice produces byte-identical IR JSON.
- PYTHONHASHSEED does not affect compiler output.
- Compile order does not affect per-program IR output.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process

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
    parse_codec: str = "msgpack",
) -> str:
    """Compile via a subprocess to ensure full process isolation."""
    script = (
        "import json, sys; "
        "sys.path.insert(0, {src!r}); "
        "from molt.frontend import compile_to_tir; "
        "ir = compile_to_tir(sys.stdin.read(), parse_codec={codec!r}); "
        "print(json.dumps(ir, sort_keys=True, indent=2))"
    ).format(src=str(ROOT / "src"), codec=parse_codec)

    env = os.environ.copy()
    # ``pythonhashseed="random"`` exercises the *unpinned* path: a fresh,
    # process-chosen hash seed on every run.  This is the only configuration
    # that catches a hash-order leak that happens to agree across a fixed
    # seed set (the #34 async-local-offset bug evaded a fixed-seed-only test).
    env["PYTHONHASHSEED"] = pythonhashseed

    result = run_native_test_process(
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


def _compile_outcome_subprocess(
    source_text: str,
    *,
    pythonhashseed: str,
    parse_codec: str,
) -> str:
    """Return the deterministic *outcome* of compiling, success or failure.

    On success this is the canonical IR JSON.  On a compile error it is a
    normalized ``COMPILE_ERROR::<ExceptionType>::<message>`` string.  Either
    way the outcome must be byte-identical across hash seeds — a program that
    raises the *same* CompatibilityError on every seed is still deterministic;
    only an outcome that *varies* with the seed is a leak.  (The plain IR
    helper above ``pytest.fail``s on any error, which is right for programs
    that are expected to compile but wrong for asserting determinism over a
    set that may include legitimately-unsupported constructs.)
    """
    wrapper = (
        "import json, sys\n"
        "sys.path.insert(0, {src!r})\n"
        "src = sys.stdin.read()\n"
        "try:\n"
        "    from molt.frontend import compile_to_tir\n"
        "    ir = compile_to_tir(src, parse_codec={codec!r})\n"
        "    sys.stdout.write('IR::' + json.dumps(ir, sort_keys=True))\n"
        "except BaseException as exc:\n"
        "    sys.stdout.write("
        "'COMPILE_ERROR::' + type(exc).__name__ + '::' + str(exc))\n"
    ).format(src=str(ROOT / "src"), codec=parse_codec)

    env = os.environ.copy()
    env["PYTHONHASHSEED"] = pythonhashseed

    result = run_native_test_process(
        [sys.executable, "-c", wrapper],
        input=source_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
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


def _async_programs() -> list[Path]:
    """Async programs that exercise the async-local spill/restore path.

    Async functions spill live values into closure slots across every
    ``await``/``yield`` state boundary.  The slot offsets are assigned in
    ``_spill_async_temporaries``, which historically iterated a *set* of
    spill names — making every assigned offset depend on PYTHONHASHSEED
    (bug #34).  These programs are the regression surface for that class:
    any hash-order leak feeding IR emission shows up here as a per-seed
    IR divergence.
    """
    if not BASIC_DIR.is_dir():
        return []
    return sorted(BASIC_DIR.glob("async_*.py"))


ASYNC_PROGRAMS = _async_programs()


# Programs that exercise other set-iteration -> IR-emission leaks of the #34
# class beyond async spill: unpacking/star targets and structural-pattern
# capture names flow through ``_collect_target_names`` /
# ``_collect_pattern_capture_names`` into the function's co_varnames tuple via
# ``_collect_assigned_names_ordered``.  Both returned sets historically, leaking
# hash order into the emitted IR.  These are the regression surface for that
# class; the names are pinned so a regression in any one is attributable.
_HASH_ORDER_LEAK_PROGRAMS = [
    "unpack_assignment.py",
    "stress_structures_pass.py",
    "stress_structures_fail.py",
    "sum_map_function_defaults.py",
    "ws_pair_basic.py",
    "match_guard_capture_order.py",
    "pattern_matching_core_matrix.py",
    "pattern_matching_class_guard_matrix.py",
    "pep634_pattern_matching_more.py",
]


def _hash_order_leak_programs() -> list[Path]:
    if not BASIC_DIR.is_dir():
        return []
    return [BASIC_DIR / name for name in _HASH_ORDER_LEAK_PROGRAMS]


HASH_ORDER_LEAK_PROGRAMS = _hash_order_leak_programs()


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
    assert ir_a == ir_b, f"IR differs for {program.name} between two separate processes"


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
# Tests: async-spill hash-order independence (regression for #34)
#
# The async-local spill path assigns closure-slot offsets while iterating a
# set of spill names.  Before the fix, that iteration order — and thus the
# emitted offsets — varied with PYTHONHASHSEED.  A fixed-seed-only check
# missed it because the chosen seeds happened to agree; the decisive
# configuration is an *unpinned* (process-random) seed, run alongside a few
# fixed seeds, across BOTH parse codecs.
# ---------------------------------------------------------------------------


def _assert_outcome_hashseed_stable(program: Path, parse_codec: str) -> None:
    """Compile *program* under several fixed seeds + an explicit random seed +
    the unpinned ``"random"`` path and assert a single byte-identical outcome.

    ``"random"`` is decisive: it runs the compiler under a fresh, process-chosen
    hash seed, so it catches a leak even when a hand-picked fixed-seed set
    happens to agree (which is exactly how #34 evaded the original test).
    """
    source = program.read_text()
    seeds = ["0", "1", "42", "12345", str(_random_seed()), "random"]
    reference = _compile_outcome_subprocess(
        source, pythonhashseed=seeds[0], parse_codec=parse_codec
    )
    for seed in seeds[1:]:
        outcome = _compile_outcome_subprocess(
            source, pythonhashseed=seed, parse_codec=parse_codec
        )
        assert outcome == reference, (
            f"Compile outcome for {program.name} [{parse_codec}] differs with "
            f"PYTHONHASHSEED={seed} vs PYTHONHASHSEED={seeds[0]} — a hash-order "
            f"leak into IR emission (regression of #34)"
        )


@pytest.mark.parametrize(
    "program",
    ASYNC_PROGRAMS,
    ids=[p.name for p in ASYNC_PROGRAMS],
)
@pytest.mark.parametrize("parse_codec", ["msgpack", "json"])
def test_async_spill_hashseed_independence(program: Path, parse_codec: str) -> None:
    """Async-spill IR must be byte-identical across hash seeds, unpinned."""
    _assert_outcome_hashseed_stable(program, parse_codec)


@pytest.mark.parametrize(
    "program",
    HASH_ORDER_LEAK_PROGRAMS,
    ids=[p.name for p in HASH_ORDER_LEAK_PROGRAMS],
)
@pytest.mark.parametrize("parse_codec", ["msgpack", "json"])
def test_unpack_and_pattern_hashseed_independence(
    program: Path, parse_codec: str
) -> None:
    """Unpacking-target and pattern-capture names must not leak hash order.

    These flow through ``_collect_target_names`` / ``_collect_pattern_capture
    _names`` into the function's co_varnames tuple; both returned sets before
    the fix, so the emitted IR depended on PYTHONHASHSEED.
    """
    _assert_outcome_hashseed_stable(program, parse_codec)


def _random_seed() -> int:
    import random

    return random.randint(1, 2**31 - 1)


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

    assert ir_a1 == ir_a2, f"IR for {prog_a.name} changed depending on compile order"
    assert ir_b1 == ir_b2, f"IR for {prog_b.name} changed depending on compile order"
