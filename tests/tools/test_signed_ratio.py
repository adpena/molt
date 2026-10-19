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

import ast
import math
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import perf_authority as pa  # noqa: E402
from tools import check_ratio_direction as crd  # noqa: E402
from molt import metric_ratios  # noqa: E402
from tests.process_guard_common import ModuleView, run_guarded_test_process  # noqa: E402
from tools import git_identity  # noqa: E402
from tools.git_identity import repository_source_paths  # noqa: E402


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


@pytest.mark.parametrize(
    "expression,expected_ops",
    [
        ("a_ｔｉｍｅ / b_ｔｉｍｅ", ["/"]),
        ("row.a_ｍｓ * row.b_ｍｓ", ["*"]),
        (r'stats["left\x5ftime"] / stats["right\u005ftime"]', ["/"]),
        ('stats["left_ti" "me"] / stats["right_" "time"]', ["/"]),
        ('stats["left_ti"\n # joined key\n "me"] / stats["right_" "time"]', ["/"]),
        (
            "f\"{stats['left_ti' 'me'] / stats['right_' 'time']:"
            '{a_ｔｉｍｅ / b_ｔｉｍｅ}}"',
            ["/", "/"],
        ),
    ],
)
def test_drift_gate_uses_parsed_timing_names(
    tmp_path: Path, expression: str, expected_ops: list[str]
) -> None:
    # Each is real executable ratio syntax with fewer than two raw suffixes.
    # Python owns NFKC identifier normalization and constant key decoding/joining.
    assert expression.count("_time") + expression.count("_ms") < 2
    offender = tmp_path / "decoded_names.py"
    offender.write_text(f"value = {expression}\n", encoding="utf-8")
    assert [row["op"] for row in crd.scan_file(offender)] == expected_ops


@pytest.mark.parametrize(
    "operand,is_timing",
    [
        ("left_time", True),
        ("row.left_time", True),
        ('stats["left_time"]', True),
        ("left_time + 1", True),
        ("left_time ** 0", True),
        ("-left_time", True),
        ("left_time if condition else 0", True),
        ("0 if condition else left_time", True),
        ("value := left_time", True),
        ("read(left_time)", True),
        ("read(value=left_time)", True),
        ("read(**left_time)", True),
        ("row[left_time]", False),
        ("left_time.other", False),
        ("left_time[0]", False),
        ("left_time()", False),
        ("read(left_time=0)", False),
        ("read(*left_time)", False),
        ("0 if left_time else 1", False),
        ("left_time := 0", False),
        ("'left_time'", False),
        ("(left_time,)", False),
        ("[left_time]", False),
        ("left_time and True", False),
        ("left_time < 1", False),
        ("f'{left_time}'", False),
        ("lambda: left_time", False),
    ],
)
def test_drift_gate_preserves_value_operand_boundaries(
    tmp_path: Path, operand: str, is_timing: bool
) -> None:
    source = tmp_path / "operand_boundaries.py"
    source.write_text(f"value = ({operand}) / right_time\n", encoding="utf-8")
    assert [row["op"] for row in crd.scan_file(source)] == (["/"] if is_timing else [])


@pytest.mark.parametrize(
    "owner",
    [
        "(left_time / right_time)()",
        "(left_time / right_time).other",
        "row[left_time / right_time]",
        "f'{left_time / right_time}'",
        "f\"{f'{left_time / right_time}'}\"",
        "((left_time / right_time) and 1)",
    ],
)
def test_drift_gate_traverses_contexts_that_do_not_propagate_operands(
    tmp_path: Path, owner: str
) -> None:
    source = tmp_path / "nested_context.py"
    source.write_text(f"value = ({owner}) / outer_time\n", encoding="utf-8")
    # The nested computation is still visited; its enclosing opaque value does
    # not become a timing operand merely because the syntax contains that ratio.
    assert [(row["line"], row["op"]) for row in crd.scan_file(source)] == [(1, "/")]


def test_drift_gate_retains_syntax_preorder_rather_than_line_order(
    tmp_path: Path,
) -> None:
    source = tmp_path / "preorder.py"
    source.write_text(
        "value = (\n"
        "    (left_time / other_time)\n"
        "    if (test_time * another_time)\n"
        "    else fallback_time\n"
        ") / right_time\n",
        encoding="utf-8",
    )
    assert [(row["line"], row["op"]) for row in crd.scan_file(source)] == [
        (1, "/"),
        (3, "*"),
        (2, "/"),
    ]


@pytest.mark.parametrize(
    "shape,size",
    [
        ("left", 128),
        ("left", 2048),
        ("right", 128),
        ("right", 2048),
        ("balanced", 7),
        ("balanced", 11),
    ],
)
def test_drift_gate_operand_work_is_linear_and_stack_safe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, shape: str, size: int
) -> None:
    def leaf() -> ast.Name:
        return ast.Name(id="sample_time", ctx=ast.Load())

    def balanced(depth: int, index: int) -> tuple[ast.expr, list[tuple[int, str]]]:
        if not depth:
            return leaf(), []
        left, left_hits = balanced(depth - 1, index * 2)
        right, right_hits = balanced(depth - 1, index * 2 + 1)
        node = ast.BinOp(left=left, op=ast.Mult(), right=right, lineno=index)
        return node, [(index, "*"), *left_hits, *right_hits]

    if shape == "balanced":
        expression, expected = balanced(size, 1)
        count = 2**size - 1
    else:
        count = size
        expression = leaf()
        for line in range(1, count + 1):
            left, right = (
                (expression, leaf()) if shape == "left" else (leaf(), expression)
            )
            expression = ast.BinOp(left=left, op=ast.Div(), right=right, lineno=line)
        expected = [(line, "/") for line in range(count, 0, -1)]
    tree = ast.Expression(body=expression)
    visits: dict[ast.expr, int] = {}
    original = crd._is_time_operand

    def counted(node: ast.expr) -> bool:
        visits[node] = visits.get(node, 0) + 1
        return original(node)

    monkeypatch.setattr(crd, "_is_time_operand", counted)
    # The parser has its own nesting limits. Supply an otherwise ordinary AST
    # to isolate the checker's traversal, without raising Python's stack limit.
    monkeypatch.setattr(crd, "ast", ModuleView(ast, parse=lambda *args, **kwargs: tree))
    source = tmp_path / "long_arithmetic.py"
    source.write_text("value = left_time / right_time\n", encoding="utf-8")
    assert [(row["line"], row["op"]) for row in crd.scan_file(source)] == expected
    assert len(visits) == 2 * count + 1
    assert set(visits.values()) == {1}


def test_ratio_gate_does_not_resolve_clean_sources(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "unit_conversion.py"
    source.write_text("value = sample_time / 1000\n", encoding="utf-8")

    def forbidden_resolution(_path: Path) -> str:
        pytest.fail("a clean file needs no physical diagnostic path")

    monkeypatch.setattr(crd, "_rel", forbidden_resolution)
    assert crd.scan_file(source) == []


def test_ratio_gate_preserves_authority_exemption_before_reading(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(crd, "REPO_ROOT", tmp_path)
    authority = tmp_path / "src/molt/metric_ratios.py"
    original_read = Path.read_text

    def forbidden_read(path: Path, *args: object, **kwargs: object) -> str:
        if path == authority:
            pytest.fail("the canonical authority is exempt even when unreadable")
        return original_read(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", forbidden_read)
    assert crd.scan_file(authority) == []


def test_ratio_gate_preserves_resolved_exemption_for_hits_and_read_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias = tmp_path / "alias.py"
    alias.write_text("value = left_time / right_time\n", encoding="utf-8")

    def resolved_authority(path: Path) -> str:
        assert path == alias
        return "src/molt/metric_ratios.py"

    monkeypatch.setattr(crd, "_rel", resolved_authority)
    assert crd.scan_file(alias) == []
    original_read = Path.read_text

    def unreadable(path: Path, *args: object, **kwargs: object) -> str:
        if path == alias:
            raise PermissionError("unreadable authority alias")
        return original_read(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", unreadable)
    assert crd.scan_file(alias) == []


@pytest.mark.parametrize("link_kind", ["canonical_file", "canonical_parent", "alias"])
def test_ratio_gate_exemption_follows_physical_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, link_kind: str
) -> None:
    monkeypatch.setattr(crd, "REPO_ROOT", tmp_path)
    canonical = tmp_path / "src/molt/metric_ratios.py"
    if link_kind == "canonical_parent":
        target = tmp_path / "ordinary-src/molt/metric_ratios.py"
        target.parent.mkdir(parents=True)
        link, referent = tmp_path / "src", target.parent.parent
        source = canonical
        expected = "ordinary-src/molt/metric_ratios.py"
    elif link_kind == "canonical_file":
        target = tmp_path / "ordinary.py"
        canonical.parent.mkdir(parents=True)
        link, referent = canonical, target
        source = canonical
        expected = "ordinary.py"
    else:
        target = canonical
        target.parent.mkdir(parents=True)
        link, referent = tmp_path / "alias.py", canonical
        source = link
        expected = None
    target.write_text("value = left_time / right_time\n", encoding="utf-8")
    try:
        link.symlink_to(referent, target_is_directory=link_kind == "canonical_parent")
    except OSError as exc:
        pytest.skip(f"host cannot create source symlinks: {exc}")
    assert [(row["path"], row["op"]) for row in crd.scan_file(source)] == (
        [] if expected is None else [(expected, "/")]
    )


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
    assert crd.scan_file(REPO_ROOT / "src/molt/metric_ratios.py") == []


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


def _git(root: Path, *arguments: str) -> None:
    run_guarded_test_process(["git", *arguments], cwd=root, check=True, timeout=30)


def _ratio_repository(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    # The final whitespace is a literal path component, not Git's newline.
    root = tmp_path / (
        "source repository with spaces "
        if os.name != "nt"
        else "source repository with spaces"
    )
    root.mkdir()
    root = root.resolve()
    _git(root, "init", "--quiet")
    monkeypatch.setattr(crd, "REPO_ROOT", root)
    return root


def test_git_inventory_checks_admitted_source_and_excludes_operational_trees(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _ratio_repository(tmp_path, monkeypatch)
    (root / ".gitignore").write_text(
        "logs/\ngenerated/\nthird_party/\n", encoding="utf-8"
    )
    tracked = root / "tracked source.py"
    tracked.write_text("answer = first_time / second_time\n", encoding="utf-8")
    _git(root, "add", ".gitignore", tracked.name)
    _git(
        root,
        "-c",
        "user.name=Ratio Test",
        "-c",
        "user.email=ratio@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "source inventory fixture",
    )
    # Git, rather than a spelling blacklist, excludes this real nested worktree.
    nested = root / "nested.py"
    _git(root, "worktree", "add", "--quiet", "--detach", str(nested), "HEAD")
    (nested / "nested violation.py").write_text(
        "x = left_time / right_time\n", encoding="utf-8"
    )
    untracked = root / (
        "untracked\nsource with spaces.py"
        if os.name != "nt"
        else "untracked source with spaces.py"
    )
    untracked.write_text("answer = first_time / second_time\n", encoding="utf-8")
    for directory in ("logs", "generated", "third_party"):
        excluded = root / directory
        excluded.mkdir()
        (excluded / "operation_probe.py").write_text(
            "x = left_time / right_time\n", encoding="utf-8"
        )
        vanished = excluded / "vanished.py"
        vanished.write_text("x = left_time / right_time\n", encoding="utf-8")
        vanished.unlink()
    forced = root / "logs" / "tracked source.py"
    forced.write_text("x = left_time / right_time\n", encoding="utf-8")
    _git(root, "add", "--force", "logs/tracked source.py")

    assert crd._iter_py_files() == sorted(
        [tracked, untracked, forced], key=lambda path: path.as_posix()
    )
    report = crd.run()
    assert report["files_scanned"] == 3
    assert {row["path"] for row in report["violations"]} == {
        tracked.name,
        untracked.name,
        "logs/tracked source.py",
    }


def test_git_inventory_retains_missing_tracked_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _ratio_repository(tmp_path, monkeypatch)
    source = root / "missing.py"
    source.write_text("x = left_time / right_time\n", encoding="utf-8")
    _git(root, "add", source.name)
    source.unlink()
    assert crd._iter_py_files() == [source]
    with pytest.raises(FileNotFoundError):
        crd.run()


def test_git_inventory_failure_cannot_pass_the_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(crd, "REPO_ROOT", tmp_path)
    with pytest.raises(subprocess.CalledProcessError):
        crd.run()


@pytest.mark.skipif(os.name == "nt", reason="Windows reserves backslash separators")
def test_literal_backslash_source_cannot_impersonate_ratio_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _ratio_repository(tmp_path, monkeypatch)
    source = root / r"src\molt\metric_ratios.py"
    source.write_text("x = left_time / right_time\n", encoding="utf-8")
    _git(root, "add", source.name)
    report = crd.run()
    assert [row["path"] for row in report["violations"]] == [source.name]


@pytest.mark.parametrize(
    "inventory,root_suffix,message",
    [
        (b"source.py", b"\n", "NUL terminated"),
        (b"../escape.py\0", b"\n", "escaped its root"),
        (b"source.py\0", b"", "newline terminated"),
    ],
)
def test_git_inventory_rejects_malformed_or_unconfined_results(
    tmp_path: Path, inventory: bytes, root_suffix: bytes, message: str
) -> None:
    def read_git(cwd: Path, arguments: list[str]) -> bytes:
        assert cwd == tmp_path
        return (
            inventory
            if arguments[0] == "ls-files"
            else os.fsencode(tmp_path) + root_suffix
        )

    with pytest.raises(ValueError, match=message):
        repository_source_paths(tmp_path, run_git=read_git, include_untracked=True)


def test_git_source_root_accepts_windows_newline_protocol(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(git_identity, "os", ModuleView(os, name="nt"))

    def read_git(cwd: Path, arguments: list[str]) -> bytes:
        return (
            b"source with spaces.py\0"
            if arguments[0] == "ls-files"
            else os.fsencode(tmp_path) + b"\r\n"
        )

    assert repository_source_paths(tmp_path, run_git=read_git) == [
        tmp_path / "source with spaces.py"
    ]


def test_ratio_gate_by_path_binds_its_source_root_from_foreign_cwd(
    tmp_path: Path,
) -> None:
    foreign = tmp_path / "unrelated working directory with spaces"
    foreign.mkdir()
    result = run_guarded_test_process(
        [sys.executable, str(REPO_ROOT / "tools/check_ratio_direction.py"), "--json"],
        cwd=foreign,
        check=True,
        timeout=60,
    )
    report = json.loads(result.stdout)
    assert report["violations"] == []
    assert report["files_scanned"] > 0
