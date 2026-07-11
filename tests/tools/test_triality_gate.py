"""Teeth for the A3 triality drift gate (M63 mechanized).

Proves the PURE ``window_drift`` classifier BLOCKS drift and ALLOWS a consistent
window across all three legs (bug-class / perf-claim / finding), that the opt-out
requires a real rationale, that ``evaluate`` is loop-safe (block-once per HEAD),
and that the leg is fail-open under a raising core (via ``stop_gates``).
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks._common as common  # noqa: E402
import tools.hooks.stop_gates as stop_gates  # noqa: E402
import tools.triality_gate as tg  # noqa: E402


# --- pure classifier: BUG-CLASS leg ---------------------------------------


def test_bug_fix_without_net_fires():
    drift = tg.window_drift(
        ["fix(runtime): PyLong_AsLong silent truncation on >2**46 (POISON #7)"],
        ["runtime/molt-lang-cpython-abi/src/numbers.rs"],
    )
    legs = [leg for leg, _ in drift]
    assert "bug-class" in legs


def test_bug_fix_with_test_allows():
    drift = tg.window_drift(
        ["fix(runtime): silent truncation on >2**46"],
        [
            "runtime/molt-lang-cpython-abi/src/numbers.rs",
            "runtime/molt-lang-cpython-abi/tests/test_long_conversions.rs",
        ],
    )
    assert not drift


def test_bug_fix_with_gate_or_registry_allows():
    for net in ("tools/fail_closed_gate.py", "tools/fail_closed_registry.toml"):
        drift = tg.window_drift(
            ["fix: miscompile in loop-IV modulo"],
            ["runtime/src/lowering.rs", net],
        )
        assert not drift, net


def test_bug_fix_with_authority_allows():
    drift = tg.window_drift(
        ["fix: silent wrong-answer in dispatch"],
        ["runtime/src/x.rs", "runtime/molt-ir/src/tir/op_kinds.toml"],
    )
    assert not drift


def test_docs_only_bug_word_passes():
    # A ledger commit that NAMES the class is recording, not fixing -> no fire.
    drift = tg.window_drift(
        ["docs(agent): poison-orphan ledger -- 32 audited findings"],
        ["docs/agent/POISON_ORPHAN_LEDGER.md"],
    )
    assert not drift


# --- pure classifier: PERF-CLAIM leg --------------------------------------


def test_perf_claim_without_bench_fires():
    drift = tg.window_drift(
        ["perf(runtime): int-mul CheckedMul peel -- 1.65x CPython"],
        ["runtime/src/arith.rs"],
    )
    assert [leg for leg, _ in drift] == ["perf-claim"]


def test_perf_claim_with_bench_allows():
    drift = tg.window_drift(
        ["perf(runtime): int-mul peel -- 1.65x CPython"],
        ["runtime/src/arith.rs", "tools/bench_evidence.py"],
    )
    assert not drift


def test_bare_perf_type_without_number_does_not_fire():
    # The commit *type* perf(...) is not a claim; only a stated number is.
    drift = tg.window_drift(
        ["perf(runtime-wasm): route split-runtime APP build through single-compile"],
        ["runtime/src/build.rs"],
    )
    assert not drift


# --- pure classifier: FINDING leg -----------------------------------------


def test_finding_in_code_without_ledger_fires():
    drift = tg.window_drift(
        ["fix: root-cause of the stale module listing was an NTFS mtime cache"],
        ["src/molt/frontend/module_resolution.py"],
    )
    assert [leg for leg, _ in drift] == ["finding"]


def test_finding_with_ledger_allows():
    drift = tg.window_drift(
        ["fix: root-cause of stale module listing (NTFS mtime cache)"],
        [
            "src/molt/frontend/module_resolution.py",
            "docs/agent/REVIEW_FINDINGS_20260708.md",
        ],
    )
    assert not drift


def test_landed_is_not_a_finding_token():
    # "landed" is M12 boilerplate on ~every commit and must NOT trigger the leg.
    drift = tg.window_drift(
        ["feat(runtime): buffer-export lease landed and wired"],
        ["runtime/src/buffer.rs"],
    )
    assert not drift


def test_clean_refactor_passes():
    drift = tg.window_drift(
        ["refactor(runtime): split handle_call_op into 12 helpers"],
        ["runtime/src/function_compiler.rs"],
    )
    assert not drift


# --- opt-out grammar ------------------------------------------------------


def test_opt_out_with_reason_honored():
    r = tg.opt_out_rationale(
        ["fix: silent truncation [no-triality] mechanical revert, net lands next"]
    )
    assert r is not None and "mechanical revert" in r


def test_bare_opt_out_token_not_honored():
    assert tg.opt_out_rationale(["fix: x [no-triality]"]) is None
    assert tg.opt_out_rationale(["fix: x [skip-drift]"]) is None


def test_opt_out_placeholder_reason_not_honored():
    assert tg.opt_out_rationale(["fix: x [no-triality] todo"]) is None


# --- evaluate() end-to-end (loop-safe, block-once) ------------------------


def _seed_marker(root, last_head, last_block_head=None):
    common.state_dir(root)
    common.write_window_marker(
        root,
        tg.MARKER_NAME,
        {"last_head": last_head, "last_block_head": last_block_head},
    )


def _patch(monkeypatch, *, head, subjects, files):
    monkeypatch.setattr(tg._common, "git_head", lambda root: head)
    monkeypatch.setattr(tg._common, "git_window_subjects", lambda root, base: subjects)
    monkeypatch.setattr(tg._common, "git_window_files", lambda root, base: files)


def test_evaluate_first_stop_initializes_never_blocks(tmp_path, monkeypatch):
    _patch(
        monkeypatch,
        head="H0",
        subjects=["fix: poison silent truncation"],
        files=["a.rs"],
    )
    # no marker -> EventWindow initializes to HEAD, never blocks
    assert tg.evaluate({"session_id": "s"}, tmp_path) is None
    marker = common.read_window_marker(tmp_path, tg.MARKER_NAME)
    assert marker["last_head"] == "H0"


def test_evaluate_blocks_drift_then_block_once(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["fix(abi): silent truncation (POISON Lane A #1)"],
        files=["runtime/src/numbers.rs"],
    )
    first = tg.evaluate({"session_id": "s"}, tmp_path)
    assert first is not None and "Triality drift" in first
    # block-once: identical HEAD must not re-block (no wedge)
    second = tg.evaluate({"session_id": "s"}, tmp_path)
    assert second is None


def test_evaluate_allows_when_net_landed(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["fix(abi): silent truncation"],
        files=["runtime/src/numbers.rs", "runtime/tests/test_numbers.rs"],
    )
    assert tg.evaluate({"session_id": "s"}, tmp_path) is None


def test_evaluate_no_new_commits_silent(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "SAME")
    _patch(monkeypatch, head="SAME", subjects=["fix: poison silent"], files=["a.rs"])
    assert tg.evaluate({"session_id": "s"}, tmp_path) is None


def test_evaluate_opt_out_records_waiver(tmp_path, monkeypatch):
    _seed_marker(tmp_path, "BASE")
    _patch(
        monkeypatch,
        head="NEW",
        subjects=["fix: silent truncation [no-triality] revert only, net next turn"],
        files=["runtime/src/numbers.rs"],
    )
    assert tg.evaluate({"session_id": "s"}, tmp_path) is None
    waivers = common.read_jsonl(common.state_dir(tmp_path) / "waivers.jsonl")
    assert any(w.get("gate") == "TRIALITY_GATE" for w in waivers)


# --- fail-open: a raising core must not wedge the session -----------------


def test_stop_gates_fail_open_on_raising_triality(tmp_path, monkeypatch, capsys):
    def boom(data, root):
        raise RuntimeError("synthetic triality core failure")

    monkeypatch.setattr(tg, "evaluate", boom)
    monkeypatch.setattr(stop_gates, "GATES", [("triality_gate", tg.evaluate)])
    monkeypatch.setattr(
        stop_gates._common, "read_hook_input", lambda: {"cwd": str(tmp_path)}
    )
    monkeypatch.setattr(stop_gates._common, "repo_root", lambda cwd=None: tmp_path)
    # run() must return 0 (allow) and emit no block JSON despite the raising leg
    rc = stop_gates.run()
    assert rc == 0
    assert capsys.readouterr().out.strip() == ""


# --- gate-liveness canaries for this gate fire ----------------------------


def test_triality_canaries_live():
    from tools import check_gate_liveness

    results = {f"{c.gate}:{c.name}": ok for c, ok in check_gate_liveness.run()}
    fired = {k: v for k, v in results.items() if k.startswith("triality_gate:")}
    assert fired, "no triality_gate canaries registered"
    assert all(fired.values()), (
        f"dead triality canaries: {[k for k, v in fired.items() if not v]}"
    )


# --- CI self-test mode is falsifiable and currently green -----------------


def test_selftest_check_mode_green():
    assert tg.main(["--check"]) == 0
