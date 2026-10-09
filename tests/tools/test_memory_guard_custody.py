"""Birth-bound active evidence is shared by the producer, CLI, and disk guard."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
from threading import Event
from types import SimpleNamespace

import pytest

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


def test_dry_run_then_apply_keeps_evidence_and_records_exact_births(tmp_path):
    marker = _marker(tmp_path)
    original = marker.read_bytes()
    before = set(tmp_path.iterdir())
    report = custody.reconcile_active_guard_markers(tmp_path, _snapshot())
    assert report.decisions[0].disposition == "terminalize"
    assert report.terminalized == 0
    assert marker.read_bytes() == original
    assert custody.has_active_guard_marker(tmp_path)

    report = custody.reconcile_active_guard_markers(tmp_path, _snapshot(), apply=True)
    assert report.terminalized == 1
    assert set(tmp_path.iterdir()) == before
    payload = _payload(marker)
    assert payload["status"] == "custody_reconciled"
    receipt = payload["reconciliation"]
    assert receipt["previous_status"] == "child_running"
    assert [
        (item["pid"], item["expected_started_at_ns"], item["state"])
        for item in receipt["evidence"]
    ] == [(10, 100, "absent"), (20, 200, "absent")]
    assert not custody.has_active_guard_marker(tmp_path)
    frozen = marker.read_bytes()
    again = custody.reconcile_active_guard_markers(tmp_path, _snapshot(), apply=True)
    assert again.decisions[0].disposition == "already_terminal"
    assert marker.read_bytes() == frozen


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
    marker = _marker(tmp_path)
    original = marker.read_bytes()
    report = custody.reconcile_active_guard_markers(
        tmp_path, _snapshot(*samples), apply=True
    )
    assert report.decisions[0].reason == reason
    assert report.preserved == 1
    assert marker.read_bytes() == original
    assert custody.has_active_guard_marker(tmp_path)


def test_reused_guard_and_child_are_distinct_identities(tmp_path):
    _marker(tmp_path)
    report = custody.reconcile_active_guard_markers(
        tmp_path, _snapshot(_sample(10, 1000), _sample(20, 2000)), apply=True
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
    _marker(tmp_path, status=status, child=False)
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
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
    marker = _marker(tmp_path, status="guard_starting", child=False)
    custody.update_active_guard_marker(
        marker, _payload(marker)["token"], status=status, child_launch_state="pending"
    )
    report = custody.reconcile_active_guard_markers(tmp_path, _snapshot(), apply=True)
    assert report.decisions[0].reason == "child_launch_identity_unpublished"
    assert custody.has_active_guard_marker(tmp_path)


@pytest.mark.parametrize("birth,child_birth", [(None, 200), (100, None)])
@pytest.mark.parametrize("status", ["child_running", "completed"])
def test_missing_birth_cannot_release_even_terminal_evidence(
    tmp_path, birth, child_birth, status
):
    marker = _marker(tmp_path, birth=birth, child_birth=child_birth, status=status)
    original = marker.read_bytes()
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert marker.read_bytes() == original
    assert custody.has_active_guard_marker(tmp_path)


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
    marker = _marker(tmp_path, status="completed")
    _rewrite(marker, mutate)
    original = marker.read_bytes()
    assert custody.has_active_guard_marker(tmp_path)
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert marker.read_bytes() == original


@pytest.mark.parametrize(
    "content", [b"{", b"[]", b"\xff", b'{"status":"completed","status":"completed"}']
)
def test_inexact_or_unreadable_json_is_protective(tmp_path, content):
    marker = tmp_path / f"guard-10-{'a' * 32}.json"
    marker.write_bytes(content)
    assert custody.has_active_guard_marker(tmp_path)
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
        ).preserved
        == 1
    )
    assert marker.read_bytes() == content


def test_corrupted_reconciliation_receipt_does_not_release_artifacts(tmp_path):
    marker = _marker(tmp_path)
    custody.reconcile_active_guard_markers(tmp_path, _snapshot(), apply=True)
    _rewrite(
        marker,
        lambda p: p["reconciliation"]["evidence"][0].update(expected_started_at_ns=999),
    )
    assert custody.has_active_guard_marker(tmp_path)
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
        ).preserved
        == 1
    )


def test_guard_shaped_directory_is_protective_and_never_rewritten(tmp_path):
    marker = tmp_path / f"guard-10-{'a' * 32}.json"
    marker.mkdir()
    assert custody.has_active_guard_marker(tmp_path)
    assert (
        custody.reconcile_active_guard_markers(
            tmp_path, _snapshot(), apply=True
        ).preserved
        == 1
    )
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
    original_iterdir = Path.iterdir

    def unreadable(path):
        if path == tmp_path:
            raise PermissionError("denied")
        return original_iterdir(path)

    monkeypatch.setattr(Path, "iterdir", unreadable)
    assert custody.has_active_guard_marker(tmp_path)
    with pytest.raises(PermissionError, match="denied"):
        custody.reconcile_active_guard_markers(tmp_path, _snapshot(), apply=True)


def test_parent_watched_pid_and_mtime_never_override_nested_identity(tmp_path):
    nested = _marker(tmp_path)
    parent = _marker(tmp_path, pid=1, status="completed")
    _rewrite(parent, lambda p: p.update(termination_reports=[{"watched_pids": [10]}]))
    os.utime(nested, ns=(100, 100))
    os.utime(parent, ns=(200, 200))
    report = custody.reconcile_active_guard_markers(
        tmp_path, _snapshot(_sample(10, 100)), apply=True
    )
    assert report.preserved == 1
    assert _payload(nested)["status"] == "child_running"
    assert custody.has_active_guard_marker(tmp_path)


def test_snapshot_failure_or_empty_result_never_changes_markers(
    tmp_path, monkeypatch, capsys
):
    marker = _marker(tmp_path)
    original = marker.read_bytes()

    def failed():
        raise process_model.ProcessSnapshotError("unavailable")

    for sampler in (failed, lambda: {}):
        monkeypatch.setattr(cli, "sample_processes", sampler)
        assert cli.main(["--active-dir", str(tmp_path), "--apply"]) == 2
        assert "no markers changed" in capsys.readouterr().err
        assert marker.read_bytes() == original


def test_invalid_snapshot_identity_rejected_before_any_apply(tmp_path):
    marker = _marker(tmp_path)
    original = marker.read_bytes()
    with pytest.raises(custody.ActiveCustodyError, match="invalid identity"):
        custody.reconcile_active_guard_markers(
            tmp_path, lambda: {99: _sample(100)}, apply=True
        )
    assert marker.read_bytes() == original


def test_kernel_process_group_zero_does_not_invalidate_full_native_snapshot(tmp_path):
    _marker(tmp_path)
    report = custody.reconcile_active_guard_markers(
        tmp_path, _snapshot(_sample(pgid=0))
    )
    assert report.decisions[0].disposition == "terminalize"


def test_cli_defaults_to_one_snapshot_and_dry_run(tmp_path, monkeypatch, capsys):
    marker = _marker(tmp_path)
    original = marker.read_bytes()
    calls = []

    def snapshot():
        calls.append(True)
        return {99: _sample()}

    monkeypatch.setattr(cli, "sample_processes", snapshot)
    assert cli.main(["--active-dir", str(tmp_path), "--json"]) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["apply"] is False
    assert result["decisions"][0]["disposition"] == "terminalize"
    assert calls == [True]
    assert marker.read_bytes() == original


def test_markers_created_during_snapshot_are_not_judged_by_older_observation(tmp_path):
    def snapshot():
        _marker(tmp_path)
        return {99: _sample()}

    report = custody.reconcile_active_guard_markers(tmp_path, snapshot, apply=True)
    assert report.decisions == ()
    assert custody.has_active_guard_marker(tmp_path)


def test_same_bytes_in_new_file_generation_fail_compare_and_swap(tmp_path):
    marker = _marker(tmp_path)
    original = marker.read_bytes()

    def snapshot():
        replacement = tmp_path / "replacement"
        replacement.write_bytes(original)
        os.replace(replacement, marker)
        return {99: _sample()}

    report = custody.reconcile_active_guard_markers(tmp_path, snapshot, apply=True)
    assert report.decisions[0].reason == "marker_changed_during_reconciliation"
    assert marker.read_bytes() == original


def test_producer_update_wins_over_concurrent_reconciliation(tmp_path, monkeypatch):
    marker = _marker(tmp_path)
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
                custody.reconcile_active_guard_markers, tmp_path, snapshot, apply=True
            )
            assert snapshot_taken.wait(5)
        finally:
            release_writer.set()
        assert writer.result(timeout=5)
        report = reconciler.result(timeout=5)
    assert report.decisions[0].reason == "marker_changed_during_reconciliation"
    assert _payload(marker)["detail"] == "latest"
    assert custody.has_active_guard_marker(tmp_path)


def test_update_rejects_identity_changes(tmp_path):
    marker = _marker(tmp_path)
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
