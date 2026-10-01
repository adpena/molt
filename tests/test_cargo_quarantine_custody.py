"""Model contracts inject filesystem/closure premises; actual_* tests are native.

Passing modeled transactions is not native platform qualification. Native Darwin argv behavior requires the actual-platform gate; Windows model
passes are not native credit. Darwin filesystem recovery still defers unknown.
"""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
import subprocess
import contextlib
from unittest.mock import patch

import pytest

from molt.file_locks import _try_acquire_file_lock, _release_file_lock
from tools.memory_guard_core import cargo_quarantine as cargo
from tools.memory_guard_core.process_model import process_identity


def unit(target, profile="dev-fast", name="owned-unit"):
    path = target / profile / "incremental" / name
    path.mkdir(parents=True)
    (path / "work.o").write_text("owned")
    return path


def observation(path):
    return cargo.CargoIncrementalObservation(
        90051, 300, str(path if path.name == "incremental" else path.parent), 90050, 200
    )


@pytest.fixture(autouse=True)
def model_filesystem_premise(request, monkeypatch):
    if not request.node.name.startswith(("test_actual_", "test_installed_")):
        monkeypatch.setattr(cargo, "_local_cargo_lock_filesystem", lambda path: True)


def recover(target, observed, *, assume_closed=True):
    closure = (
        patch.object(cargo, "_observed_compilers_closed", return_value=True)
        if assume_closed
        else contextlib.nullcontext()
    )
    with closure:
        return cargo._quarantine_cargo_incremental_state(
            reason="timeout",
            target_dir=target,
            command=["cargo", "test", "--profile", "dev-fast"],
            cwd=target.parent,
            observations=observed,
            eligible_observations=frozenset(observed),
            descendants_closed=True,
        )


def test_observe_incremental_requires_typed_owned_rustc_birth(tmp_path):
    path = tmp_path / "cache with spaces" / "owned unit"
    sample = SimpleNamespace(
        pid=90051,
        ppid=90050,
        rss_kb=64,
        command="rustc diagnostic",
        pgid=90050,
        elapsed_sec=None,
        started_at_ns=300,
        argv=("rustc", "-C", f"incremental={path}"),
    )
    parent = SimpleNamespace(
        pid=90050,
        ppid=1,
        started_at_ns=200,
        command="cargo check",
        argv=("cargo", "check"),
    )
    samples = {sample.pid: sample, parent.pid: parent}
    identities = {pid: process_identity(item) for pid, item in samples.items()}
    assert cargo.observe_owned_incremental_state(samples, set(samples), identities) == {
        cargo.CargoIncrementalObservation(90051, 300, str(path), 90050, 200)
    }
    assert (
        cargo.observe_owned_incremental_state({sample.pid: sample}, {sample.pid}, {})
        == set()
    )
    old = SimpleNamespace(started_at_ns=299)
    assert (
        cargo.observe_owned_incremental_state(
            {sample.pid: sample}, {sample.pid}, {sample.pid: process_identity(old)}
        )
        == set()
    )


def test_waiting_unobserved_cargo_preserves_all_incremental_state(tmp_path):
    target = tmp_path / "target"
    owned = unit(target)
    sibling = unit(target, "debug")
    receipt = recover(target, ())
    assert owned.exists() and sibling.exists()
    assert receipt.ownership_status == "deferred" and receipt.moved_paths == ()


def test_observed_profile_recovery_preserves_other_profiles_and_old_evidence(
    tmp_path,
):
    target = tmp_path / "target"
    owned = unit(target)
    sibling = unit(target, name="other-unit")
    debug = unit(target, "debug")
    old = (
        target
        / ".molt_state"
        / "quarantine"
        / "cargo_incremental"
        / "old"
        / "receipt.json"
    )
    old.parent.mkdir(parents=True)
    old.write_text("unique historical evidence")
    receipt = recover(target, (observation(owned),))
    assert receipt.errors == () and receipt.ownership_status == "quarantined"
    assert not owned.exists() and not sibling.exists() and debug.exists()
    assert (
        Path(receipt.quarantine_dir) / "dev-fast" / "incremental" / sibling.name
    ).exists()
    assert old.read_text() == "unique historical evidence"
    assert len(receipt.moved_paths) == 1 and Path(receipt.receipt_path).is_file()
    assert (
        json.loads(Path(receipt.receipt_path).read_text())["ownership_status"]
        == "quarantined"
    )


@pytest.mark.parametrize("lock_name", [".cargo-lock", ".cargo-build-lock"])
def test_active_coordinate_defers_recovery_without_moving_any_units(
    tmp_path, lock_name
):
    target = tmp_path / "target"
    owned = unit(target)
    handle = _try_acquire_file_lock(owned.parent.parent / lock_name)
    assert handle is not None
    try:
        receipt = recover(target, (observation(owned),))
    finally:
        _release_file_lock(handle)
    assert (
        owned.exists()
        and receipt.moved_paths == ()
        and receipt.ownership_status == "deferred"
    )


def test_unverified_filesystem_and_outside_coordinate_never_mutate(
    tmp_path, monkeypatch
):
    target = tmp_path / "target"
    owned = unit(target)
    outside = unit(tmp_path / "foreign")
    assert recover(target, (observation(outside),)).ownership_status == "deferred"
    monkeypatch.setattr(cargo, "_local_cargo_lock_filesystem", lambda _path: False)
    assert recover(target, (observation(owned),)).ownership_status == "deferred"
    assert owned.exists() and outside.exists()


def test_installed_cargo_artifact_lock_conflicts_with_recovery_exclusion(tmp_path):
    from tests.process_guard_common import run_custody_subject_process

    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(
        '[package]\nname="molt-custody-lock-probe"\nversion="0.0.0"\nedition="2021"\n'
    )
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.rs").write_text("pub fn unused() {}")
    target = tmp_path / "target"
    profile = target / "debug"
    profile.mkdir(parents=True)
    handle = _try_acquire_file_lock(profile / ".cargo-lock")
    assert handle is not None
    try:
        with pytest.raises(subprocess.TimeoutExpired) as result:
            run_custody_subject_process(
                [
                    "cargo",
                    "+1.96.1",
                    "check",
                    "--offline",
                    "--manifest-path",
                    str(manifest),
                    "--target-dir",
                    str(target),
                ],
                cwd=tmp_path,
                timeout=2,
                capture_output=True,
                text=True,
            )
        stderr = result.value.stderr or b""
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf8", errors="replace")
        assert "Blocking" in stderr and "artifact directory" in stderr
        assert not (profile / "deps").exists()
    finally:
        _release_file_lock(handle)


def test_release_failure_attempts_all_coordinate_handles(tmp_path, monkeypatch):
    import molt.file_locks as locks

    target = tmp_path / "target"
    owned = unit(target)
    acquire = locks._try_acquire_file_lock
    release = locks._release_file_lock
    acquired = []
    released = []

    def capture(path):
        handle = acquire(path)
        if handle is not None:
            acquired.append(handle)
        return handle

    def release_then_fail_once(handle):
        released.append(handle)
        release(handle)
        if len(released) == 1:
            raise OSError("injected coordinate close failure")

    monkeypatch.setattr(locks, "_try_acquire_file_lock", capture)
    monkeypatch.setattr(locks, "_release_file_lock", release_then_fail_once)
    try:
        receipt = recover(target, (observation(owned),))
        assert len(acquired) == len(released) == 2
        assert all(handle.file.closed for handle in acquired)
        assert receipt.ownership_status == "partial"
        assert len(receipt.moved_paths) == 1
        stored = json.loads(Path(receipt.receipt_path).read_text())
        assert stored["ownership_status"] == "partial"
        assert stored["errors"] == list(receipt.errors)
        assert any(
            "injected coordinate close failure" in error for error in receipt.errors
        )
    finally:
        # The witness never leaves real owned handles held, even if it fails.
        for handle in acquired:
            release(handle)


def test_failed_final_publication_keeps_pending_receipt_fail_closed(
    tmp_path, monkeypatch
):
    target = tmp_path / "target"
    owned = unit(target)
    write = cargo._write_cargo_quarantine_receipt

    def deny_final(*, receipt_path, payload):
        if payload["ownership_status"] != "cleanup_pending":
            raise OSError("injected final receipt failure")
        write(receipt_path=receipt_path, payload=payload)

    monkeypatch.setattr(cargo, "_write_cargo_quarantine_receipt", deny_final)
    receipt = recover(target, (observation(owned),))
    assert receipt.ownership_status == "partial"
    assert len(receipt.moved_paths) == 1
    stored = json.loads(Path(receipt.receipt_path).read_text())
    assert stored["ownership_status"] == "cleanup_pending"
    assert any("injected final receipt failure" in error for error in receipt.errors)


def test_unknown_descendant_closure_never_mutates_profile(tmp_path):
    target = tmp_path / "target"
    owned = unit(target)
    receipt = cargo._quarantine_cargo_incremental_state(
        reason="timeout",
        target_dir=target,
        command=["cargo", "check"],
        cwd=tmp_path,
        observations=(observation(owned),),
        descendants_closed=False,
    )
    assert owned.exists() and receipt.ownership_status == "deferred"
    assert receipt.moved_paths == ()


def test_live_or_unknown_compiler_birth_never_mutates_profile(tmp_path, monkeypatch):
    from tools.memory_guard_core import process_model

    target = tmp_path / "target"
    owned = unit(target)
    for birth in (300, None):
        monkeypatch.setattr(
            process_model,
            "sample_processes",
            lambda: {90051: SimpleNamespace(started_at_ns=birth)},
        )
        receipt = recover(target, (observation(owned),), assume_closed=False)
        assert owned.exists() and receipt.ownership_status == "deferred"
        assert receipt.moved_paths == ()


@pytest.mark.parametrize("profile", ["debug", "incremental"])
def test_recorded_cargo_profile_incremental_root_has_correct_lock_coordinate(
    tmp_path, profile
):
    target = tmp_path / "target with spaces"
    owned = unit(target, profile)
    observed = cargo.CargoIncrementalObservation(
        90051, 300, str(owned.parent), 90050, 200
    )
    assert cargo._observed_incremental_units(target, (observed,)) == {
        owned.parent.resolve(): owned.parent.parent.resolve()
    }
    receipt = recover(target, (observed,))
    assert receipt.ownership_status == "quarantined"
    assert Path(receipt.moved_paths[0].original_path) == owned.parent


def test_planned_receipt_failure_prevents_every_cache_move(tmp_path, monkeypatch):
    target = tmp_path / "target"
    owned = unit(target)

    def fail_write(**kwargs):
        raise OSError("injected initial atomic publication failure")

    monkeypatch.setattr(cargo, "_write_cargo_quarantine_receipt", fail_write)
    receipt = recover(target, (observation(owned),))
    assert owned.exists() and receipt.moved_paths == ()
    assert receipt.ownership_status == "deferred" and receipt.receipt_path is None


def test_partial_profile_move_retains_complete_planned_receipt(tmp_path, monkeypatch):
    target = tmp_path / "target"
    first = unit(target, "debug")
    second = unit(target, "dev-fast")
    rename = Path.rename
    calls = []

    def fail_second_profile(self, destination):
        calls.append(self)
        if len(calls) == 2:
            raise OSError("injected second profile move failure")
        return rename(self, destination)

    monkeypatch.setattr(Path, "rename", fail_second_profile)
    receipt = recover(target, (observation(first), observation(second)))
    assert receipt.ownership_status == "partial" and len(receipt.moved_paths) == 1
    stored = json.loads(Path(receipt.receipt_path).read_text())
    assert stored["ownership_status"] == "partial"
    assert len(stored["planned_paths"]) == 2 and len(stored["moved_paths"]) == 1
    assert second.exists()


def test_reused_parent_birth_never_grants_incremental_observation(tmp_path):
    path = tmp_path / "incremental"
    parent = SimpleNamespace(
        pid=90050, ppid=1, started_at_ns=400, command="cargo", argv=("cargo",)
    )
    child = SimpleNamespace(
        pid=90051,
        ppid=90050,
        started_at_ns=300,
        command="rustc",
        argv=("rustc", "-C", f"incremental={path}"),
    )
    samples = {parent.pid: parent, child.pid: child}
    identities = {pid: process_identity(sample) for pid, sample in samples.items()}
    assert (
        cargo.observe_owned_incremental_state(samples, set(samples), identities)
        == set()
    )


def test_release_interrupt_attempts_all_handles_then_propagates(tmp_path, monkeypatch):
    import molt.file_locks as locks

    target = tmp_path / "target"
    owned = unit(target)
    acquire, release = locks._try_acquire_file_lock, locks._release_file_lock
    acquired, attempts = [], []

    def capture(path):
        handle = acquire(path)
        if handle is not None:
            acquired.append(handle)
        return handle

    def interrupt_once(handle):
        attempts.append(handle)
        release(handle)
        if len(attempts) == 1:
            raise KeyboardInterrupt("injected release interruption")

    monkeypatch.setattr(locks, "_try_acquire_file_lock", capture)
    monkeypatch.setattr(locks, "_release_file_lock", interrupt_once)
    try:
        with pytest.raises(KeyboardInterrupt, match="injected release interruption"):
            recover(target, (observation(owned),))
        assert len(acquired) == len(attempts) == 2
        assert all(handle.file.closed for handle in acquired)
        receipts = list(
            target.glob(".molt_state/quarantine/cargo_incremental/*/receipt.json")
        )
        assert len(receipts) == 1
        assert (
            json.loads(receipts[0].read_text())["ownership_status"] == "cleanup_pending"
        )
    finally:
        for handle in acquired:
            release(handle)


def test_atomic_final_commit_failure_preserves_complete_pending_receipt(
    tmp_path, monkeypatch
):
    import molt.file_publication as publication

    target = tmp_path / "target"
    owned = unit(target)
    replace, calls = publication.durable_replace, []

    def fail_final(staged, destination):
        calls.append(destination)
        if len(calls) == 3:
            assert staged.is_file() and staged.stat().st_size > 0
            raise OSError("injected commit failure after staged write")
        replace(staged, destination)

    monkeypatch.setattr(publication, "durable_replace", fail_final)
    receipt = recover(target, (observation(owned),))
    assert receipt.ownership_status == "partial"
    stored = json.loads(Path(receipt.receipt_path).read_text())
    assert stored["ownership_status"] == "cleanup_pending"
    assert len(stored["moved_paths"]) == 1


def test_actual_cargo_held_profile_locks_defer_recovery(tmp_path):
    import os
    import threading
    import time
    from tests.process_guard_common import run_custody_subject_process

    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(
        '[package]\nname="molt-custody-held-lock"\nversion="0.0.0"\nedition="2021"\n'
    )
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.rs").write_text("pub fn unused() {}")
    ready, release = tmp_path / "ready", tmp_path / "release"
    (tmp_path / "build.rs").write_text(
        'fn main() { let root=std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()); '
        'std::fs::write(root.join("ready"), b"ready").unwrap(); '
        "let start=std::time::Instant::now(); "
        'while !root.join("release").exists() && start.elapsed().as_secs()<25 {'
        "std::thread::sleep(std::time::Duration::from_millis(10));} }"
    )
    target = tmp_path / "target with spaces"
    outcome = []
    env = dict(
        os.environ, CARGO_INCREMENTAL="1", RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS=""
    )

    def run():
        try:
            outcome.append(
                run_custody_subject_process(
                    [
                        "cargo",
                        "+1.96.1",
                        "check",
                        "--offline",
                        "--manifest-path",
                        str(manifest),
                        "--target-dir",
                        str(target),
                    ],
                    cwd=tmp_path,
                    env=env,
                    timeout=40,
                    capture_output=True,
                    text=True,
                )
            )
        except BaseException as exc:
            outcome.append(exc)

    from tools.memory_guard_core import process_model

    tracker = process_model.ProcessTreeTracker(os.getpid())
    tracker.update(process_model.sample_processes())
    actual_observations = set()
    worker = threading.Thread(target=run)
    worker.start()
    try:
        deadline = time.perf_counter() + 25
        while (
            not ready.exists() and worker.is_alive() and time.perf_counter() < deadline
        ):
            samples = process_model.sample_processes()
            watched = tracker.update(samples)
            actual_observations.update(
                cargo.observe_owned_incremental_state(
                    samples, watched, tracker.known_identities or {}
                )
            )
            time.sleep(0.01)
        assert ready.exists(), outcome
        assert actual_observations, (
            "No actual birth-custodied Rustc/Cargo pair observed"
        )
        assert all(
            Path(item.incremental_dir).resolve()
            == (target / "debug" / "incremental").resolve()
            for item in actual_observations
        )
        for name in (".cargo-build-lock", ".cargo-lock"):
            held = _try_acquire_file_lock(target / "debug" / name)
            if held is not None:
                _release_file_lock(held)
            assert held is None, f"real Cargo lock not observed: {name}"
        observed_root = target / "debug" / "incremental"
        assert observed_root.is_dir()
        receipt = recover(
            target,
            tuple(actual_observations),
        )
        assert receipt.ownership_status == "deferred" and receipt.moved_paths == ()
        assert observed_root.is_dir()
        assert any("coordinate is active" in error for error in receipt.errors)
    finally:
        release.write_text("release")
        worker.join(timeout=45)
    assert not worker.is_alive()
    assert len(outcome) == 1 and not isinstance(outcome[0], BaseException)
    assert outcome[0].returncode == 0, outcome[0].stderr
    stored = {
        "cargo_version": "1.96.1",
        "platform": os.name,
        "actual_cargo_returncode": outcome[0].returncode,
        "both_profile_locks_blocked_recovery": True,
        "observations": [
            {
                "rustc_pid": item.rustc_pid,
                "rustc_started_at_ns": item.rustc_started_at_ns,
                "cargo_pid": item.cargo_pid,
                "cargo_started_at_ns": item.cargo_started_at_ns,
                "incremental_dir": item.incremental_dir,
            }
            for item in sorted(actual_observations, key=lambda item: item.rustc_pid)
        ],
    }
    (tmp_path / "actual-cargo-observer-receipt.json").write_text(
        json.dumps(stored, indent=2)
    )


@pytest.mark.parametrize(
    "case", ["manual", "missing", "reused", "unknown", "cycle", "shelltoken"]
)
def test_observer_rejects_unproven_cargo_producer(tmp_path, case):
    child = SimpleNamespace(
        pid=90051,
        ppid=90050,
        started_at_ns=300,
        command="rustc",
        argv=("rustc", "-C", f"incremental={tmp_path}"),
    )
    parent = SimpleNamespace(
        pid=90050,
        ppid=1,
        started_at_ns=200,
        command="cargo check",
        argv=("cargo", "check"),
    )
    if case == "manual":
        parent.argv = ("python",)
    elif case == "reused":
        parent.started_at_ns = 400
    elif case == "unknown":
        parent.argv = ()
    elif case == "cycle":
        parent.ppid = child.pid
        parent.argv = ("python",)
    elif case == "shelltoken":
        parent.argv = ("sh", "-c", "cargo check")
    samples = {child.pid: child, parent.pid: parent}
    identities = {pid: process_identity(item) for pid, item in samples.items()}
    if case == "missing":
        identities.pop(parent.pid)
    assert (
        cargo.observe_owned_incremental_state(samples, set(samples), identities)
        == set()
    )


def test_observed_cargo_instance_must_also_be_closed(tmp_path, monkeypatch):
    from tools.memory_guard_core import process_model

    target = tmp_path / "target"
    owned = unit(target)
    monkeypatch.setattr(
        process_model,
        "sample_processes",
        lambda: {90050: SimpleNamespace(started_at_ns=200)},
    )
    receipt = recover(target, (observation(owned),), assume_closed=False)
    assert receipt.ownership_status == "deferred" and owned.exists()


def test_unproven_cargo_receipt_cannot_authorize_recovery(tmp_path):
    target = tmp_path / "target"
    owned = unit(target)
    receipt = recover(
        target,
        (cargo.CargoIncrementalObservation(90051, 300, str(owned.parent), 0, 0),),
    )
    assert receipt.ownership_status == "deferred" and owned.exists()


@pytest.mark.parametrize("pid", [90050, 90051])
def test_missing_sampler_row_requires_positive_process_closure(
    tmp_path, monkeypatch, pid
):
    from tools.memory_guard_core import process_model

    target = tmp_path / "target"
    owned = unit(target)
    monkeypatch.setattr(process_model, "sample_processes", lambda: {})
    monkeypatch.setattr(
        cargo, "_observed_pid_is_definitely_closed", lambda candidate: candidate != pid
    )
    receipt = recover(target, (observation(owned),), assume_closed=False)
    assert receipt.ownership_status == "deferred" and owned.exists()


@pytest.mark.parametrize("error", [PermissionError("denied"), OSError("unknown")])
def test_posix_omitted_pid_probe_errors_remain_unknown(monkeypatch, error):
    monkeypatch.setattr(cargo.os, "name", "posix")

    def probe(pid, signal):
        assert pid == 90051 and signal == 0
        raise error

    monkeypatch.setattr(cargo.os, "kill", probe)
    assert not cargo._observed_pid_is_definitely_closed(90051)


def test_posix_pid_probe_requires_esrch_not_snapshot_absence(monkeypatch):
    monkeypatch.setattr(cargo.os, "name", "posix")
    monkeypatch.setattr(cargo.os, "kill", lambda pid, signal: None)
    assert not cargo._observed_pid_is_definitely_closed(90051)

    def absent(pid, signal):
        raise ProcessLookupError(3, "ESRCH")

    monkeypatch.setattr(cargo.os, "kill", absent)
    assert cargo._observed_pid_is_definitely_closed(90051)


@pytest.mark.parametrize("pid", [0, -1, True, 0x100000000])
def test_invalid_windows_pid_cannot_prove_absence(monkeypatch, pid):
    monkeypatch.setattr(cargo.os, "name", "nt")
    assert not cargo._observed_pid_is_definitely_closed(pid)


def test_actual_native_current_process_never_counts_closed():
    import os

    assert not cargo._observed_pid_is_definitely_closed(os.getpid())


@pytest.mark.parametrize(
    "found,error,close_ok,expected",
    [
        (False, 18, True, True),
        (False, 5, True, False),
        (False, 87, True, False),
        (False, 18, False, False),
        (True, 18, True, False),
    ],
)
def test_windows_complete_pid_enumeration_is_required(
    monkeypatch, found, error, close_ok, expected
):
    from tools.memory_guard_core import windows_snapshot
    import os

    calls, errors = [], [0]

    class Entry:
        th32ProcessID = os.getpid()

    def next_entry(handle, entry):
        if found and len(calls) == 0:
            calls.append("next")
            entry.th32ProcessID = 90051
            return True
        errors[0] = error
        return False

    api = SimpleNamespace(
        create_snapshot=lambda flags, pid: 111,
        invalid_handle_value=-1,
        ProcessEntry32W=Entry,
        ctypes=SimpleNamespace(
            sizeof=lambda cls: 1,
            byref=lambda entry: entry,
            get_last_error=lambda: errors[0],
            set_last_error=lambda error: errors.__setitem__(0, error),
        ),
        process_first=lambda handle, entry: True,
        process_next=next_entry,
        close_handle=lambda handle: calls.append(handle) or close_ok,
    )
    monkeypatch.setattr(cargo.os, "name", "nt")
    monkeypatch.setattr(windows_snapshot, "_windows_snapshot_api", lambda: api)
    assert cargo._observed_pid_is_definitely_closed(90051) == expected
    assert calls[-1] == 111


def test_posix_oversize_pid_cannot_escape_structured_deferral(monkeypatch):
    monkeypatch.setattr(cargo.os, "name", "posix")

    def forbidden(*args):
        raise AssertionError("invalid PID reached native API")

    monkeypatch.setattr(cargo.os, "kill", forbidden)
    assert not cargo._observed_pid_is_definitely_closed(2**31)


@pytest.mark.parametrize(
    "mode", ["empty", "self_missing", "stale_error", "invalid_snapshot"]
)
def test_windows_ambiguous_snapshot_cannot_authorize_absence(monkeypatch, mode):
    from tools.memory_guard_core import windows_snapshot
    import os

    calls, error = [], [18]

    class Entry:
        th32ProcessID = os.getpid() if mode == "stale_error" else 90050

    def next_entry(handle, entry):
        if mode != "stale_error":
            error[0] = 18
        return False

    api = SimpleNamespace(
        create_snapshot=lambda flags, pid: None if mode == "invalid_snapshot" else 111,
        invalid_handle_value=-1,
        ProcessEntry32W=Entry,
        ctypes=SimpleNamespace(
            sizeof=lambda cls: 1,
            byref=lambda entry: entry,
            get_last_error=lambda: error[0],
            set_last_error=lambda value: error.__setitem__(0, value),
        ),
        process_first=lambda handle, entry: False if mode == "empty" else True,
        process_next=next_entry,
        close_handle=lambda handle: calls.append(handle) or True,
    )
    monkeypatch.setattr(cargo.os, "name", "nt")
    monkeypatch.setattr(windows_snapshot, "_windows_snapshot_api", lambda: api)
    assert not cargo._observed_pid_is_definitely_closed(90051)
    assert calls == ([] if mode == "invalid_snapshot" else [111])


def test_observed_missing_pid_closure_is_deduplicated(tmp_path, monkeypatch):
    from tools.memory_guard_core import process_model

    calls = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: {})
    monkeypatch.setattr(
        cargo,
        "_observed_pid_is_definitely_closed",
        lambda pid: calls.append(pid) or True,
    )
    observations = [
        cargo.CargoIncrementalObservation(90051 + i, 300 + i, str(tmp_path), 90050, 200)
        for i in range(3)
    ]
    assert cargo._observed_compilers_closed(observations)
    assert sorted(calls) == [90050, 90051, 90052, 90053]


def test_completed_compiler_cache_is_not_quarantined_on_later_timeout(tmp_path):
    target = tmp_path / "target"
    owned = unit(target)
    seen = (observation(owned),)
    receipt = cargo._quarantine_cargo_incremental_state(
        reason="timeout",
        target_dir=target,
        command=["cargo", "test"],
        cwd=tmp_path,
        observations=seen,
        descendants_closed=True,
    )
    assert receipt.ownership_status == "deferred" and owned.exists()
    assert receipt.ownership_observations == seen and not receipt.recovery_observations


@pytest.mark.parametrize(
    "wrapper", ["python", "build-script-build", "sccache", "rustc"]
)
def test_unverified_wrapper_cannot_grant_cargo_producer_authority(tmp_path, wrapper):
    samples = {
        90050: SimpleNamespace(
            pid=90050, ppid=1, started_at_ns=200, command="cargo", argv=("cargo",)
        ),
        90051: SimpleNamespace(
            pid=90051, ppid=90050, started_at_ns=250, command=wrapper, argv=(wrapper,)
        ),
        90052: SimpleNamespace(
            pid=90052,
            ppid=90051,
            started_at_ns=300,
            command="rustc",
            argv=("rustc", "-C", f"incremental={tmp_path}"),
        ),
    }
    assert not cargo.observe_owned_incremental_state(
        samples,
        set(samples),
        {pid: process_identity(sample) for pid, sample in samples.items()},
    )


def test_provisional_receipt_retains_complete_plan_on_final_write_failure(
    tmp_path, monkeypatch
):
    target = tmp_path / "target"
    owned = unit(target)
    original = cargo._write_cargo_quarantine_receipt
    calls = []

    def fail_final(**kwargs):
        calls.append(kwargs)
        if len(calls) == 3:
            raise OSError("final publication fault")
        original(**kwargs)

    monkeypatch.setattr(cargo, "_write_cargo_quarantine_receipt", fail_final)
    receipt = recover(target, (observation(owned),))
    stored = json.loads(Path(receipt.receipt_path).read_text())
    assert stored["ownership_status"] == "cleanup_pending"
    assert len(stored["planned_paths"]) == len(stored["moved_paths"]) == 1
    assert len(stored["recovery_observations"]) == 1


def test_cli_cargo_deferral_guidance_preserves_evidence(tmp_path):
    import io
    from tools.memory_guard_core import reporting, process_custody

    receipt = recover(tmp_path, ())
    stream = io.StringIO()
    result = process_custody.GuardResult(
        returncode=124,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        timed_out=True,
        elapsed_s=1.0,
        cargo_incremental_quarantine=receipt,
    )
    reporting.emit_terminal_report(
        result,
        timeout_s=1.0,
        max_rss_gb=1.0,
        max_total_rss_gb=2.0,
        repro_payload=None,
        signal_payload=lambda code: None,
        stderr=stream,
    )
    assert "molt clean --apply" not in stream.getvalue()
    assert "deferral details" in stream.getvalue()
    assert (
        "summary JSON" in stream.getvalue()
        and "no per-quarantine receipt" in stream.getvalue()
    )


@pytest.mark.parametrize("fault", ["none", "denied", "birth", "parent", "close"])
def test_windows_job_command_context_has_single_ordinary_birth_bound_query(
    monkeypatch, fault
):
    import ctypes
    from ctypes import wintypes
    from tools.memory_guard_core import windows_snapshot as native

    calls = []
    closed = []

    class Api:
        def open_process(self, rights, inherit, pid):
            calls.append((rights, inherit, pid))
            return 0 if fault == "denied" else 44

        def get_process_times(self, handle, created, *rest):
            created._obj.dwLowDateTime = 1
            created._obj.dwHighDateTime = 0
            return True

        def close_handle(self, handle):
            closed.append(handle)
            return fault != "close"

    api = Api()
    api.ctypes = ctypes
    api.wintypes = wintypes
    monkeypatch.setattr(native, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(native, "_windows_snapshot_api", lambda: api)
    monkeypatch.setattr(
        native, "_filetime_to_unix_ns", lambda *args: 99 if fault == "birth" else 100
    )
    monkeypatch.setattr(
        native,
        "_snapshot_basic_info",
        lambda *args: SimpleNamespace(
            UniqueProcessId=51,
            InheritedFromUniqueProcessId=-1 if fault == "parent" else 50,
        ),
    )
    monkeypatch.setattr(
        native,
        "_snapshot_command_line",
        lambda *args: r'"C:\tool path\rustc.exe" -C incremental="E:\cache path"',
    )
    result = native.windows_job_command_context(51, 100)
    assert calls == [(0x0410, False, 51)]
    assert closed == ([] if fault == "denied" else [44])
    assert (result is not None) == (fault == "none")
    if result is not None:
        assert result[0] == 50 and "incremental=" in result[1]


@pytest.mark.skipif(
    __import__("os").name != "nt", reason="Actual Windows Job observer coordinate"
)
def test_actual_windows_job_cargo_observer_preserves_completed_cache_on_late_timeout(
    tmp_path, monkeypatch
):
    import os
    from tools import memory_guard

    (tmp_path / "src").mkdir()
    (tmp_path / "Cargo.toml").write_text(
        '[package]\nname="molt-job-custody-probe"\nversion="0.0.0"\nedition="2021"\n'
    )
    (tmp_path / "src/lib.rs").write_text("pub fn answer() -> u32 { 42 }\n")
    (tmp_path / "build.rs").write_text(
        'fn main() { std::fs::write("ready", "ready").unwrap(); std::thread::sleep(std::time::Duration::from_secs(30)); }\n'
    )
    target = tmp_path / "target with spaces"
    env = dict(os.environ)
    for key in (
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_BUILD_DIR",
        "CARGO_BUILD_TARGET",
    ):
        env.pop(key, None)
    env.update(
        CARGO_TARGET_DIR=str(target),
        CARGO_INCREMENTAL="1",
        RUSTFLAGS="",
        CARGO_ENCODED_RUSTFLAGS="",
    )
    observed = set()
    original = memory_guard.observe_owned_incremental_state

    def record(*args, **kwargs):
        result = original(*args, **kwargs)
        observed.update(result)
        return result

    monkeypatch.setattr(memory_guard, "observe_owned_incremental_state", record)
    result = memory_guard.run_guarded(
        [
            "cargo",
            "+1.96.1",
            "check",
            "--offline",
            "--manifest-path",
            str(tmp_path / "Cargo.toml"),
        ],
        max_rss_kb=2_000_000,
        max_total_rss_kb=3_000_000,
        poll_interval=0.015,
        cwd=tmp_path,
        env=env,
        timeout=12,
    )
    assert observed, result.stderr
    assert all(
        Path(x.incremental_dir).resolve() == (target / "debug/incremental").resolve()
        for x in observed
    )
    assert (
        (tmp_path / "ready").exists() and result.timed_out and result.returncode == 124
    )
    receipt = result.cargo_incremental_quarantine
    assert receipt.ownership_status == "deferred" and not receipt.moved_paths
    assert receipt.ownership_observations and not receipt.recovery_observations
    assert (target / "debug/incremental").exists()


@pytest.mark.parametrize(
    "case", ["release", "never_release", "closure_unknown", "acquire_error"]
)
def test_bounded_model_lock_settle_preserves_authority(tmp_path, monkeypatch, case):
    import threading
    import time

    target = tmp_path / "target"
    owned = unit(target)
    lock_path = owned.parent.parent / ".cargo-build-lock"
    handle = _try_acquire_file_lock(lock_path)
    assert handle is not None
    released = threading.Event()

    def release_later():
        time.sleep(0.06)
        _release_file_lock(handle)
        released.set()

    worker = None
    if case != "never_release":
        worker = threading.Thread(target=release_later)
        worker.start()
    if case == "closure_unknown":
        monkeypatch.setattr(cargo, "_observed_compilers_closed", lambda *args: False)
    if case == "acquire_error":
        from molt import file_locks

        original = file_locks._try_acquire_file_lock
        calls = []

        def injected(path):
            calls.append(path)
            if len(calls) == 2:
                raise OSError("native lock query failed")
            return original(path)

        monkeypatch.setattr(file_locks, "_try_acquire_file_lock", injected)
    started = time.perf_counter()
    try:
        obs = (observation(owned),)
        receipt = cargo._quarantine_cargo_incremental_state(
            reason="timeout",
            target_dir=target,
            command=["cargo", "check"],
            cwd=tmp_path,
            observations=obs,
            eligible_observations=frozenset(obs),
            descendants_closed=True,
            profile_lock_settle_s=0.2,
        )
    finally:
        if worker is not None:
            worker.join(2)
            assert not worker.is_alive()
        else:
            _release_file_lock(handle)
    assert time.perf_counter() - started < 2
    if case == "release":
        assert released.is_set() and receipt.ownership_status == "quarantined"
        assert not owned.exists()
    else:
        assert receipt.ownership_status == "deferred" and not receipt.moved_paths
        assert owned.exists()
    for name in (".cargo-lock", ".cargo-build-lock"):
        probe = _try_acquire_file_lock(lock_path.parent / name)
        assert probe is not None
        _release_file_lock(probe)


@pytest.mark.parametrize(
    "style", ["split", "joined", "long_split", "long_joined", "empty", "relative"]
)
def test_observer_uses_effective_final_incremental_option_only(tmp_path, style):
    previous = tmp_path / "previous/incremental"
    effective = tmp_path / "effective/incremental"
    final = {
        "split": ("-C", f"incremental={effective}"),
        "joined": (f"-Cincremental={effective}",),
        "long_split": ("--codegen", f"incremental={effective}"),
        "long_joined": (f"--codegen=incremental={effective}",),
        "empty": ("-C", "incremental="),
        "relative": ("-C", "incremental=relative"),
    }[style]
    child = SimpleNamespace(
        pid=90051,
        ppid=90050,
        started_at_ns=300,
        command="diagnostic",
        argv=("rustc", "-C", f"incremental={previous}", *final),
    )
    parent = SimpleNamespace(
        pid=90050,
        ppid=1,
        started_at_ns=200,
        command="diagnostic",
        argv=("cargo", "check"),
    )
    samples = {child.pid: child, parent.pid: parent}
    actual = cargo.observe_owned_incremental_state(
        samples,
        set(samples),
        {pid: process_identity(item) for pid, item in samples.items()},
    )
    assert actual == (
        set()
        if style in {"empty", "relative"}
        else {cargo.CargoIncrementalObservation(90051, 300, str(effective), 90050, 200)}
    )


def test_lock_rebinding_during_closure_never_mutates_cache(tmp_path, monkeypatch):
    target = tmp_path / "target"
    owned = unit(target)
    checked = False
    original = Path.lstat

    def replaced(path, *args, **kwargs):
        actual = original(path, *args, **kwargs)
        if checked and path.name in {".cargo-lock", ".cargo-build-lock"}:
            return SimpleNamespace(
                st_mode=actual.st_mode, st_dev=actual.st_dev, st_ino=actual.st_ino + 1
            )
        return actual

    def closed(*args):
        nonlocal checked
        checked = True
        return True

    monkeypatch.setattr(cargo, "_observed_compilers_closed", closed)
    monkeypatch.setattr(Path, "lstat", replaced)
    obs = (observation(owned),)
    receipt = cargo._quarantine_cargo_incremental_state(
        reason="timeout",
        target_dir=target,
        command=["cargo", "check"],
        cwd=tmp_path,
        observations=obs,
        eligible_observations=frozenset(obs),
        descendants_closed=True,
    )
    assert (
        receipt.ownership_status == "deferred"
        and not receipt.moved_paths
        and owned.exists()
    )
    assert "identity changed" in receipt.errors[0]


@pytest.mark.parametrize(
    "tail", [("@unexpanded",), ("--", "-C", "incremental=ignored")]
)
def test_response_file_unknown_and_option_terminator_are_not_scanned(tmp_path, tail):
    path = tmp_path / "observed/incremental"
    child = SimpleNamespace(
        pid=90051,
        ppid=90050,
        started_at_ns=300,
        command="diagnostic",
        argv=("rustc", "-C", f"incremental={path}", *tail),
    )
    parent = SimpleNamespace(
        pid=90050,
        ppid=1,
        started_at_ns=200,
        command="diagnostic",
        argv=("cargo", "check"),
    )
    samples = {child.pid: child, parent.pid: parent}
    actual = cargo.observe_owned_incremental_state(
        samples,
        set(samples),
        {pid: process_identity(item) for pid, item in samples.items()},
    )
    assert actual == (
        set()
        if tail[0].startswith("@")
        else {cargo.CargoIncrementalObservation(90051, 300, str(path), 90050, 200)}
    )


def test_actual_native_profile_lock_shared_budget_uses_other_process(
    tmp_path, monkeypatch
):
    """Native lock contention + modeled clock/closure; not native timing proof."""
    import os
    import sys
    import threading
    import queue

    target = tmp_path / "target"
    owned = unit(target)
    obs = (observation(owned),)
    kwargs = dict(
        reason="timeout",
        target_dir=target,
        command=["cargo", "check"],
        cwd=tmp_path,
        observations=obs,
        eligible_observations=frozenset(obs),
        descendants_closed=True,
        profile_lock_settle_s=0.2,
    )
    if not cargo._local_cargo_lock_filesystem(owned.parent.parent):
        receipt = cargo._quarantine_cargo_incremental_state(**kwargs)
        assert receipt.ownership_status == "deferred" and owned.exists()
        return  # Native unsupported admission proved; no positive-platform credit.
    code = "from pathlib import Path; import sys; from molt.file_locks import _try_acquire_file_lock,_release_file_lock; h=_try_acquire_file_lock(Path(sys.argv[1])); assert h is not None; print('ready',flush=True); sys.stdin.readline(); _release_file_lock(h)"
    proc = subprocess.Popen(
        [sys.executable, "-c", code, str(owned.parent.parent / ".cargo-build-lock")],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=dict(os.environ),
    )
    ready = queue.Queue()
    threading.Thread(
        target=lambda: ready.put(proc.stdout.readline()), daemon=True
    ).start()
    try:
        assert ready.get(timeout=10).strip() == "ready"
        elapsed = 0.0
        released = False

        def sleep(delay):
            nonlocal elapsed, released
            elapsed += delay
            if elapsed >= 0.15 and not released:
                released = True
                proc.stdin.write("release\n")
                proc.stdin.flush()
                assert proc.wait(timeout=10) == 0

        monkeypatch.setattr(
            cargo, "time", SimpleNamespace(monotonic=lambda: elapsed, sleep=sleep)
        )
        monkeypatch.setattr(cargo, "_observed_compilers_closed", lambda *args: False)
        receipt = cargo._quarantine_cargo_incremental_state(**kwargs)
        assert receipt.ownership_status == "deferred" and owned.exists()
        assert receipt.admission_telemetry["lock_attempts"] > 2
        assert receipt.admission_telemetry["closure_attempts"] >= 1
        assert receipt.admission_telemetry[
            "total_admission_elapsed_s"
        ] == pytest.approx(0.2)
        assert receipt.admission_telemetry["lock_elapsed_s"] >= 0.15
        assert receipt.admission_telemetry["closure_elapsed_s"] <= 0.05 + 1e-10
    finally:
        if proc.poll() is None:
            proc.terminate()
        proc.wait(timeout=10)
        for stream in (proc.stdin, proc.stdout, proc.stderr):
            stream.close()


def test_darwin_native_argv_decoder_preserves_boundaries_and_raw_bytes():
    import ctypes
    import sys
    from tools.memory_guard_core import process_model as model

    wanted = (
        b"/toolchain with 'quotes'/rustc",
        b"-Cincremental=/cache with spaces",
        b"",
        b"raw-\xff",
    )
    raw = (
        len(wanted).to_bytes(4, sys.byteorder, signed=True)
        + b"/native/exec\0\0"
        + b"\0".join(wanted)
        + b"\0ENV=value\0"
    )
    calls = []

    def sysctl(mib, count, buffer, size, new, newlen):
        calls.append(tuple(mib))
        assert count == 3 and tuple(mib) == (1, 49, 7)
        size._obj.value = len(raw)
        if buffer is not None:
            ctypes.memmove(buffer, raw, len(raw))
        return 0

    authority = model._DarwinProcessAuthority(
        ctypes, None, None, object, lambda *args: 0, sysctl
    )
    assert authority.argv(7) == tuple(
        arg.decode(errors="surrogateescape") for arg in wanted
    )
    assert calls == [(1, 49, 7), (1, 49, 7)]


@pytest.mark.parametrize("failure", [None, "cargo_reuse", "rustc_reuse", "denied"])
def test_darwin_sampler_to_cargo_observer_preserves_native_authority(
    tmp_path, monkeypatch, failure
):
    from tools.memory_guard_core import process_model as model

    incremental = tmp_path / "cache with spaces and 'quotes'" / "debug" / "incremental"
    argv = {
        100: ("/toolchain with 'quotes'/cargo", "check"),
        101: ("/toolchain with 'quotes'/rustc", "-C", f"incremental={incremental}", ""),
    }
    metadata = {100: (1, 100, 1000, "cargo"), 101: (100, 101, 2000, "rustc")}
    calls = {100: 0, 101: 0}

    def birth(pid):
        calls[pid] += 1
        row = metadata[pid]
        if calls[pid] == 2 and failure == (
            "cargo_reuse" if pid == 100 else "rustc_reuse"
        ):
            return (*row[:2], row[2] + 1, row[3])
        return row

    class Authority:
        def argv(self, pid):
            if failure == "denied":
                raise PermissionError("ordinary native argv denied")
            return argv[pid]

    monkeypatch.setattr(model.sys, "platform", "darwin")
    monkeypatch.setattr(model, "_darwin_process_authority_cache", Authority())
    monkeypatch.setattr(model, "_darwin_proc_metadata", birth)
    monkeypatch.setattr(
        model.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            [],
            0,
            "100 1 100 64 Thu Jul 17 07:15:01 2026 cargo\n101 100 101 64 Thu Jul 17 07:15:01 2026 rustc\n",
            "",
        ),
    )
    samples = model.sample_processes_posix()
    identities = {pid: model.ProcessIdentity(row[2]) for pid, row in metadata.items()}
    observed = cargo.observe_owned_incremental_state(samples, set(samples), identities)
    if failure is None:
        assert observed == {
            cargo.CargoIncrementalObservation(101, 2000, str(incremental), 100, 1000)
        }
        assert samples[101].argv == argv[101]
        assert samples[100].argv == argv[100]
        assert cargo._samples_include_cargo_build_state(samples, set(samples))
    else:
        assert not observed
        if failure == "denied":
            assert all(
                sample.argv == () and sample.started_at_ns is None
                for sample in samples.values()
            )


def test_linux_native_sampler_preserves_argv_without_flattening(tmp_path):
    from tools.memory_guard_core import process_model as model

    proc = tmp_path / "123"
    proc.mkdir()
    argv = (b"/toolchain with 'quotes'/rustc", b"-Cincremental=/cache with spaces", b"")
    (proc / "cmdline").write_bytes(b"\0".join(argv) + b"\0")
    (proc / "status").write_text("VmRSS: 40 kB\n")
    samples = model.sample_processes_linux_proc(
        tmp_path, stat_reader=lambda *args: (1, 123, 2000, "rustc"), uptime_sec=1
    )
    assert samples[123].argv == tuple(arg.decode() for arg in argv)


@pytest.mark.parametrize(
    "argv,expected",
    [
        (("/path with 'quotes'/rustc",), True),
        (("/path with 'quotes'/cargo",), True),
        (("/path/rustc.exe",), True),
        (("/path/'rustc'",), False),
        (("/path/python", "cargo"), False),
        ((), False),
    ],
)
def test_native_build_kind_uses_executable_not_flattened_diagnostic(argv, expected):
    sample = SimpleNamespace(argv=argv, command="rustc misleading-diagnostic")
    assert cargo._samples_include_cargo_build_state({7: sample}, {7}) is expected


def test_host_native_launcher_classification_uses_typed_arguments():
    from tools.memory_guard_core import process_model as model

    sample = model.ProcessSample(
        7,
        1,
        1,
        "misleading diagnostic",
        argv=("/prefix with 'quotes'/node", "/scripts with spaces/codex.js"),
    )
    assert model.is_host_control_plane_process(sample)
    changed = model.ProcessSample(
        7, 1, 1, sample.command, argv=("/prefix/python", "unrelated.py")
    )
    assert not model.is_host_control_plane_process(changed)


class _UnformattableRecoveryError(OSError):
    def __str__(self):
        raise AssertionError("exception formatting must not run")


@pytest.mark.parametrize("failure_site", ["release", "initial", "final"])
def test_recovery_diagnostics_cannot_interrupt_cleanup(
    tmp_path, monkeypatch, failure_site
):
    import molt.file_locks as locks

    target = tmp_path / "target"
    owned = unit(target)
    acquire, release = locks._try_acquire_file_lock, locks._release_file_lock
    acquired, attempts = [], []
    publish = cargo._write_cargo_quarantine_receipt
    publications = []

    def capture(path):
        handle = acquire(path)
        if handle is not None:
            acquired.append(handle)
        return handle

    def release_once(handle):
        attempts.append(handle)
        release(handle)
        if failure_site == "release" and len(attempts) == 1:
            raise _UnformattableRecoveryError("injected release failure")

    def failing_publication(**kwargs):
        publications.append(kwargs)
        if failure_site == "initial" or (
            failure_site == "final" and len(publications) == 3
        ):
            raise _UnformattableRecoveryError("injected publication failure")
        return publish(**kwargs)

    monkeypatch.setattr(locks, "_try_acquire_file_lock", capture)
    monkeypatch.setattr(locks, "_release_file_lock", release_once)
    monkeypatch.setattr(cargo, "_write_cargo_quarantine_receipt", failing_publication)
    try:
        receipt = recover(target, (observation(owned),))
        assert len(acquired) == len(attempts) == 2
        assert all(handle.file.closed for handle in acquired)
        assert receipt.ownership_status == (
            "deferred" if failure_site == "initial" else "partial"
        )
        assert any("_UnformattableRecoveryError" in error for error in receipt.errors)
        if failure_site == "initial":
            assert owned.exists() and not receipt.moved_paths
        else:
            assert len(receipt.moved_paths) == 1
    finally:
        for handle in acquired:
            release(handle)


class _DiagnosticTrapMetaclass(type):
    @property
    def __name__(cls):
        raise AssertionError("metaclass name formatting must not run")


class _DiagnosticTrapError(OSError, metaclass=_DiagnosticTrapMetaclass):
    @property
    def args(self):
        raise AssertionError("overridden exception args must not run")

    def __str__(self):
        raise AssertionError("exception formatting must not run")


def test_diagnostic_uses_builtin_descriptors_not_exception_callbacks():
    error = _DiagnosticTrapError("primitive failure detail")
    assert (
        cargo._exception_diagnostic(error)
        == "_DiagnosticTrapError: primitive failure detail"
    )


def test_diagnostic_normalizes_builtin_name_str_subclass_without_callbacks():
    class HostileName(str):
        def __add__(self, other):
            raise AssertionError("name addition callback")

        def __format__(self, spec):
            raise AssertionError("name formatting callback")

        def __str__(self):
            raise AssertionError("name conversion callback")

    class Error(OSError):
        pass

    Error.__name__ = HostileName("NamedError")
    assert (
        cargo._exception_diagnostic(Error("primitive detail"))
        == "NamedError: primitive detail"
    )
