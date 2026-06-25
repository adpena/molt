"""Tests for ratio-direction canonicalization (audit meta-bug item 2).

Two non-negotiable invariants:

  * ``molt.metric_ratios.signed_ratio`` is the SOLE implementation authority
    for every wall-clock ratio field: a None / 0 / NaN / inf / negative operand
    yields ``value=None`` (never a finite or infinite number), and every result
    carries explicit ``RatioDirection`` metadata so a downstream consumer can
    never misread the sign of a ratio. ``tools.perf_authority`` re-exports it
    for benchmark tooling.
  * The ``check_ratio_direction`` drift-gate makes the unguarded twin
    UNEXPRESSIBLE: a raw ``<x>_time / <y>_time`` division outside
    ``metric_ratios.py`` fails CI. The gate MUST itself fail on a synthetic
    violation injected here (proving it is not another proxy-measurement
    meta-bug) and pass on the real, canonicalized tree.
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority as pa  # noqa: E402
import check_ratio_direction as crd  # noqa: E402
from molt import metric_ratios  # noqa: E402


# --- signed_ratio: a degenerate time can never become a ratio ----------------


@pytest.mark.parametrize(
    "numer,denom",
    [
        (1.0, None),  # denom missing (external runtime absent / build failure)
        (None, 1.0),  # numer missing
        (None, None),
        (1.0, 0.0),  # zero denom (degenerate)
        (1.0, -0.5),  # negative denom
        (0.0, 1.0),  # zero numer
        (-1.0, 1.0),  # negative numer
        (float("nan"), 1.0),  # non-finite
        (1.0, float("nan")),
        (float("inf"), 1.0),
        (1.0, float("inf")),
        ("oops", 1.0),  # non-numeric (defensive)
        (1.0, "oops"),
    ],
)
def test_signed_ratio_unmeasurable_value_is_none(numer: object, denom: object) -> None:
    block = pa.signed_ratio(
        numer,  # type: ignore[arg-type]
        denom,  # type: ignore[arg-type]
        direction=pa.RatioDirection.MOLT_OVER_BASELINE,
    )
    assert block["value"] is None, (
        f"unmeasurable ({numer!r}, {denom!r}) must yield value=None, got "
        f"{block['value']!r} - a degenerate time must NEVER render as a ratio"
    )
    # Direction metadata is present and correct even when the value is None.
    assert block["direction"] == pa.RatioDirection.MOLT_OVER_BASELINE.value


def test_signed_ratio_none_denom_never_finite() -> None:
    # The exact shape an absent external runtime produces: denom = None.
    for numer in (0.001, 0.063, 1.0, 33.25, 1e9):
        block = pa.signed_ratio(numer, None, direction=pa.RatioDirection.RATIO)
        assert block["value"] is None
        assert not (isinstance(block["value"], float) and math.isfinite(block["value"]))
        assert block["denominator_ok"] is False
        assert block["numerator_ok"] is True


def test_signed_ratio_real_measurement_and_direction_metadata() -> None:
    # molt/baseline direction: 0.5s molt / 0.1s cpython = 5.0 (molt 5x SLOWER).
    block = pa.signed_ratio(0.5, 0.1, direction=pa.RatioDirection.MOLT_OVER_BASELINE)
    assert block["value"] == pytest.approx(5.0)
    assert block["direction"] == pa.RatioDirection.MOLT_OVER_BASELINE.value
    assert block["numerator_ok"] is True
    assert block["denominator_ok"] is True

    # speedup direction: 0.1s cpython / 0.5s molt = 0.2 (molt 5x slower again,
    # but expressed as a speedup < 1) - same physical fact, opposite number,
    # which is EXACTLY why the direction field is mandatory.
    sp = pa.signed_ratio(0.1, 0.5, direction=pa.RatioDirection.SPEEDUP)
    assert sp["value"] == pytest.approx(0.2)
    assert sp["direction"] == pa.RatioDirection.SPEEDUP.value


def test_signed_ratio_every_block_has_full_metadata() -> None:
    block = pa.signed_ratio(2.0, 1.0, direction=pa.RatioDirection.SPEEDUP)
    assert set(block) == {
        "value",
        "direction",
        "numerator_ok",
        "denominator_ok",
    }


def test_signed_ratio_rejects_non_enum_direction() -> None:
    with pytest.raises(TypeError):
        pa.signed_ratio(1.0, 1.0, direction="speedup")  # type: ignore[arg-type]


def test_signed_ratio_value_is_scalar_projection() -> None:
    assert pa.signed_ratio_value(
        2.0, 1.0, direction=pa.RatioDirection.SPEEDUP
    ) == pytest.approx(2.0)
    assert pa.signed_ratio_value(1.0, None, direction=pa.RatioDirection.RATIO) is None
    assert pa.signed_ratio_value(1.0, 0.0, direction=pa.RatioDirection.RATIO) is None


def test_ratio_direction_enum_values_are_self_describing() -> None:
    # Each direction string spells out the numerator/denominator order and which
    # side of 1.0 means the candidate is faster, so a serialized ``direction`` is
    # unambiguous to a reader with no access to this code.
    assert "candidate_time" in pa.RatioDirection.SPEEDUP.value
    assert ">1=candidate_faster" in pa.RatioDirection.SPEEDUP.value
    assert "<1=molt_faster" in pa.RatioDirection.MOLT_OVER_BASELINE.value
    # The three directions are distinct.
    assert (
        len({d.value for d in pa.RatioDirection}) == len(list(pa.RatioDirection)) == 3
    )


def test_perf_authority_reexports_packaged_metric_ratio_authority() -> None:
    # Product CLI code imports molt.metric_ratios; development tools import
    # perf_authority. They must be one implementation, not parallel helpers.
    assert pa.RatioDirection is metric_ratios.RatioDirection
    assert pa.safe_speedup is metric_ratios.safe_speedup
    assert pa.signed_ratio is metric_ratios.signed_ratio
    assert pa.signed_ratio_value is metric_ratios.signed_ratio_value
    assert pa.budget_utilization is metric_ratios.budget_utilization
    assert pa.relative_time_delta is metric_ratios.relative_time_delta


# --- check_ratio_direction drift-gate: fail-closed on a raw time/time --------


def test_drift_gate_flags_synthetic_raw_time_ratio(tmp_path: Path) -> None:
    # The EXACT meta-bug shape: a raw molt_time / cpython_time division in code,
    # outside metric_ratios.py. The gate MUST flag it (proving the gate is not
    # itself a proxy-measurement meta-bug).
    offender = tmp_path / "offender.py"
    offender.write_text(
        "def ratio(molt_time, cpython_time):\n    return molt_time / cpython_time\n",
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert violations, "a raw `<x>_time / <y>_time` division MUST be flagged"
    assert violations[0]["op"] == "/"
    assert "molt" in violations[0]["message"] or "_time" in violations[0]["message"]


def test_drift_gate_flags_time_ratio_via_attribute_and_subscript(
    tmp_path: Path,
) -> None:
    # Attribute (cell.warm_time) and subscript (stats["molt_time"]) operands are
    # also real wall-clock divisions and must be caught.
    offender = tmp_path / "offender2.py"
    offender.write_text(
        "def f(cell, stats):\n"
        "    a = cell.warm_time / cell.cold_time\n"
        '    b = stats["molt_time"] / stats["cpython_time"]\n'
        "    return a, b\n",
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert len(violations) == 2, f"expected 2 violations, got {violations}"


def test_drift_gate_flags_time_s_and_mean_ms_ratio_fields(tmp_path: Path) -> None:
    # The benchmark JSON families use both `*_time_s` and `mean_ms` fields. The
    # gate must cover both, or the DX tools can reintroduce raw timing ratios
    # beside the authority with different suffixes.
    offender = tmp_path / "offender_time_units.py"
    offender.write_text(
        "def f(result, cpython_result, lune_result):\n"
        '    a = result["molt_time_s"] / result["cpython_time_s"]\n'
        '    b = cpython_result["mean_ms"] / lune_result["mean_ms"]\n'
        "    return a, b\n",
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert len(violations) == 2, f"expected 2 violations, got {violations}"


def test_drift_gate_flags_normalized_time_delta(tmp_path: Path) -> None:
    # `(new_time - old_time) / old_time` is still a timing ratio and must route
    # through the same authority; direct-operand-only scans miss this shape.
    offender = tmp_path / "offender_delta.py"
    offender.write_text(
        "def f(result, baseline_time):\n"
        "    return (result.molt_time_s - baseline_time) / baseline_time\n",
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert violations, "a normalized time delta must be flagged"


def test_drift_gate_flags_time_ratio_hidden_in_fstring(tmp_path: Path) -> None:
    # A division hidden inside an f-string EXPRESSION is real code; the AST sees
    # the BinOp even though the audit's sketched line-regex would have MISSED it
    # (it only matched bare `time / time`, not `{time / time}`). This proves the
    # AST gate is strictly stronger than the regex it replaces.
    offender = tmp_path / "offender3.py"
    offender.write_text(
        "def f(molt_time, cpython_time):\n"
        '    return f"{molt_time / cpython_time:.2f}x"\n',
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert violations, "a time/time division inside an f-string MUST be flagged"


def test_drift_gate_ignores_direction_label_strings(tmp_path: Path) -> None:
    # The canonical board documents its direction as the LITERAL string
    # "speedup = cpython_time / molt_time". That is a label, not a computation;
    # a naive line-grep would falsely flag it. The AST gate must NOT.
    benign = tmp_path / "benign.py"
    benign.write_text(
        'DIRECTION = "speedup = cpython_time / molt_time; >1 = molt faster"\n'
        "# molt_cpython_ratio is molt_time / cpython_time (lower = faster)\n"
        "def describe():\n"
        '    """Returns molt_time / cpython_time as documented."""\n'
        "    return DIRECTION\n",
        encoding="utf-8",
    )
    assert crd.scan_file(benign) == []


def test_drift_gate_ignores_non_time_operand_division(tmp_path: Path) -> None:
    # The canonical scoreboard divides `lo_s / molt.median_s`, `numerator /
    # denominator` etc. - operands that are NOT `_time`-suffixed. Those are
    # legitimately guarded by perf_scoreboard's own helpers and must not trip
    # this gate (which targets the bench-lane `_time / _time` bug specifically).
    benign = tmp_path / "scoreboard_like.py"
    benign.write_text(
        "def cell(lo_s, median_s, numerator, denominator):\n"
        "    a = lo_s / median_s\n"
        "    b = numerator / denominator\n"
        "    return a, b\n",
        encoding="utf-8",
    )
    assert crd.scan_file(benign) == []


def test_drift_gate_flags_ms_and_time_s_operand_forms(tmp_path: Path) -> None:
    # The benchmark friend tools compare via `mean_ms` and the harness via
    # `*_time_s`; both are real wall-clock timing fields and must be caught
    # (the operand vocabulary covers `_time`, `_time_s`, `_ms`, mean/median_ms).
    offender = tmp_path / "ms_forms.py"
    offender.write_text(
        "def f(cpython, rust, r):\n"
        '    a = cpython["mean_ms"] / rust["mean_ms"]\n'
        "    b = r.molt_time_s / r.cpython_time_s\n"
        "    return a, b\n",
        encoding="utf-8",
    )
    violations = crd.scan_file(offender)
    assert len(violations) == 2, f"expected 2 violations, got {violations}"


def test_drift_gate_prefilter_is_sound_for_ms_only(tmp_path: Path) -> None:
    # A file whose ONLY timing tokens are `_ms` (no `_time` substring) must NOT
    # be skipped by the cheap text pre-filter - otherwise a real `mean_ms /
    # mean_ms` ratio would slip past unflagged. Pins pre-filter soundness against
    # the broadened operand vocabulary.
    offender = tmp_path / "ms_only.py"
    offender.write_text(
        "def f(a_ms, b_ms):\n    return a_ms / b_ms\n",
        encoding="utf-8",
    )
    assert "_time" not in offender.read_text(encoding="utf-8")
    assert crd.scan_file(offender), "an _ms-only timing ratio must be flagged"


# --- budget_utilization: a SEPARATE domain (zero spend is valid) -------------


def test_budget_utilization_zero_spend_is_zero_not_none() -> None:
    # Compile-budget utilization is NOT a speedup: a function that spent 0ms
    # used 0% of its budget - a VALID 0.0, not the "unmeasurable -> None" that
    # signed_ratio (correctly) returns for a zero TIME. This distinction is why
    # build_diagnostics routes through budget_utilization, not signed_ratio.
    assert pa.budget_utilization(0.0, 5.0) == 0.0
    assert pa.signed_ratio_value(0.0, 5.0, direction=pa.RatioDirection.RATIO) is None


def test_budget_utilization_normal_and_overbudget() -> None:
    assert pa.budget_utilization(2.5, 5.0) == pytest.approx(0.5)
    # Over budget (>100%) is a real, reportable value, not clamped.
    assert pa.budget_utilization(7.5, 5.0) == pytest.approx(1.5)


@pytest.mark.parametrize(
    "spent,budget",
    [
        (1.0, 0.0),  # zero budget -> cannot divide
        (1.0, None),  # missing budget
        (None, 5.0),  # missing spend
        (1.0, -5.0),  # negative budget
        (-1.0, 5.0),  # negative spend (not a valid utilization)
        (float("nan"), 5.0),  # non-finite spend
        (1.0, float("nan")),  # non-finite budget
        (1.0, float("inf")),  # non-finite budget
        ("x", 5.0),  # non-numeric
    ],
)
def test_budget_utilization_degenerate_is_none(spent: object, budget: object) -> None:
    assert (
        pa.budget_utilization(spent, budget)  # type: ignore[arg-type]
        is None
    )


def test_drift_gate_exempts_metric_ratio_authority_itself() -> None:
    # metric_ratios.py is the ONE implementation module permitted to divide one
    # timing-like operand by another. tools/perf_authority.py only re-exports
    # those primitives for tooling; it is not a second implementation authority.
    assert crd.EXEMPT_RELATIVE == {"src/molt/metric_ratios.py"}
    report = crd.run()
    offending_authority = [
        v for v in report["violations"] if v["path"] == "src/molt/metric_ratios.py"
    ]
    assert offending_authority == []


def test_live_tree_has_no_unguarded_time_ratio() -> None:
    # The REAL repo must be clean: every wall-clock ratio routes through
    # molt.metric_ratios.signed_ratio. This is the drift-gate that keeps the
    # bench lanes from silently re-introducing a raw, direction-less
    # `time / time` beside the guarded authority.
    report = crd.run()
    violations = [v["message"] for v in report["violations"]]
    assert violations == [], (
        "unguarded time/time ratio(s) present outside metric_ratios.py:\n"
        + "\n".join(violations)
    )
