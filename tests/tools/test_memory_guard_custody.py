"""Birth-bound custody evidence is shared by the producer, CLI, and disk guard.

Tests use a private state root: ``<tmp>/state/active`` and its retired
history ``<tmp>/state/retired``. Process tables are faked only at the
snapshot boundary; markers, scratch generations and locks are real files.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Event
from types import SimpleNamespace

import pytest

from molt import file_publication
from molt import temporary_artifacts as scratch
from molt.exact_json import read_exact
from tests.process_guard_common import (
    close_owned_test_process,
    install_module_view,
    start_owned_test_process,
)
from tools import memory_guard_custody as cli
from tools.memory_guard_core import active_custody as custody
from tools.memory_guard_core import process_model, windows_snapshot

# These tests fake process data the session sentinel also reads.
pytestmark = pytest.mark.usefixtures("session_sentinel_paused")


def _sample(pid=99, birth=9900, pgid=None):
    return SimpleNamespace(pid=pid, started_at_ns=birth, pgid=pgid)


def _snapshot(*samples):
    values = samples or (_sample(),)
    return lambda: {sample.pid: sample for sample in values}


def _active(tmp_path):
    return tmp_path / "state" / "active"


def _retired(tmp_path):
    return tmp_path / "state" / "retired"


def _marker(
    directory, *, pid=10, birth=100, child_birth=200, status="child_running", child=True
):
    directory.mkdir(parents=True, exist_ok=True)
    token = f"{pid:032x}"
    payload = {
        "schema_version": 2,
        "pid": pid,
        "token": token,
        "status": status,
        "guard_process": {"pid": pid, "started_at_ns": birth},
        "child_process": {
            "pid": pid + 10,
            "started_at_ns": child_birth,
            "pgid": pid + 10,
        }
        if child
        else None,
        "child_launch_state": "recorded" if child else "not_started",
    }
    path = directory / f"guard-{pid}-{token}.json"
    custody.write_active_guard_marker(path, payload)
    return path


def _payload(path):
    return json.loads(path.read_text(encoding="utf-8"))


def _rewrite(path, mutate):
    payload = _payload(path)
    mutate(payload)
    path.write_text(json.dumps(payload), encoding="utf-8")


def test_dry_run_then_apply_records_exact_births_and_retires_the_record(tmp_path):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    report = custody.reconcile_active_guard_markers(active, _snapshot())
    assert report.decisions[0].disposition == "terminalize"
    assert report.terminalized == 0
    assert marker.read_bytes() == original
    assert not _retired(tmp_path).exists()
    assert custody.has_active_guard_marker(active)

    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.terminalized == 1 and report.retired == 1
    # The resolved record and its lock leave active/; history keeps the record.
    assert list(active.iterdir()) == []
    retired = _retired(tmp_path) / marker.name
    assert report.decisions[0].retired_to == str(retired)
    payload = _payload(retired)
    assert payload["status"] == "custody_reconciled"
    receipt = payload["reconciliation"]
    assert receipt["previous_status"] == "child_running"
    assert receipt["reason"] == "recorded_custody_absent_or_reused"
    assert [
        (item["pid"], item["expected_started_at_ns"], item["state"])
        for item in receipt["evidence"]
    ] == [(10, 100, "absent"), (20, 200, "absent")]
    assert custody.read_marker_record(retired).terminal
    assert not custody.has_active_guard_marker(active)
    frozen = retired.read_bytes()
    again = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert again.decisions == ()
    assert retired.read_bytes() == frozen


@pytest.mark.parametrize(
    "samples,reason",
    [
        ((_sample(10, 100),), "guard_process_identity_match"),
        ((_sample(10, None),), "guard_process_identity_unavailable"),
        ((_sample(20, 200),), "child_process_identity_match"),
        ((_sample(20, None),), "child_process_identity_unavailable"),
        ((_sample(21, 210, pgid=20),), "child_process_group_still_present"),
    ],
)
def test_live_or_uncomparable_custody_remains_protective(tmp_path, samples, reason):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(*samples), apply=True
    )
    assert report.decisions[0].reason == reason
    assert report.preserved == 1
    assert marker.read_bytes() == original
    assert custody.has_active_guard_marker(active)


def test_reused_guard_and_child_are_distinct_identities(tmp_path):
    active = _active(tmp_path)
    _marker(active)
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(10, 1000), _sample(20, 2000)), apply=True
    )
    assert report.terminalized == 1
    assert [item.state for item in report.decisions[0].evidence] == [
        "identity_mismatch",
        "identity_mismatch",
    ]


@pytest.mark.parametrize(
    "status", ["guard_starting", "launch_prepared", "guard_exception"]
)
def test_durable_before_launch_boundary_can_reconcile(tmp_path, status):
    active = _active(tmp_path)
    _marker(active, status=status, child=False)
    assert (
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True
        ).terminalized
        == 1
    )


@pytest.mark.parametrize(
    "status",
    ["spawn_pending", "spawn_failed", "guard_exception", "finalizer_completed"],
)
def test_interrupted_or_failed_spawn_without_child_identity_is_protective(
    tmp_path, status
):
    active = _active(tmp_path)
    marker = _marker(active, status="guard_starting", child=False)
    custody.update_active_guard_marker(
        marker, _payload(marker)["token"], status=status, child_launch_state="pending"
    )
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.decisions[0].reason == "child_launch_identity_unpublished"
    assert report.decisions[0].operator_resolvable
    assert custody.has_active_guard_marker(active)


@pytest.mark.parametrize("birth,child_birth", [(None, 200), (100, None)])
def test_missing_birth_cannot_release_without_process_evidence(
    tmp_path, birth, child_birth
):
    active = _active(tmp_path)
    marker = _marker(active, birth=birth, child_birth=child_birth)
    assert custody.has_active_guard_marker(active)
    # The pid with the unknown birth is present: it may be the recorded process.
    present = _sample(10, 100) if birth is None else _sample(20, 200)
    original = marker.read_bytes()
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(present), apply=True
    )
    decision = report.decisions[0]
    role = "guard" if birth is None else "child"
    assert (decision.disposition, decision.reason) == (
        "preserve",
        f"{role}_process_identity_unavailable",
    )
    assert marker.read_bytes() == original
    assert custody.has_active_guard_marker(active)
    steps = report.next_steps()
    assert len(steps) == 1
    assert f"--release {marker} --apply" in steps[0]


@pytest.mark.parametrize("birth,child_birth", [(None, 200), (100, None)])
def test_absent_pid_is_dead_whatever_its_recorded_birth(tmp_path, birth, child_birth):
    active = _active(tmp_path)
    marker = _marker(active, birth=birth, child_birth=child_birth)
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.terminalized == 1 and report.retired == 1
    payload = _payload(_retired(tmp_path) / marker.name)
    receipt = payload["reconciliation"]
    assert receipt["previous_status"] == "child_running"
    assert [
        (item["role"], item["expected_started_at_ns"], item["state"])
        for item in receipt["evidence"]
    ] == [("guard", birth, "absent"), ("child", child_birth, "absent")]
    assert not custody.has_active_guard_marker(active)


@pytest.mark.parametrize("status", ["completed", "finalizer_completed"])
def test_producer_terminal_status_does_not_depend_on_births(tmp_path, status):
    # A fast child exits before the guard can read its birth.
    active = _active(tmp_path)
    marker = _marker(active, child_birth=None, status=status)
    assert custody.read_marker_record(marker).terminal
    assert not custody.has_active_guard_marker(active)
    # Its guard still runs: the producer retires its own record.
    kept = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(10, 100)), apply=True
    )
    assert kept.decisions[0].disposition == "already_terminal"
    assert marker.exists()
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.retired == 1 and report.terminalized == 0
    assert _payload(_retired(tmp_path) / marker.name)["status"] == status


def test_reused_child_leader_proves_its_recorded_group_closed(tmp_path):
    active = _active(tmp_path)
    _marker(active)
    # Pid 20 now names another process; its new group 20 has a member.
    samples = (_sample(20, 2000, pgid=20), _sample(21, 2100, pgid=20))
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(*samples), apply=True
    )
    assert report.terminalized == 1
    assert [item.state for item in report.decisions[0].evidence] == [
        "absent",
        "identity_mismatch",
    ]


def test_group_of_a_non_leader_child_stays_protective_after_reuse(tmp_path):
    active = _active(tmp_path)
    marker = _marker(active)
    _rewrite(marker, lambda p: p["child_process"].update(pgid=30))
    samples = (_sample(20, 2000), _sample(31, 3100, pgid=30))
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(*samples), apply=True
    )
    assert report.decisions[0].reason == "child_process_group_still_present"
    assert not report.decisions[0].operator_resolvable
    assert custody.has_active_guard_marker(active)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda p: p.update(schema_version=1),
        lambda p: p.update(schema_version=3),
        lambda p: p.update(schema_version=True),
        lambda p: p.update(schema_version=2.0),
        lambda p: p.update(pid=True),
        lambda p: p.update(token="wrong"),
        lambda p: p["guard_process"].update(pid=11),
        lambda p: p["guard_process"].update(started_at_ns=True),
        lambda p: p["child_process"].update(started_at_ns=2.0),
        lambda p: p["child_process"].update(pgid=False),
        lambda p: p["child_process"].update(pid=10),
        lambda p: p.update(status="unrecognized"),
        lambda p: p.update(child_launch_state="pending"),
        lambda p: p.update(status="custody_reconciled"),
    ],
)
def test_invalid_terminal_records_fail_closed(tmp_path, mutate):
    active = _active(tmp_path)
    marker = _marker(active, status="completed")
    _rewrite(marker, mutate)
    original = marker.read_bytes()
    assert custody.has_active_guard_marker(active)
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.preserved == 1
    assert report.decisions[0].reason.startswith("marker_invalid: ")
    assert marker.read_bytes() == original


@pytest.mark.parametrize(
    "content", [b"{", b"[]", b"\xff", b'{"status":"completed","status":"completed"}']
)
def test_inexact_or_unreadable_json_is_protective(tmp_path, content):
    active = _active(tmp_path)
    active.mkdir(parents=True)
    marker = active / f"guard-10-{'a' * 32}.json"
    marker.write_bytes(content)
    assert custody.has_active_guard_marker(active)
    assert (
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert marker.read_bytes() == content


@pytest.mark.parametrize(
    "corrupt",
    [
        lambda r: r["evidence"][0].update(expected_started_at_ns=999),
        lambda r: r["evidence"][0].update(state="identity_unavailable"),
        lambda r: r["evidence"][0].update(state="identity_match"),
        lambda r: r.update(reason="age"),
        lambda r: r.update(previous_status="custody_reconciled"),
    ],
)
def test_corrupted_reconciliation_receipt_does_not_release_artifacts(tmp_path, corrupt):
    active = _active(tmp_path)
    marker = _marker(active)
    custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    # Put a damaged copy of the reconciled record back into active custody.
    payload = _payload(_retired(tmp_path) / marker.name)
    corrupt(payload["reconciliation"])
    marker.write_text(json.dumps(payload), encoding="utf-8")
    assert custody.has_active_guard_marker(active)
    assert (
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True
        ).preserved
        == 1
    )


def test_guard_shaped_directory_is_protective_and_never_rewritten(tmp_path):
    active = _active(tmp_path)
    marker = active / f"guard-10-{'a' * 32}.json"
    marker.mkdir(parents=True)
    assert custody.has_active_guard_marker(active)
    assert (
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert marker.is_dir()
    # An operator release cannot move what is not a regular file.
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(), apply=True, release=[marker]
    )
    assert report.decisions[0].retired_to is None
    assert report.decisions[0].retirement.startswith("marker_is_not_a_regular_file")
    assert marker.is_dir()


def test_indirect_marker_never_releases_or_rewrites_target(tmp_path):
    target_dir = tmp_path / "target"
    target = _marker(target_dir, status="completed")
    active = tmp_path / "active"
    active.mkdir()
    link = active / target.name
    try:
        link.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    original = target.read_bytes()
    assert custody.has_active_guard_marker(active)
    assert (
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert target.read_bytes() == original
    assert link.is_symlink()


def test_unreadable_directory_fails_closed(tmp_path, monkeypatch):
    active = _active(tmp_path)
    active.mkdir(parents=True)
    original_iterdir = Path.iterdir

    def unreadable(path):
        if path == active:
            raise PermissionError("denied")
        return original_iterdir(path)

    monkeypatch.setattr(Path, "iterdir", unreadable)
    assert custody.has_active_guard_marker(active)
    with pytest.raises(PermissionError, match="denied"):
        custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)


def test_parent_watched_pid_and_mtime_never_override_nested_identity(tmp_path):
    active = _active(tmp_path)
    nested = _marker(active)
    parent = _marker(active, pid=1, status="completed")
    _rewrite(parent, lambda p: p.update(termination_reports=[{"watched_pids": [10]}]))
    os.utime(nested, ns=(100, 100))
    os.utime(parent, ns=(200, 200))
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(10, 100)), apply=True
    )
    assert report.preserved == 1
    # The finished parent leaves; its nested child's live record stays.
    assert report.retired == 1 and not parent.exists()
    assert _payload(nested)["status"] == "child_running"
    assert custody.has_active_guard_marker(active)


def test_snapshot_failure_or_empty_result_never_changes_markers(
    tmp_path, monkeypatch, capsys
):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()

    def failed():
        raise process_model.ProcessSnapshotError("unavailable")

    for sampler in (failed, lambda: {}):
        monkeypatch.setattr(cli, "sample_processes", sampler)
        assert cli.main(["--active-dir", str(active), "--apply"]) == 2
        assert "no markers changed" in capsys.readouterr().err
        assert marker.read_bytes() == original


def test_invalid_snapshot_identity_rejected_before_any_apply(tmp_path):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    with pytest.raises(custody.ActiveCustodyError, match="invalid identity"):
        custody.reconcile_active_guard_markers(
            active, lambda: {99: _sample(100)}, apply=True
        )
    assert marker.read_bytes() == original


def test_kernel_process_group_zero_does_not_invalidate_full_native_snapshot(tmp_path):
    active = _active(tmp_path)
    _marker(active)
    report = custody.reconcile_active_guard_markers(active, _snapshot(_sample(pgid=0)))
    assert report.decisions[0].disposition == "terminalize"


def test_cli_defaults_to_one_snapshot_and_dry_run(tmp_path, monkeypatch, capsys):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    calls = []

    def snapshot():
        calls.append(True)
        return {99: _sample()}

    monkeypatch.setattr(cli, "sample_processes", snapshot)
    assert cli.main(["--active-dir", str(active), "--json"]) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["apply"] is False
    assert result["decisions"][0]["disposition"] == "terminalize"
    assert calls == [True]
    assert marker.read_bytes() == original


def test_markers_created_during_snapshot_are_not_judged_by_older_observation(tmp_path):
    active = _active(tmp_path)
    active.mkdir(parents=True)

    def snapshot():
        _marker(active)
        return {99: _sample()}

    report = custody.reconcile_active_guard_markers(active, snapshot, apply=True)
    assert report.decisions == ()
    assert custody.has_active_guard_marker(active)


def test_same_bytes_in_new_file_generation_fail_compare_and_swap(tmp_path):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()

    def snapshot():
        replacement = active / "replacement"
        replacement.write_bytes(original)
        os.replace(replacement, marker)
        return {99: _sample()}

    report = custody.reconcile_active_guard_markers(active, snapshot, apply=True)
    assert report.decisions[0].reason == "marker_changed_during_reconciliation"
    assert marker.read_bytes() == original


def test_producer_update_wins_over_concurrent_reconciliation(tmp_path, monkeypatch):
    active = _active(tmp_path)
    marker = _marker(active)
    token = _payload(marker)["token"]
    writer_entered, snapshot_taken, release_writer = Event(), Event(), Event()
    publish = custody.atomic_write_bytes

    def paused_publish(path, data, **kwargs):
        if json.loads(data)["status"] == "child_running_telemetry_degraded":
            writer_entered.set()
            assert release_writer.wait(5)
        return publish(path, data, **kwargs)

    def snapshot():
        snapshot_taken.set()
        return {99: _sample()}

    monkeypatch.setattr(custody, "atomic_write_bytes", paused_publish)
    with ThreadPoolExecutor(max_workers=2) as executor:
        writer = executor.submit(
            custody.update_active_guard_marker,
            marker,
            token,
            status="child_running_telemetry_degraded",
            detail="latest",
        )
        try:
            assert writer_entered.wait(5)
            reconciler = executor.submit(
                custody.reconcile_active_guard_markers, active, snapshot, apply=True
            )
            assert snapshot_taken.wait(5)
        finally:
            release_writer.set()
        assert writer.result(timeout=5)
        report = reconciler.result(timeout=5)
    assert report.decisions[0].reason == "marker_changed_during_reconciliation"
    assert _payload(marker)["detail"] == "latest"
    assert custody.has_active_guard_marker(active)


def test_update_rejects_identity_changes(tmp_path):
    marker = _marker(_active(tmp_path))
    token = _payload(marker)["token"]
    original = marker.read_bytes()
    assert not custody.update_active_guard_marker(marker, "wrong", status="completed")
    with pytest.raises(custody.ActiveCustodyError, match="immutable"):
        custody.update_active_guard_marker(marker, token, status="completed", pid=20)
    with pytest.raises(custody.ActiveCustodyError, match="child identity"):
        custody.update_active_guard_marker(
            marker,
            token,
            status="completed",
            child_process={"pid": 20, "started_at_ns": 201, "pgid": 20},
        )
    assert marker.read_bytes() == original


def test_windows_current_birth_observes_owned_pseudo_handle_only(monkeypatch):
    class CurrentProcess:
        def __call__(self):
            return -1

    seen = []
    monkeypatch.setattr(windows_snapshot, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(
        windows_snapshot,
        "process_query_api",
        lambda: SimpleNamespace(GetCurrentProcess=CurrentProcess()),
    )
    monkeypatch.setattr(
        windows_snapshot,
        "windows_process_handle_started_at_ns",
        lambda handle: seen.append(handle) or 1234,
    )
    assert windows_snapshot.windows_current_process_started_at_ns() == 1234
    assert seen == [-1]


def test_windows_scalar_birth_never_opens_arbitrary_pid(monkeypatch):
    monkeypatch.setattr(
        process_model, "os", SimpleNamespace(name="nt", getpid=lambda: 10)
    )
    monkeypatch.setattr(
        process_model, "windows_current_process_started_at_ns", lambda: 1234
    )
    assert process_model.process_started_at_ns(10) == 1234
    assert process_model.process_started_at_ns(20) is None


# --- launch outcome --------------------------------------------------------


def test_failed_launch_records_no_child_and_reconciles_once_the_guard_is_gone(
    tmp_path,
):
    active = _active(tmp_path)
    marker = _marker(active, status="guard_starting", child=False)
    token = _payload(marker)["token"]
    custody.update_active_guard_marker(
        marker, token, status="spawn_pending", child_launch_state="pending"
    )
    custody.update_active_guard_marker(
        marker, token, status="spawn_failed", child_launch_state="failed"
    )
    custody.update_active_guard_marker(marker, token, status="guard_exception")
    assert _payload(marker)["child_launch_state"] == "failed"
    assert custody.has_active_guard_marker(active)
    live = custody.reconcile_active_guard_markers(active, _snapshot(_sample(10, 100)))
    assert live.decisions[0].reason == "guard_process_identity_match"
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.terminalized == 1 and report.retired == 1
    assert not custody.has_active_guard_marker(active)


@pytest.mark.parametrize(
    "prior,launch",
    [
        ([], "failed"),
        ([("spawn_pending", "pending"), ("child_running", "recorded")], "failed"),
        ([("spawn_pending", "pending"), ("spawn_failed", "failed")], "pending"),
        ([("spawn_pending", "pending"), ("spawn_failed", "failed")], "not_started"),
    ],
)
def test_launch_failure_is_reachable_only_from_the_launch_boundary(
    tmp_path, prior, launch
):
    marker = _marker(_active(tmp_path), status="guard_starting", child=False)
    token = _payload(marker)["token"]
    for status, state in prior:
        fields = {"child_launch_state": state}
        if state == "recorded":
            fields["child_process"] = {"pid": 20, "started_at_ns": 200, "pgid": 20}
        custody.update_active_guard_marker(marker, token, status=status, **fields)
    original = marker.read_bytes()
    with pytest.raises(custody.ActiveCustodyError):
        custody.update_active_guard_marker(
            marker, token, status="guard_exception", child_launch_state=launch
        )
    assert marker.read_bytes() == original


def test_failed_launch_cannot_claim_a_child(tmp_path):
    marker = _marker(_active(tmp_path), status="guard_starting", child=False)
    _rewrite(
        marker,
        lambda p: p.update(status="completed", child_launch_state="failed"),
    )
    assert custody.read_marker_record(marker).error == (
        "child_launch_status_inconsistent"
    )


# --- retirement and bounded history ----------------------------------------


def test_active_readers_cost_live_records_not_history(tmp_path, monkeypatch):
    active = _active(tmp_path)
    for pid in range(10, 30):
        _marker(active, pid=pid, status="completed")
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.retired == 20
    live = _marker(active, pid=40)
    parsed = []
    read = custody.read_marker_record
    monkeypatch.setattr(
        custody, "read_marker_record", lambda path: parsed.append(path) or read(path)
    )
    assert custody.has_active_guard_marker(active)
    assert parsed == [live]
    assert sorted(path.name for path in active.iterdir()) == [
        live.name,
        live.with_suffix(".lock").name,
    ]


def test_retired_history_keeps_the_newest_records(tmp_path, monkeypatch):
    monkeypatch.setattr(custody, "RETIRED_GUARD_MARKER_KEEP", 3)
    monkeypatch.setattr(custody, "_RETIRED_PRUNE_SLACK", 1)
    active = _active(tmp_path)
    for pid in range(10, 15):
        marker = _marker(active, pid=pid, status="completed")
        # A retired marker keeps its last-write time; newer pids wrote later.
        os.utime(marker, ns=(pid * 10**9, pid * 10**9))
        report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
        assert report.retired == 1
    names = sorted(path.name for path in _retired(tmp_path).iterdir())
    assert names == [f"guard-{pid}-{pid:032x}.json" for pid in (12, 13, 14)]


def test_guard_marker_lookup_reads_active_and_retired_records_of_one_pid(tmp_path):
    active = _active(tmp_path)
    done = _marker(active, pid=10, status="completed")
    custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    running = _marker(active, pid=11)
    assert [record.path for record in custody.guard_marker_records(active, 10)] == [
        _retired(tmp_path) / done.name
    ]
    assert [record.path for record in custody.guard_marker_records(active, 11)] == [
        running
    ]


def test_orphan_locks_leave_with_a_full_apply(tmp_path):
    active = _active(tmp_path)
    live = _marker(active, pid=10)
    orphan = active / f"guard-11-{11:032x}.lock"
    orphan.write_bytes(b"")
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(10, 100)), apply=True
    )
    assert report.removed_locks == 1
    assert not orphan.exists()
    assert live.with_suffix(".lock").exists()


def test_producer_retires_only_its_own_resolved_record(tmp_path):
    active = _active(tmp_path)
    done = _marker(active, pid=10, status="completed")
    raised = _marker(active, pid=11, status="guard_exception", child=False)
    token = _payload(done)["token"]
    assert custody.retire_active_guard_marker(done, "f" * 32) is None
    assert custody.retire_active_guard_marker(raised, _payload(raised)["token"]) is None
    retired = custody.retire_active_guard_marker(done, token)
    assert retired == _retired(tmp_path) / done.name
    assert not done.exists() and not done.with_suffix(".lock").exists()
    assert raised.exists()


# --- operator release ------------------------------------------------------


def test_release_resolves_inconclusive_evidence_with_an_attested_receipt(tmp_path):
    active = _active(tmp_path)
    pending = _marker(active, pid=10, status="guard_starting", child=False)
    custody.update_active_guard_marker(
        pending,
        _payload(pending)["token"],
        status="guard_exception",
        child_launch_state="pending",
    )
    unknown = _marker(active, pid=11, child_birth=None)
    samples = _snapshot(_sample(21, 2100))
    plan = custody.reconcile_active_guard_markers(active, samples)
    assert [item.reason for item in plan.decisions] == [
        "child_launch_identity_unpublished",
        "child_process_identity_unavailable",
    ]
    assert len(plan.next_steps()) == 2
    report = custody.reconcile_active_guard_markers(
        active, samples, apply=True, release=[pending, unknown]
    )
    assert report.terminalized == 2 and report.retired == 2
    for marker in (pending, unknown):
        receipt = _payload(_retired(tmp_path) / marker.name)["reconciliation"]
        assert receipt["reason"] == "operator_attested"
    states = _payload(_retired(tmp_path) / unknown.name)["reconciliation"]["evidence"]
    assert [item["state"] for item in states] == ["absent", "identity_unavailable"]
    assert not custody.has_active_guard_marker(active)


def test_release_refuses_live_evidence(tmp_path):
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(20, 200)), apply=True, release=[marker]
    )
    assert report.decisions[0].reason == "child_process_identity_match"
    assert report.next_steps() == []
    assert marker.read_bytes() == original


def test_release_moves_an_unreadable_record_unchanged(tmp_path):
    active = _active(tmp_path)
    active.mkdir(parents=True)
    marker = active / f"guard-10-{'a' * 32}.json"
    marker.write_bytes(b"{")
    plan = custody.reconcile_active_guard_markers(active, _snapshot())
    assert plan.decisions[0].operator_resolvable
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(), apply=True, release=[marker]
    )
    assert report.decisions[0].disposition == "release"
    assert (_retired(tmp_path) / marker.name).read_bytes() == b"{"
    assert not custody.has_active_guard_marker(active)


@pytest.mark.parametrize("name", ["other/guard-10-" + "a" * 32 + ".json", "x.json"])
def test_release_names_only_records_in_the_active_directory(tmp_path, name):
    active = _active(tmp_path)
    _marker(active)
    outside = tmp_path / name
    outside.parent.mkdir(parents=True, exist_ok=True)
    outside.write_text("{}", encoding="utf-8")
    with pytest.raises(custody.ActiveCustodyError, match="release names"):
        custody.reconcile_active_guard_markers(
            active, _snapshot(), apply=True, release=[outside]
        )


def test_cli_names_the_release_command_for_inconclusive_records(
    tmp_path, monkeypatch, capsys
):
    active = _active(tmp_path)
    marker = _marker(active, child_birth=None)
    monkeypatch.setattr(cli, "sample_processes", _snapshot(_sample(20, 2000)))
    assert cli.main(["--active-dir", str(active)]) == 0
    out = capsys.readouterr().out
    assert "operator action required:" in out
    assert f"--release {marker} --apply" in out
    assert cli.main(["--active-dir", str(active), "--release", str(marker)]) == 0
    assert marker.exists()  # A release without --apply is a dry run.
    args = ["--active-dir", str(active), "--release", str(marker), "--apply"]
    assert cli.main(args) == 0
    assert not marker.exists()
    assert (_retired(tmp_path) / marker.name).exists()


def test_cli_apply_records_what_the_sweep_left(tmp_path, monkeypatch):
    active = _active(tmp_path)
    _marker(active, pid=10)
    _marker(active, pid=11)
    monkeypatch.setattr(cli, "sample_processes", _snapshot(_sample(11, 100)))
    assert cli.main(["--active-dir", str(active), "--apply"]) == 0
    receipt = read_exact(active.parent / "sweep.json", max_bytes=65536, label="test")
    assert receipt["remaining"] == 1


# --- scratch custody -------------------------------------------------------


def _guarded_scratch(tmp_path, *, pid=10, status="child_running"):
    """A real scratch lease bound to a real marker, as the producer makes them."""
    active = _active(tmp_path)
    marker = _marker(active, pid=pid)
    token = _payload(marker)["token"]
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(active.parent),
        "MOLT_MEMORY_GUARD_TOKEN": token,
        "MOLT_MEMORY_GUARD_MARKER": str(marker),
    }
    lease = scratch.acquire_guard_scratch(tmp_path, env)
    (lease.target / "output").write_bytes(b"payload")
    custody.update_active_guard_marker(
        marker,
        token,
        status=status,
        temporary_artifacts={"state": "leased", "target": str(lease.target)},
    )
    return active, marker, lease


def _finish_indeterminate(marker, lease):
    outcome = scratch.finish_guard_scratch(
        lease, closed=False, success=False, evidence={"closed": False}
    )
    custody.update_active_guard_marker(
        marker,
        _payload(marker)["token"],
        status="completed",
        temporary_artifacts=outcome,
    )


def test_dead_guard_lease_is_adopted_through_the_scratch_authority(tmp_path):
    active, marker, lease = _guarded_scratch(tmp_path)
    lease.release()  # The guard died: its lock is gone, its lease is not.
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    decision = report.decisions[0]
    assert decision.disposition == "terminalize" and decision.retired_to
    assert decision.scratch["state"] == "retained"
    assert not lease.target.exists()
    payload = lease.generation / "payload" / "output"
    assert payload.read_bytes() == b"payload"
    terminal = read_exact(
        lease.generation / "terminal.json", max_bytes=65536, label="test"
    )
    assert terminal["closure"]["authority"] == "reconciled-custody"
    assert terminal["finished_ns"] == 0
    assert terminal["retained_bytes"] == len(b"payload")
    assert report.scratch_retention[0]["retained_count"] == 1


def test_busy_lease_keeps_the_record_until_its_lock_is_free(tmp_path):
    active, marker, lease = _guarded_scratch(tmp_path)
    try:
        report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
        decision = report.decisions[0]
        assert decision.applied and decision.retired_to is None
        assert decision.retirement == "scratch_busy"
        assert lease.target.is_dir()
        # Reconciled, so it no longer blocks reclamation, but it stays listed.
        assert not custody.has_active_guard_marker(active)
        assert marker.exists()
    finally:
        lease.release()
    again = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert again.decisions[0].disposition == "retire"
    assert again.decisions[0].retired_to
    assert (lease.generation / "payload" / "output").read_bytes() == b"payload"


def test_indeterminate_scratch_waits_while_the_child_group_lives(tmp_path):
    active, marker, lease = _guarded_scratch(tmp_path)
    _finish_indeterminate(marker, lease)
    assert not custody.has_active_guard_marker(active)
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(21, 2100, pgid=20)), apply=True
    )
    decision = report.decisions[0]
    assert decision.disposition == "already_terminal"
    assert decision.reason == "child_process_group_still_present"
    assert (lease.target / "output").read_bytes() == b"payload"
    assert marker.exists()
    # The same holds when the scratch is already resolved.
    resolved = _marker(active, pid=11, status="completed")
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(22, 2200, pgid=21)), apply=True
    )
    (kept,) = [item for item in report.decisions if item.marker == str(resolved)]
    assert kept.reason == "child_process_group_still_present"
    assert resolved.exists()


def test_indeterminate_scratch_whose_payload_is_gone_resolves_with_a_receipt(
    tmp_path,
):
    active, marker, lease = _guarded_scratch(tmp_path)
    _finish_indeterminate(marker, lease)
    shutil.rmtree(lease.target)  # An operator removed the payload by hand.
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    decision = report.decisions[0]
    assert decision.disposition == "retire" and decision.retired_to
    assert decision.scratch["state"] == "reclaimed"
    assert decision.scratch.get("error") is None
    # Reclaimed generations hold no custody: the receipts go too.
    assert not lease.generation.exists()
    assert report.scratch_retention[0]["errors"] == []
    again = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert again.decisions == ()
    assert scratch.reclaim_terminal_scratch(lease.generation.parent)["errors"] == []


def test_retirement_removes_a_reclaimed_generation_left_by_older_code(tmp_path):
    active, marker, lease = _guarded_scratch(tmp_path)
    outcome = scratch.finish_guard_scratch(
        lease, closed=True, success=False, evidence={"closed": True}
    )
    assert outcome["state"] == "retained"
    # Older code reclaimed the payload, dropped the index, and kept receipts.
    with scratch._locked(lease.generation):
        owner = scratch._reclaim_locked(
            lease.generation, scratch._owner(lease.generation)
        )
    scratch._drop_index(lease.generation)
    assert owner["state"] == "reclaimed" and lease.generation.is_dir()
    custody.update_active_guard_marker(
        marker,
        _payload(marker)["token"],
        status="completed",
        temporary_artifacts={**outcome, "state": "reclaimed"},
    )
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.decisions[0].retired_to
    assert not lease.generation.exists()


def test_scratch_of_another_marker_is_never_adopted(tmp_path):
    active, marker, lease = _guarded_scratch(tmp_path)
    lease.release()
    other = _marker(active, pid=12)
    custody.update_active_guard_marker(
        other,
        _payload(other)["token"],
        status="child_running",
        temporary_artifacts={
            "state": "indeterminate",
            "receipt": str(lease.generation / "owner.json"),
        },
    )
    report = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(10, 100)), apply=True
    )
    (decision,) = [item for item in report.decisions if item.marker == str(other)]
    assert decision.retirement == "scratch_error"
    assert "another guard marker" in decision.scratch["error"]
    assert (lease.target / "output").read_bytes() == b"payload"


# --- automatic sweep -------------------------------------------------------


def test_exit_sweep_runs_only_when_records_gathered(tmp_path, monkeypatch):
    monkeypatch.setattr(custody, "AUTO_SWEEP_GROWTH", 1)
    active = _active(tmp_path)
    own = _sample(os.getpid(), 1)
    _marker(active, pid=10)
    assert custody.sweep_active_guard_markers(active, _snapshot(own)) is None
    _marker(active, pid=11)
    report = custody.sweep_active_guard_markers(active, _snapshot(own))
    assert report is not None and report.retired == 2
    receipt = read_exact(active.parent / "sweep.json", max_bytes=65536, label="test")
    assert receipt["remaining"] == 0
    # Live records raise the baseline, so they do not trigger every exit.
    for pid in (12, 13):
        _marker(active, pid=pid)
    live = _snapshot(own, _sample(12, 100), _sample(13, 100))
    assert custody.sweep_active_guard_markers(active, live).remaining == 2
    _marker(active, pid=14)
    assert custody.sweep_active_guard_markers(active, live) is None


def test_exit_sweep_refuses_a_table_without_its_reader(tmp_path, monkeypatch):
    monkeypatch.setattr(custody, "AUTO_SWEEP_GROWTH", 0)
    active = _active(tmp_path)
    marker = _marker(active)
    original = marker.read_bytes()
    with pytest.raises(custody.ActiveCustodyError, match="own reader"):
        custody.sweep_active_guard_markers(active, _snapshot())
    assert marker.read_bytes() == original


def test_exit_sweep_leaves_pre_retirement_history_to_the_operator(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(custody, "AUTO_SWEEP_GROWTH", 0)
    monkeypatch.setattr(custody, "AUTO_SWEEP_LIMIT", 3)
    active = _active(tmp_path)
    for pid in range(10, 14):
        _marker(active, pid=pid, status="completed")
    scanned = []
    scandir = os.scandir

    class CountingScandir:
        def __init__(self, path):
            self._entries = scandir(path)

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            self._entries.close()

        def __iter__(self):
            for entry in self._entries:
                scanned.append(entry.name)
                yield entry

    install_module_view(monkeypatch, "os", os, custody, scandir=CountingScandir)
    snapshot = _snapshot(_sample(os.getpid(), 1))
    assert custody.sweep_active_guard_markers(active, snapshot) is None
    # The gate stops reading at the limit plus one marker.
    assert sum(name.endswith(".json") for name in scanned) == 4
    assert len(list(active.glob("*.json"))) == 4


# --- descendant closure ----------------------------------------------------


def _completion(active, *, pid=10, closed=None, groups=None, status="completed"):
    marker = _marker(active, pid=pid, status=status)

    def mutate(payload):
        if closed is not None:
            payload["descendants_closed"] = closed
        if groups is not None:
            payload["orphaned_process_groups"] = groups

    _rewrite(marker, mutate)
    return marker


@pytest.mark.parametrize(
    "closed,groups,terminal",
    [
        (True, [], True),
        (True, [30], True),
        (False, [], False),
        (False, [30], False),
        # Records written before the producer published its verdict.
        (None, None, True),
        (None, [], True),
        (None, [30], False),
    ],
)
@pytest.mark.parametrize("status", ["completed", "finalizer_completed"])
def test_terminal_status_closes_custody_only_with_proven_closure(
    tmp_path, closed, groups, terminal, status
):
    active = _active(tmp_path)
    marker = _completion(active, closed=closed, groups=groups, status=status)
    assert custody.read_marker_record(marker).terminal is terminal
    assert custody.has_active_guard_marker(active) is not terminal
    retired = custody.retire_active_guard_marker(marker, _payload(marker)["token"])
    assert (retired is not None) is terminal
    assert marker.exists() is not terminal


@pytest.mark.parametrize(
    "mutate",
    [
        lambda p: p.update(descendants_closed=1),
        lambda p: p.update(descendants_closed="true"),
        lambda p: p.update(orphaned_process_groups=[0]),
        lambda p: p.update(orphaned_process_groups=[True]),
        lambda p: p.update(orphaned_process_groups=30),
    ],
)
def test_malformed_closure_evidence_is_protective(tmp_path, mutate):
    active = _active(tmp_path)
    marker = _marker(active, status="completed")
    _rewrite(marker, mutate)
    assert custody.read_marker_record(marker).error is not None
    assert custody.has_active_guard_marker(active)


def test_unclosed_completion_waits_for_every_orphaned_group(tmp_path):
    active = _active(tmp_path)
    marker = _completion(active, closed=False, groups=[30, 40])
    live = custody.reconcile_active_guard_markers(
        active, _snapshot(_sample(41, 4100, pgid=40)), apply=True
    )
    decision = live.decisions[0]
    assert decision.reason == "orphaned_process_group_still_present"
    assert not decision.operator_resolvable
    assert custody.has_active_guard_marker(active)
    report = custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    assert report.terminalized == 1 and report.retired == 1
    receipt = _payload(_retired(tmp_path) / marker.name)["reconciliation"]
    assert receipt["previous_status"] == "completed"
    # The child's own group and both orphaned groups were seen empty.
    assert receipt["empty_process_groups"] == [20, 30, 40]
    assert not custody.has_active_guard_marker(active)


@pytest.mark.parametrize(
    "corrupt",
    [
        lambda r: r.update(empty_process_groups=[20, 30]),
        lambda r: r.pop("empty_process_groups"),
        lambda r: r.update(empty_process_groups=[20, 30, 40, 0]),
    ],
)
def test_reconciliation_receipt_must_name_every_orphaned_group(tmp_path, corrupt):
    active = _active(tmp_path)
    marker = _completion(active, closed=False, groups=[30, 40])
    custody.reconcile_active_guard_markers(active, _snapshot(), apply=True)
    payload = _payload(_retired(tmp_path) / marker.name)
    corrupt(payload["reconciliation"])
    marker.write_text(json.dumps(payload), encoding="utf-8")
    assert custody.read_marker_record(marker).error is not None
    assert custody.has_active_guard_marker(active)


@pytest.mark.skipif(os.name == "nt", reason="POSIX process groups")
def test_a_real_orphaned_group_protects_until_its_last_member_exits(tmp_path):
    live = process_model.sample_processes()
    taken = set(live) | {sample.pgid for sample in live.values()}
    guard_pid, child_pid = [pid for pid in range(70_000, 90_000) if pid not in taken][
        :2
    ]
    orphan = start_owned_test_process(
        [sys.executable, "-c", "import time; time.sleep(60)"]
    )
    try:
        group = os.getpgid(orphan.pid)
        assert group != os.getpgid(0)
        active = _active(tmp_path)
        marker = _marker(active, pid=guard_pid, status="completed")
        _rewrite(
            marker,
            lambda p: p.update(
                child_process={"pid": child_pid, "started_at_ns": 1, "pgid": child_pid},
                descendants_closed=False,
                orphaned_process_groups=[group],
            ),
        )
        report = custody.reconcile_active_guard_markers(
            active, process_model.sample_processes, apply=True
        )
        assert report.decisions[0].reason == "orphaned_process_group_still_present"
        assert marker.exists()
    finally:
        close_owned_test_process(orphan)
    report = custody.reconcile_active_guard_markers(
        active, process_model.sample_processes, apply=True
    )
    assert report.terminalized == 1 and report.retired == 1
    receipt = _payload(_retired(tmp_path) / marker.name)["reconciliation"]
    assert group in receipt["empty_process_groups"]


def test_retirement_issues_no_fsync(tmp_path, monkeypatch):
    """A crash that rolls the move back leaves a terminal record to retire again."""
    active = _active(tmp_path)
    marker = _marker(active, status="completed")
    fsyncs = []
    real_fsync = os.fsync
    # Only file publication issues fsync on these paths; count it there
    # without faking os.fsync for the whole process.
    install_module_view(
        monkeypatch,
        "os",
        os,
        file_publication,
        fsync=lambda fd: fsyncs.append(fd) or real_fsync(fd),
    )
    assert custody.retire_active_guard_marker(marker, _payload(marker)["token"])
    assert fsyncs == []
    assert not marker.exists()


def test_exit_sweep_gate_grows_with_the_records_that_stay(tmp_path, monkeypatch):
    monkeypatch.setattr(custody, "AUTO_SWEEP_GROWTH", 2)
    active = _active(tmp_path)
    own = _sample(os.getpid(), 1)
    live = [own]
    for pid in range(10, 30):
        _marker(active, pid=pid)
        live.append(_sample(pid, 100))
    # 20 live records: the first sweep leaves all 20.
    assert custody.sweep_active_guard_markers(active, _snapshot(*live)).remaining == 20
    # The next sweep waits for more than 2 * 20 + 2 records, not 20 + 2.
    for pid in range(30, 52):
        _marker(active, pid=pid)
        live.append(_sample(pid, 100))
    assert custody.sweep_active_guard_markers(active, _snapshot(*live)) is None
    _marker(active, pid=52)
    live.append(_sample(52, 100))
    assert custody.sweep_active_guard_markers(active, _snapshot(*live)).remaining == 43
