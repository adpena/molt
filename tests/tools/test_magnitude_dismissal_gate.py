"""Teeth for the A6 magnitude-dismissal + verdict-scope gate.

Proves the PURE classifiers BLOCK an absolute-magnitude dismissal and an unscoped
negative verdict, ALLOW them WITH relative-significance math / a measurement / a
waiver / a scope declaration, that markdown-table rows are not a false-positive
source, that ``evaluate`` is loop-safe, and that the leg is fail-open.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks._common as common  # noqa: E402
import tools.hooks.stop_gates as stop_gates  # noqa: E402
import tools.magnitude_dismissal_gate as mg  # noqa: E402


# --- magnitude-dismissal: the pure classifier -----------------------------


def test_absolute_dismissal_without_math_fires():
    msgs = mg.classify_window(
        ["the erasure lane gives a weak delta; not worth chasing, deferring it"], {}
    )
    assert any("magnitude-based dismissal" in m for m in msgs)


def test_dismissal_with_relative_significance_allows():
    msgs = mg.classify_window(
        [
            "the erasure delta 0.012 is 18% of the remaining gap to green, so we keep "
            "it (relative significance, not absolute)"
        ],
        {},
    )
    assert not msgs


def test_dismissal_with_inline_waiver_allows():
    msgs = mg.classify_window(
        [
            "deferring the tiny scalar-carrier win  # MAGNITUDE_DISMISSAL_OK: the "
            "loop-unbox lane subsumes it, measured overlap"
        ],
        {},
    )
    assert not msgs


def test_dismissal_with_placeholder_waiver_still_fires():
    msgs = mg.classify_window(
        ["deferring the tiny win because it is weak  # MAGNITUDE_DISMISSAL_OK: todo"],
        {},
    )
    assert any("magnitude-based dismissal" in m for m in msgs)


def test_dismissal_with_measured_unrecoverability_allows():
    msgs = mg.classify_window(
        [
            "the label-noise residual is at the measured noise floor (exit criterion "
            "hit), so this lane is negligible and we stop"
        ],
        {},
    )
    assert not msgs


def test_window_opt_out_token_detected():
    assert mg.mag_is_opted_out(["chore: shelve the weak lane [magnitude-ok]"]) is True
    assert mg.mag_is_opted_out(["chore: shelve the weak lane"]) is False


def test_markdown_table_rows_are_not_a_false_positive():
    # A dense ledger table straddled by the 3-line window must NOT trip magnitude.
    table = [
        "| `PyType_FromModuleAndSpec` | DIVERGENT | trivial shim | skip for now |",
        "| `PyType_Ready` | PARTIAL | minimal | deferred |",
        "| `PyDict_New` | FAITHFUL | small wrapper | not worth inlining |",
    ]
    msgs = mg.classify_window([], {"docs/agent/CPYTHON_ABI_COVERAGE_MATRIX.md": table})
    assert not msgs


# --- verdict-scope: the pure classifier -----------------------------------


def test_kill_without_scope_fires():
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md", ["Lever-D is FALSIFIED, dropping it"]
    )
    assert msgs and "verdict_scope" in msgs[0]


def test_verdict_with_instance_scope_allows():
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md",
        ["Lever-D FALSIFIED", "verdict_scope: instance -- only the toy formulation"],
    )
    assert not msgs


def test_inline_scope_declaration_allows():
    # verdict token + scope on the SAME line (inline style) must be recognized.
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md",
        ["Lever-D FALSIFIED  verdict_scope: instance -- only the toy formulation"],
    )
    assert not msgs


def test_family_kill_without_evidence_fires():
    msgs = mg.verdict_scope_violations(
        "docs/agent/REVIEW_FINDINGS_20260708.md",
        ["the whole approach is DEAD", "verdict_scope: family"],
    )
    assert msgs and "family" in msgs[0]


def test_family_kill_with_citation_allows():
    msgs = mg.verdict_scope_violations(
        "docs/agent/REVIEW_FINDINGS_20260708.md",
        [
            "the whole approach is DEAD across two distinct formulations",
            "verdict_scope: family -- see arxiv:2401.00001, impossibility bound",
        ],
    )
    assert not msgs


def test_kill_at_formulation_without_reformulation_fires():
    msgs = mg.verdict_scope_violations(
        "docs/agent/PANIC_REACHABILITY_LEDGER.md",
        ["this path is NO-GO", "verdict_scope: formulation"],
    )
    assert msgs and "reformulation" in msgs[0]


def test_kill_at_formulation_with_reformulation_allows():
    msgs = mg.verdict_scope_violations(
        "docs/agent/PANIC_REACHABILITY_LEDGER.md",
        [
            "this path is NO-GO",
            "verdict_scope: formulation",
            "untested formulations / alternatives: the arena-based lowering, the CPS form",
        ],
    )
    assert not msgs


def test_wontfix_is_a_negative_verdict():
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md", ["finding #7 is WONTFIX, moving on"]
    )
    assert msgs


def test_scope_waiver_allows():
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md",
        ["Lever-D FALSIFIED  # VERDICT_SCOPE_OK: superseded by the rewritten lane"],
    )
    assert not msgs


def test_lowercase_prose_is_not_a_verdict():
    # lowercase "killed the process"/"dead code" is prose, not a verdict token.
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md",
        ["the fix killed the flaky deadlock; removed the dead code path"],
    )
    assert not msgs


def test_quoted_prior_verdict_stays_silent():
    msgs = mg.verdict_scope_violations(
        "docs/agent/POISON_ORPHAN_LEDGER.md",
        ['we reopened the lane that was previously marked "FALSIFIED"'],
    )
    assert not msgs


# --- doc scope: exempt reference docs are not scanned ---------------------


def test_exempt_reference_doc_not_scanned():
    assert mg.verdict_doc_in_scope("docs/agent/POISON_ORPHAN_LEDGER.md") is True
    for exempt in (
        "docs/agent/APPARATUS_FROM_COMMA_LAB.md",
        "docs/agent/CPYTHON_ABI_COVERAGE_MATRIX.md",
        "docs/agent/ORCHESTRATION.md",
    ):
        assert mg.verdict_doc_in_scope(exempt) is False, exempt
    # and classify_window ignores verdicts in an exempt doc
    assert not mg.classify_window(
        [], {"docs/agent/APPARATUS_FROM_COMMA_LAB.md": ["X is FALSIFIED"]}
    )


# --- evaluate() end-to-end (loop-safe, block-once) ------------------------


def _seed_marker(root, last_head, last_block_head=None):
    common.state_dir(root)
    common.write_window_marker(
        root,
        mg.MARKER_NAME,
        {"last_head": last_head, "last_block_head": last_block_head},
    )


def _patch(monkeypatch, *, head, subjects, files, diff=""):
    monkeypatch.setattr(mg._common, "git_head", lambda root: head)
    monkeypatch.setattr(mg._common, "git_window_subjects", lambda root, base: subjects)
    monkeypatch.setattr(mg._common, "git_window_files", lambda root, base: files)
    monkeypatch.setattr(
        mg._common, "git_window_diff", lambda root, base, path=None: diff
    )


def test_evaluate_blocks_unscoped_kill_then_block_once(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["docs(agent): poison ledger update"],
        files=["docs/agent/POISON_ORPHAN_LEDGER.md"],
        diff="+++ b/docs/agent/POISON_ORPHAN_LEDGER.md\n+Lever-D is FALSIFIED, dropping it\n",
    )
    first = mg.evaluate({"session_id": "s"}, tmp_path)
    assert first is not None and "Verdict-scope" in first
    second = mg.evaluate({"session_id": "s"}, tmp_path)
    assert second is None  # block-once


def test_evaluate_allows_scoped_verdict(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["docs(agent): poison ledger update"],
        files=["docs/agent/POISON_ORPHAN_LEDGER.md"],
        diff=(
            "+++ b/docs/agent/POISON_ORPHAN_LEDGER.md\n"
            "+Lever-D FALSIFIED\n+verdict_scope: instance -- toy formulation only\n"
        ),
    )
    assert mg.evaluate({"session_id": "s"}, tmp_path) is None


def test_evaluate_blocks_magnitude_dismissal_in_subject(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["chore: deferring the weak scalar-carrier lane, not worth chasing"],
        files=["runtime/src/x.rs"],
    )
    r = mg.evaluate({"session_id": "s"}, tmp_path)
    assert r is not None and "Magnitude-dismissal" in r


# --- fail-open ------------------------------------------------------------


def test_stop_gates_fail_open_on_raising_magnitude(tmp_path, monkeypatch, capsys):
    def boom(data, root):
        raise RuntimeError("synthetic magnitude core failure")

    monkeypatch.setattr(stop_gates, "GATES", [("magnitude_dismissal_gate", boom)])
    monkeypatch.setattr(
        stop_gates._common, "read_hook_input", lambda: {"cwd": str(tmp_path)}
    )
    monkeypatch.setattr(stop_gates._common, "repo_root", lambda cwd=None: tmp_path)
    rc = stop_gates.run()
    assert rc == 0
    assert capsys.readouterr().out.strip() == ""


# --- gate-liveness canaries for this gate fire ----------------------------


def test_magnitude_canaries_live():
    from tools import check_gate_liveness

    results = {f"{c.gate}:{c.name}": ok for c, ok in check_gate_liveness.run()}
    fired = {
        k: v for k, v in results.items() if k.startswith("magnitude_dismissal_gate:")
    }
    assert fired, "no magnitude_dismissal_gate canaries registered"
    assert all(fired.values()), (
        f"dead canaries: {[k for k, v in fired.items() if not v]}"
    )


def test_selftest_check_mode_green():
    assert mg.main(["--check"]) == 0
