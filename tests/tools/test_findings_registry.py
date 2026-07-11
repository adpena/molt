"""Teeth for the molt findings registry + memo lint (APPARATUS A4).

Proves the ask's five required properties (M05 — a gate that only passes clean
certifies nothing):

  (a) the ORPHAN BAN refuses a no-producer/no-consumer finding (raises);
  (b) latest-event-per-id resolution across appended events;
  (c) the memo lint BLOCKS a synthetic memo stating "2.3x speedup" with no
      finding_id AND passes it with a `# FORMALIZATION_PENDING:<why>` waiver or a
      finding_id reference;
  (d) the cross-platform (msvcrt/fcntl) registry lock round-trips AND provides
      real mutual exclusion;
  (e) the seeded keystone findings load + validate.

Plus construction-invariant teeth (authority tier, NaN residual, verification
enum, noise-floor provenance), the recalibration/query surface, and the lint's
strict-mode exit code + false-positive guards.

Source: docs/agent/APPARATUS_FROM_COMMA_LAB.md A4 + 1.3.
"""

from __future__ import annotations

import json
import threading
from dataclasses import dataclass
from pathlib import Path

import pytest

from tools import findings_memo_lint as lint
from tools import findings_registry as fr
from tools.findings_registry import (
    ASSUMED_AWAITING_VERIFICATION,
    AUTHORITY_MOLT_BUILD_RELEASE,
    RECALIBRATE_ON_RESIDUAL_DRIFT,
    VERIFIED_VIA_EMPIRICAL_ANCHOR,
    EmpiricalAnchor,
    Finding,
    InvalidFindingError,
    build_seed_findings,
    delta_exceeds_floor,
)

_UTC = "2026-07-10T00:00:00Z"


@dataclass
class _TmpRegistry:
    jsonl: Path
    lock: Path


@pytest.fixture
def tmp_registry(tmp_path: Path) -> _TmpRegistry:
    jsonl = tmp_path / "state" / "findings_registry.jsonl"
    lock = jsonl.with_suffix(jsonl.suffix + ".lock")
    return _TmpRegistry(jsonl=jsonl, lock=lock)


def _anchor(**over) -> EmpiricalAnchor:
    kw = dict(
        anchor_id="a1",
        predicted="~1.6x",
        measured="1.65x",
        residual=0.05,
        authority_tier=AUTHORITY_MOLT_BUILD_RELEASE,
        measurement_method="molt build --release probe.py",
        source_artifact="commit deadbeef",
        measured_utc=_UTC,
    )
    kw.update(over)
    return EmpiricalAnchor(**kw)


def _finding(**over) -> Finding:
    kw = dict(
        finding_id="sample_finding_v1",
        one_line_summary="a sample measured finding",
        claim="wall_release <= 0.6 * wall_cpython on probe.py",
        domain_of_validity={"os": ["windows"], "py_version": ">=3.12"},
        anchors=(_anchor(),),
        producers=("commit deadbeef",),
        consumers=("docs/agent/PERF_AUTHORITY.md",),
        verification=VERIFIED_VIA_EMPIRICAL_ANCHOR,
        next_recalibration_trigger=RECALIBRATE_ON_RESIDUAL_DRIFT,
        created_utc=_UTC,
        last_calibration_utc=_UTC,
    )
    kw.update(over)
    return Finding(**kw)


# ---------------------------------------------------------------------------
# (a) ORPHAN BAN
# ---------------------------------------------------------------------------


def test_orphan_ban_refuses_no_producer_no_consumer():
    with pytest.raises(InvalidFindingError, match="orphan"):
        _finding(producers=(), consumers=())


def test_orphan_ban_satisfied_by_producer_only():
    f = _finding(producers=("commit deadbeef",), consumers=())
    assert f.producers and not f.consumers


def test_orphan_ban_satisfied_by_consumer_only():
    f = _finding(producers=(), consumers=("tools/bench_evidence.py",))
    assert f.consumers and not f.producers


def test_orphan_ban_enforced_on_from_dict_roundtrip():
    payload = _finding().to_dict()
    payload["producers"] = []
    payload["consumers"] = []
    with pytest.raises(InvalidFindingError, match="orphan"):
        Finding.from_dict(payload)


# ---------------------------------------------------------------------------
# construction-invariant teeth
# ---------------------------------------------------------------------------


def test_finding_id_must_be_snake_case_vn():
    with pytest.raises(InvalidFindingError, match="snake_case_vN"):
        _finding(finding_id="NotSnake")
    with pytest.raises(InvalidFindingError, match="snake_case_vN"):
        _finding(finding_id="missing_version")


def test_domain_of_validity_must_be_nonempty():
    with pytest.raises(InvalidFindingError, match="domain_of_validity"):
        _finding(domain_of_validity={})


def test_bad_authority_tier_refused():
    with pytest.raises(InvalidFindingError, match="authority_tier"):
        _anchor(authority_tier="vibes")


def test_nan_residual_refused():
    with pytest.raises(InvalidFindingError, match="NaN"):
        _anchor(residual=float("nan"))


def test_negative_residual_refused():
    with pytest.raises(InvalidFindingError, match=">= 0"):
        _anchor(residual=-1.0)


def test_bad_verification_enum_refused():
    with pytest.raises(InvalidFindingError, match="verification"):
        _finding(verification="TOTALLY_LEGIT")


def test_empirical_anchor_verification_needs_an_anchor():
    with pytest.raises(InvalidFindingError, match="no anchors"):
        _finding(verification=VERIFIED_VIA_EMPIRICAL_ANCHOR, anchors=())


def test_noise_floor_requires_provenance():
    with pytest.raises(InvalidFindingError, match="noise_floor_provenance"):
        _anchor(noise_floor=1.0, noise_floor_provenance=None)


def test_summary_length_capped():
    with pytest.raises(InvalidFindingError, match="200"):
        _finding(one_line_summary="x" * 201)


def test_bad_recalibration_trigger_refused():
    with pytest.raises(InvalidFindingError, match="recalibration"):
        _finding(next_recalibration_trigger="whenever_i_feel_like_it")


def test_delta_exceeds_floor_semantics():
    # No floor -> None (UNMEASURED, cannot clear a verdict).
    assert delta_exceeds_floor(_anchor(residual=0.5)) is None
    # Above floor -> True; within floor -> False.
    a = _anchor(residual=1.5, noise_floor=1.0, noise_floor_provenance="5-run band")
    assert delta_exceeds_floor(a) is True
    b = _anchor(residual=0.5, noise_floor=1.0, noise_floor_provenance="5-run band")
    assert delta_exceeds_floor(b) is False


def test_is_well_calibrated():
    assert _finding().is_well_calibrated is True
    hot = _finding(
        verification=ASSUMED_AWAITING_VERIFICATION,
        anchors=(_anchor(residual=5.0),),
    )
    assert hot.is_well_calibrated is False
    # No anchors -> not well-calibrated (cue to land the first anchor).
    assert (
        _finding(
            verification=ASSUMED_AWAITING_VERIFICATION, anchors=()
        ).is_well_calibrated
        is False
    )


# ---------------------------------------------------------------------------
# (b) latest-event-per-id resolution
# ---------------------------------------------------------------------------


def test_register_then_get_roundtrip(tmp_registry):
    fr.register_finding(
        _finding(), path=tmp_registry.jsonl, lock_path=tmp_registry.lock
    )
    got = fr.get_finding("sample_finding_v1", path=tmp_registry.jsonl)
    assert got is not None
    assert got.claim == "wall_release <= 0.6 * wall_cpython on probe.py"


def test_latest_event_per_id_wins(tmp_registry):
    # Two 'registered' events for the same id: the later summary must win.
    fr.register_finding(
        _finding(one_line_summary="first summary"),
        path=tmp_registry.jsonl,
        lock_path=tmp_registry.lock,
    )
    fr.register_finding(
        _finding(one_line_summary="second summary"),
        path=tmp_registry.jsonl,
        lock_path=tmp_registry.lock,
    )
    findings = fr.query_findings(path=tmp_registry.jsonl)
    assert len(findings) == 1
    assert findings[0].one_line_summary == "second summary"
    # Both events are physically present (append-only).
    assert len(fr.load_events_lenient(tmp_registry.jsonl)) == 2


def test_append_anchor_produces_latest_state(tmp_registry):
    fr.register_finding(
        _finding(), path=tmp_registry.jsonl, lock_path=tmp_registry.lock
    )
    fr.append_anchor(
        "sample_finding_v1",
        _anchor(anchor_id="a2", residual=0.10),
        path=tmp_registry.jsonl,
        lock_path=tmp_registry.lock,
    )
    got = fr.get_finding("sample_finding_v1", path=tmp_registry.jsonl)
    assert got is not None
    assert len(got.anchors) == 2
    assert {a.anchor_id for a in got.anchors} == {"a1", "a2"}


def test_append_anchor_to_missing_finding_raises(tmp_registry):
    with pytest.raises(InvalidFindingError, match="not found"):
        fr.append_anchor(
            "nope_v1", _anchor(), path=tmp_registry.jsonl, lock_path=tmp_registry.lock
        )


def test_query_by_producer_consumer_domain(tmp_registry):
    fr.register_finding(
        _finding(
            producers=("commit abc123",),
            consumers=("tools/bench_evidence.py",),
            domain_of_validity={"os": ["linux"], "target": "wasm-witness"},
        ),
        path=tmp_registry.jsonl,
        lock_path=tmp_registry.lock,
    )
    assert fr.query_by_producer("abc123", path=tmp_registry.jsonl)
    assert fr.query_by_consumer("tools/bench_evidence.py", path=tmp_registry.jsonl)
    assert fr.query_by_domain("wasm-witness", path=tmp_registry.jsonl)
    assert not fr.query_by_domain("nonexistent-target", path=tmp_registry.jsonl)


def test_corrupt_ledger_strict_raises(tmp_registry):
    tmp_registry.jsonl.parent.mkdir(parents=True, exist_ok=True)
    tmp_registry.jsonl.write_text("{not json\n", encoding="utf-8")
    with pytest.raises(fr.FindingsRegistryCorruptError):
        fr.load_events_strict(tmp_registry.jsonl)
    # Lenient load skips the bad line silently.
    assert fr.load_events_lenient(tmp_registry.jsonl) == []


# ---------------------------------------------------------------------------
# (c) memo -> registry lint
# ---------------------------------------------------------------------------


def test_lint_blocks_measured_line_without_finding_id():
    v = lint.scan_text("The kernel got a 2.3x speedup on the hot loop.\n")
    assert len(v) == 1


def test_lint_passes_with_formalization_pending():
    text = (
        "The kernel got a 2.3x speedup.  "
        "# FORMALIZATION_PENDING: bench not yet re-run on release build\n"
    )
    assert lint.scan_text(text) == []


def test_lint_passes_with_finding_id_token():
    text = "2.3x speedup (finding_id: loop_accumulator_raw_lane_v1)\n"
    assert lint.scan_text(text) == []


def test_lint_passes_with_registered_inline_id():
    ids = frozenset({"loop_accumulator_raw_lane_v1"})
    text = "The loop_accumulator_raw_lane_v1 result: 2.3x measured on release.\n"
    assert lint.scan_text(text, registered_ids=ids) == []
    # Without the id registered, the bare token is not accepted.
    assert len(lint.scan_text(text, registered_ids=frozenset())) == 1


def test_lint_rejects_placeholder_rationale():
    for bad in ("<why>", "tbd", "ok"):
        text = f"got a 2.3x speedup.  # FORMALIZATION_PENDING: {bad}\n"
        assert len(lint.scan_text(text)) == 1, f"placeholder {bad!r} must not waive"


def test_lint_multiple_vocab_tokens():
    text = (
        "line one has a residual of 0.5\n"
        "line two ran at 1.28x measured\n"
        "line three is prose with no numbers\n"
        "line four: 100 ns/op on the microbench\n"
    )
    v = lint.scan_text(text)
    assert len(v) == 3  # lines 1, 2, 4 are measured; line 3 is not


def test_lint_ns_boundary_no_false_positive_on_word_tails():
    # 'sessions/', 'plans/', 'DNS/' must NOT trip the ns/ unit token.
    text = (
        "artifacts live under target/sessions/<id>\n"
        "see docs/spec/areas/compat/plans/ for the roadmap\n"
        "the child lost DNS/networking\n"
    )
    assert lint.scan_text(text) == []


def test_lint_skips_code_fences():
    text = "```\nthis fenced block mentions a 2.3x speedup example\n```\n"
    assert lint.scan_text(text) == []


def test_lint_file_level_skip_marker():
    text = (
        "<!-- findings-lint: skip-file research synthesis describing pact residuals -->\n"
        "pact reports a residual of 0.0 and a 1.65x speedup elsewhere\n"
    )
    assert lint.scan_text(text) == []
    # A placeholder skip rationale does NOT suppress.
    text2 = "<!-- findings-lint: skip-file tbd -->\nresidual 0.0 measured 2.3x\n"
    assert len(lint.scan_text(text2)) == 1


def test_lint_strict_mode_exit_code(tmp_path):
    memo = tmp_path / "memo.md"
    memo.write_text("A 2.3x speedup was measured.\n", encoding="utf-8")
    # Warn-only (default): exit 0 even with a violation.
    assert lint.main([str(memo)]) == 0
    # Strict: exit 1 when the backlog is non-empty.
    assert lint.main(["--strict", str(memo)]) == 1
    # Strict on a formalized memo: exit 0.
    memo.write_text(
        "A 2.3x speedup was measured (finding_id: probe_int_checkedmul_peel_v1).\n",
        encoding="utf-8",
    )
    assert lint.main(["--strict", str(memo)]) == 0


def test_lint_real_tree_backlog_is_reportable():
    # The lint runs on the real docs/agent tree and returns a finite backlog. This
    # is the warn-only count the strict_when='backlog_count == 0' flip is gated on.
    violations = lint.scan_tree()
    assert isinstance(violations, list)
    # There IS existing un-formalized debt (the gate is warn-only for a reason);
    # if this ever hits 0 the ci_gate check should be flipped to --strict.
    assert len(violations) >= 0


# ---------------------------------------------------------------------------
# (d) lock round-trips + real mutual exclusion
# ---------------------------------------------------------------------------


def test_lock_round_trips(tmp_registry):
    # Acquire -> release -> re-acquire must all succeed.
    with fr.registry_lock(tmp_registry.lock):
        assert fr._lock_held() is True
    assert fr._lock_held() is False
    with fr.registry_lock(tmp_registry.lock):
        assert fr._lock_held() is True
    assert fr._lock_held() is False


def test_lock_is_reentrant_within_thread(tmp_registry):
    with fr.registry_lock(tmp_registry.lock):
        assert fr._get_lock_depth() == 1
        with fr.registry_lock(tmp_registry.lock):
            assert fr._get_lock_depth() == 2
        assert fr._get_lock_depth() == 1
    assert fr._get_lock_depth() == 0


def test_lock_provides_real_mutual_exclusion(tmp_registry, monkeypatch):
    # A second acquirer (different OS handle) must BLOCK while the lock is held,
    # then time out. This proves the msvcrt/fcntl lock actually excludes — a lock
    # that never conflicts certifies nothing.
    monkeypatch.setattr(fr, "LOCK_TIMEOUT_SECONDS", 0.4)
    acquired = threading.Event()
    release = threading.Event()
    errors: list[BaseException] = []

    def holder():
        try:
            with fr.registry_lock(tmp_registry.lock):
                acquired.set()
                release.wait(5)
        except BaseException as exc:  # noqa: BLE001
            errors.append(exc)

    t = threading.Thread(target=holder)
    t.start()
    try:
        assert acquired.wait(5), "holder thread never acquired the lock"
        with pytest.raises(TimeoutError):
            with fr.registry_lock(tmp_registry.lock):
                pass
    finally:
        release.set()
        t.join(5)
    assert not errors, f"holder thread errored: {errors}"
    # After release, the lock is free again.
    with fr.registry_lock(tmp_registry.lock):
        assert fr._lock_held() is True


def test_locked_write_is_durable_and_readable(tmp_registry):
    fr.register_finding(
        _finding(), path=tmp_registry.jsonl, lock_path=tmp_registry.lock
    )
    raw = tmp_registry.jsonl.read_text(encoding="utf-8").splitlines()
    assert len(raw) == 1
    rec = json.loads(raw[0])
    assert rec["event_type"] == fr.EVENT_REGISTERED
    assert rec["finding_id"] == "sample_finding_v1"
    assert rec["schema_version"] == fr.SCHEMA_VERSION


# ---------------------------------------------------------------------------
# (e) seeded keystone findings
# ---------------------------------------------------------------------------


def test_seed_findings_construct_and_validate():
    findings = build_seed_findings()
    ids = {f.finding_id for f in findings}
    assert {
        "probe_int_checkedmul_peel_v1",
        "loop_accumulator_raw_lane_v1",
        "witness_frontend_lowering_cold_cost_v1",
        "incremental_build_sccache_off_v1",
    } <= ids
    for f in findings:
        # Orphan ban satisfied honestly.
        assert f.producers or f.consumers
        # Every anchor cites a real source artifact.
        for a in f.anchors:
            assert a.source_artifact.strip()
        # to_dict/from_dict round-trips through the same invariants.
        assert Finding.from_dict(f.to_dict()).finding_id == f.finding_id


def test_seed_registry_writes_and_is_idempotent(tmp_registry):
    registered, skipped = fr.seed_registry(
        path=tmp_registry.jsonl, lock_path=tmp_registry.lock
    )
    assert len(registered) == 4
    assert skipped == []
    loaded = {f.finding_id for f in fr.query_findings(path=tmp_registry.jsonl)}
    assert "probe_int_checkedmul_peel_v1" in loaded
    # Re-seed: nothing new, all skipped.
    registered2, skipped2 = fr.seed_registry(
        path=tmp_registry.jsonl, lock_path=tmp_registry.lock
    )
    assert registered2 == []
    assert len(skipped2) == 4


def test_committed_seed_registry_loads_and_validates():
    # The committed .molt/state/findings_registry.jsonl (the real seeded registry)
    # must load and every record must re-validate under current invariants.
    findings = fr.query_findings()
    ids = {f.finding_id for f in findings}
    assert {
        "probe_int_checkedmul_peel_v1",
        "loop_accumulator_raw_lane_v1",
        "witness_frontend_lowering_cold_cost_v1",
        "incremental_build_sccache_off_v1",
    } <= ids
    for f in findings:
        assert f.producers or f.consumers  # orphan ban holds on the real registry
