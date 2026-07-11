"""Teeth for the A2 land-or-blocker Stop gate (M12 mechanized).

Proves the pure ``decide()`` blocks a no-land turn and allows landed / queued /
blocker / report-only / non-substantive turns; and that ``evaluate()`` sets a
per-session baseline, blocks once, and never re-blocks the same HEAD (no wedge).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks._common as common  # noqa: E402
import tools.hooks.landing_gate as lg  # noqa: E402


# --- pure decide() --------------------------------------------------------


def test_decide_blocks_report_without_landing():
    reason = lg.decide(
        substantive_activity=True,
        window_has_commit=False,
        queue_row_in_flight=False,
        blocker_recorded=False,
        report_only=False,
    )
    assert reason is not None and "without landing" in reason


def test_decide_allows_non_substantive_turn():
    assert (
        lg.decide(
            substantive_activity=False,
            window_has_commit=False,
            queue_row_in_flight=False,
            blocker_recorded=False,
            report_only=False,
        )
        is None
    )


def test_decide_allows_each_satisfying_signal():
    base = dict(
        substantive_activity=True,
        window_has_commit=False,
        queue_row_in_flight=False,
        blocker_recorded=False,
        report_only=False,
    )
    for key in (
        "window_has_commit",
        "queue_row_in_flight",
        "blocker_recorded",
        "report_only",
    ):
        kw = dict(base)
        kw[key] = True
        assert lg.decide(**kw) is None, key


# --- blocker ledger -------------------------------------------------------


def test_record_and_detect_blocker(tmp_path):
    lg.record_blocker(tmp_path, "upstream numpy seal regen blocked on meson", "sess-9")
    assert lg._blocker_recorded(tmp_path, "sess-9", 0.0) is True
    # Detected by recency too, even without a session id match.
    assert lg._blocker_recorded(tmp_path, "other", 0.0) is True


# --- evaluate() end-to-end ------------------------------------------------


def _seed_marker(root, session_id, start_head, last_block_head=None):
    common.state_dir(root)
    (root / ".molt" / "state" / lg.MARKER_NAME).write_text(
        json.dumps(
            {
                "session_id": session_id,
                "start_head": start_head,
                "start_ts": 0.0,
                "last_block_head": last_block_head,
            }
        ),
        encoding="utf-8",
    )


def _patch_facts(
    monkeypatch, *, tool_uses, head, has_commit, queue, blocker, subjects=""
):
    monkeypatch.setattr(lg, "_count_tool_uses", lambda p: tool_uses)
    monkeypatch.setattr(lg._common, "git_head", lambda root: head)
    monkeypatch.setattr(lg, "_window_has_commit", lambda root, sh: has_commit)
    monkeypatch.setattr(lg, "_queue_row_in_flight", lambda root: queue)
    monkeypatch.setattr(lg, "_blocker_recorded", lambda root, sid, ts: blocker)
    monkeypatch.setattr(lg, "_window_subjects", lambda root, sh: subjects)


def test_evaluate_first_stop_sets_baseline_never_blocks(tmp_path, monkeypatch):
    monkeypatch.setattr(lg._common, "git_head", lambda root: "BASE")
    data = {"session_id": "s1", "transcript_path": None}
    assert lg.evaluate(data, tmp_path) is None
    marker = json.loads((tmp_path / ".molt" / "state" / lg.MARKER_NAME).read_text())
    assert marker["session_id"] == "s1" and marker["start_head"] == "BASE"


def test_evaluate_blocks_no_land_turn_then_block_once(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "s1", "BASE")
    _patch_facts(
        monkeypatch,
        tool_uses=5,
        head="BASE",
        has_commit=False,
        queue=False,
        blocker=False,
    )
    data = {"session_id": "s1", "transcript_path": "x"}
    first = lg.evaluate(data, tmp_path)
    assert first is not None and "without landing" in first
    # block-once: identical HEAD state must NOT re-block (no wedge / no loop).
    second = lg.evaluate(data, tmp_path)
    assert second is None


def test_evaluate_allows_when_commit_landed(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "s1", "BASE")
    _patch_facts(
        monkeypatch,
        tool_uses=5,
        head="NEW",
        has_commit=True,
        queue=False,
        blocker=False,
    )
    data = {"session_id": "s1", "transcript_path": "x"}
    assert lg.evaluate(data, tmp_path) is None


def test_evaluate_allows_report_only_token(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "s1", "BASE")
    _patch_facts(
        monkeypatch,
        tool_uses=5,
        head="NEW",
        has_commit=True,
        queue=False,
        blocker=False,
        subjects="docs: summary [report-only]\n",
    )
    data = {"session_id": "s1", "transcript_path": "x"}
    assert lg.evaluate(data, tmp_path) is None
