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
# class beyond async spill.  Three name collectors historically returned sets
# whose hash-ordered iteration reached emitted IR:
#   * ``_collect_target_names`` — unpacking / star targets;
#   * ``_collect_pattern_capture_names`` — structural-pattern captures;
#     (both feed the co_varnames tuple via ``_collect_assigned_names_ordered``)
#   * ``_collect_namedexpr_names`` / ``_collect_inline_comp_walrus_names`` —
#     comprehension walrus (:=) targets, synced to the enclosing scope with
#     per-name INDEX / module-attr-set ops.
# The names are pinned so a regression in any one is attributable.
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
    "comprehension_walrus_nested_targets.py",
    "comprehension_walrus_and_or_filters.py",
    "comprehension_nested_walrus.py",
    "pep572_walrus_edges.py",
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


# ---------------------------------------------------------------------------
# Tests: mid-end pass selection must be independent of wall-clock time (#73)
#
# The mid-end optimiser degraded its pass pipeline (capping rounds, shrinking
# SCCP/CSE iteration caps, then disabling CSE / edge-threading / LICM) when a
# per-function compile exceeded a *wall-clock time budget*.  That made the
# emitted IR a function of how fast the machine happened to run: identical
# source + identical PYTHONHASHSEED produced divergent IR across processes
# whenever a compile ran slow enough to trip the budget and disable CSE — a
# silent determinism-contract violation affecting the majority of programs.
#
# The degrade ladder now gates on a DETERMINISTIC work-unit budget (op counts),
# so pass selection — and thus the IR — is a pure function of the input.  The
# decisive, deterministic regression check: the compiled IR must be byte-
# identical regardless of the retired wall-clock budget knob
# ``MOLT_MIDEND_BUDGET_MS``. On the buggy compiler a tiny budget forced
# degradation and changed the IR; on the fixed compiler this variable has no
# active reader. Deterministic pass-selection overrides use
# ``MOLT_MIDEND_WORK_BUDGET`` instead.
# These programs were verified to degrade under a tiny time budget on the buggy
# compiler (so the assertion genuinely fails pre-fix).
# ---------------------------------------------------------------------------

_MIDEND_WALLTIME_SENSITIVE_PROGRAMS = [
    "arith.py",
    "args_kwargs.py",
    "assignment_target_eval_order.py",
    "assignment_unpack_error_propagation.py",
    "pep634_pattern_matching_more.py",
    "pattern_matching_core_matrix.py",
    "comprehension_nested_walrus.py",
    "pep572_walrus_edges.py",
]


def _midend_walltime_sensitive_programs() -> list[Path]:
    if not BASIC_DIR.is_dir():
        return []
    return [BASIC_DIR / name for name in _MIDEND_WALLTIME_SENSITIVE_PROGRAMS]


MIDEND_WALLTIME_SENSITIVE_PROGRAMS = _midend_walltime_sensitive_programs()


def _compile_ir_subprocess_with_env(
    source_text: str,
    *,
    parse_codec: str,
    extra_env: dict[str, str],
    pythonhashseed: str = "0",
) -> str:
    """Compile in a subprocess with *extra_env* applied; return canonical IR
    (or a normalized ``COMPILE_ERROR::`` string)."""
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
    env.update(extra_env)

    result = run_native_test_process(
        [sys.executable, "-c", wrapper],
        input=source_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    return result.stdout


# The spec's hash-seed sweep set for #73, plus the unpinned process-random
# path.  The fixed seeds pin the regression to attributable values; ``random``
# exercises a fresh process-chosen seed (the configuration that caught the #34
# async-offset leak when a hand-picked fixed set happened to agree).
_MIDEND_DETERMINISM_SEEDS = ("0", "1", "7", "42", "1337", "random")
# Retired wall-clock budget knob values.  ``1``/``5`` ms are tiny enough that,
# on the pre-fix *time*-gated compiler, the degrade ladder fired and disabled
# CSE/LICM/edge-threading (changing the IR); ``None`` is the default and the
# large value can never trip.  On the fixed compiler this variable has no active
# reader, so every value must yield byte-identical IR.
_MIDEND_BUDGET_KNOBS: tuple[str | None, ...] = (None, "1", "5", "999999999")


@pytest.mark.parametrize(
    "program",
    MIDEND_WALLTIME_SENSITIVE_PROGRAMS,
    ids=[p.name for p in MIDEND_WALLTIME_SENSITIVE_PROGRAMS],
)
@pytest.mark.parametrize("parse_codec", ["msgpack", "json"])
def test_midend_ir_independent_of_walltime_budget(
    program: Path, parse_codec: str
) -> None:
    """Mid-end pass selection — and the emitted IR — must be a pure function of
    the input: independent of the retired wall-clock time budget env AND of
    PYTHONHASHSEED (#73; #34 bug class).

    The compiled IR must be byte-identical across the full cross-product of
    {hash seed} x {retired wall-clock budget knob}:

    * **Budget axis** (the decisive #73 regression): the mid-end's pass-degrade
      ladder used to gate on ``time.perf_counter()``, so a compile that ran slow
      enough — or was handed a tiny ``MOLT_MIDEND_BUDGET_MS`` — degraded the
      pipeline and disabled CSE/const-dedup, changing the IR.  That made the
      output a function of machine speed (a flaky, load-dependent determinism
      violation).  Sweeping the retired budget knob *deterministically* forces
      the same degraded path a slow machine would hit by chance, so this axis
      fails on the pre-fix tree (three distinct IR hashes were observed for
      these programs) and passes once the variable is ignored and the ladder
      gates on deterministic work units.
    * **Seed axis** (#34 bug class, guarded defensively): any set-iteration that
      reaches IR emission — e.g. the SCCP worklist's ``out_changed_keys`` set
      union, whose order steers the fixed-point schedule and thus cap behaviour
      — would make the IR depend on PYTHONHASHSEED.  Crossing the seeds with a
      *tiny* budget is what makes this axis decisive: near the iteration cap, a
      hash-ordered schedule change can flip whether the fixed point converges,
      which a fixed-seed-only or default-budget-only sweep would miss.

    Every checked (seed, budget) outcome is compared to a single
    (seed=0, default-budget) reference; any divergence is a determinism-contract
    regression.  We cover both axes decisively without an exhaustive (and largely
    redundant) cross-product: the budget is swept across every knob at the
    reference seed (the decisive #73 configuration), and the hash seed is swept
    across the spec set at the *tightest* budget (the decisive #34 configuration,
    where a hash-ordered worklist schedule is most able to flip cap behaviour).
    """
    source = program.read_text()
    reference = _compile_ir_subprocess_with_env(
        source, parse_codec=parse_codec, extra_env={}, pythonhashseed="0"
    )
    assert not reference.startswith("COMPILE_ERROR::"), (
        f"{program.name} [{parse_codec}] unexpectedly failed to compile: "
        f"{reference[:300]}"
    )

    def _check(seed: str, budget: str | None) -> None:
        extra_env = {} if budget is None else {"MOLT_MIDEND_BUDGET_MS": budget}
        outcome = _compile_ir_subprocess_with_env(
            source, parse_codec=parse_codec, extra_env=extra_env, pythonhashseed=seed
        )
        assert outcome == reference, (
            f"IR for {program.name} [{parse_codec}] changed with "
            f"PYTHONHASHSEED={seed}, "
            f"MOLT_MIDEND_BUDGET_MS={budget if budget is not None else '<default>'}"
            f" vs the (seed=0, default-budget) reference — the mid-end degrade "
            f"ladder is gating on the retired wall-clock env or a set-iteration order is "
            f"leaking into IR emission (regression of #73 / #34)"
        )

    # Budget axis at the reference seed: the decisive #73 regression check.
    for budget in _MIDEND_BUDGET_KNOBS:
        _check("0", budget)
    # Seed axis at the tightest budget: the decisive #34-class check (a
    # hash-ordered SCCP schedule near the iteration cap is most able to diverge).
    tightest_budget = _MIDEND_BUDGET_KNOBS[1]
    for seed in _MIDEND_DETERMINISM_SEEDS:
        _check(seed, tightest_budget)


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
