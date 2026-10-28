"""Readers share terminal evidence; disk protection also observes live custody."""

from __future__ import annotations

import json
import os
from types import SimpleNamespace

import pytest

from tests.process_guard_common import current_thread_only, install_module_view
from molt import process_model
from molt import pytest_memory_guard_bootstrap as bootstrap
from tools import disk_guard, runtime_wasm_final_preflight as preflight
from tools.memory_guard_core import active_custody as custody
from tools.proof_queue_pkg import diagnostic_evidence as diagnostic


def _reader_snapshot():
    pid = os.getpid()
    return {
        pid: SimpleNamespace(pid=pid, started_at_ns=999, pgid=None, elapsed_sec=None)
    }


@pytest.fixture
def live_guard_processes(monkeypatch):
    # A complete fixture process table must include its own reader, or disk
    # guard correctly preserves custody as unavailable instead of observing it.
    processes = _reader_snapshot()
    processes[10] = SimpleNamespace(
        pid=10, started_at_ns=100, pgid=None, elapsed_sec=None
    )
    monkeypatch.setattr(
        process_model,
        "sample_processes",
        current_thread_only(lambda: dict(processes), process_model.sample_processes),
    )
    return processes


def _launch(tmp_path):
    state = tmp_path / "tmp" / "memory_guard"
    marker = state / "active" / f"guard-10-{'a' * 32}.json"
    target = tmp_path / "target" / "codex-fixture"
    target.mkdir(parents=True)
    command = ["cargo", "build"]
    payload = {
        "schema_version": 2,
        "pid": 10,
        "token": "a" * 32,
        "guard_process": {"pid": 10, "started_at_ns": 100},
        "child_process": {"pid": 20, "started_at_ns": 200, "pgid": 20},
        "child_launch_state": "recorded",
        "status": "child_running",
        "command": command,
        "created_at": "2026-10-03T00:00:00Z",
        "path": str(bootstrap.SOURCE_ROOT / "tools" / "memory_guard.py"),
        "artifact_scope": {
            "checkout": str(bootstrap.SOURCE_ROOT),
            "paths": [str(target)],
        },
    }
    custody.write_active_guard_marker(marker, payload)
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": payload["token"],
        "MOLT_MEMORY_GUARD_MARKER": str(marker),
    }
    summary = {
        "command": command,
        "recorded_at": payload["created_at"],
        "repro": {"env": env},
    }
    return marker, env, summary


def _queue_state(tmp_path, summary):
    return diagnostic._summary_guard_marker(
        {"cwd": str(tmp_path)}, summary, guard_pid=10
    )[0]


def test_reconciled_marker_is_terminal_for_every_reader(tmp_path, live_guard_processes):
    marker, env, summary = _launch(tmp_path)
    assert bootstrap._active_guard_marker_valid(env, guard_pid=10)
    protection = disk_guard._guard_protection((tmp_path,))
    assert protection.protects(tmp_path / "target/codex-fixture", tmp_path)
    assert len(protection.scopes) == 1
    assert protection.scopes[0].guard.state == "live"
    assert custody.has_active_guard_marker(marker.parent)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))

    live_guard_processes.clear()
    live_guard_processes.update(_reader_snapshot())
    custody.reconcile_active_guard_markers(
        marker.parent,
        lambda: dict(live_guard_processes),
        apply=True,
    )
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert not bool(disk_guard._guard_protection((tmp_path,)).scopes)
    assert not custody.has_active_guard_marker(marker.parent)
    assert _queue_state(tmp_path, summary) == "terminal"
    assert (
        preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))
        == []
    )


@pytest.mark.parametrize(
    "status", ["spawn_failed", "guard_exception", "finalizer_cleanup"]
)
def test_failed_or_unfinished_guard_cannot_claim_terminal_closure(
    tmp_path, status, live_guard_processes
):
    marker, env, summary = _launch(tmp_path)
    custody.update_active_guard_marker(
        marker, env["MOLT_MEMORY_GUARD_TOKEN"], status=status
    )
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    protection = disk_guard._guard_protection((tmp_path,))
    assert protection.protects(tmp_path / "target/codex-fixture", tmp_path)
    assert len(protection.scopes) == 1
    assert protection.scopes[0].guard.state == "live"
    assert custody.has_active_guard_marker(marker.parent)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))


@pytest.mark.parametrize("guard_alive", [False, True])
def test_malformed_terminal_is_rejected_by_every_reader(
    tmp_path, guard_alive, live_guard_processes
):
    marker, env, summary = _launch(tmp_path)
    payload = json.loads(marker.read_text(encoding="utf-8"))
    payload.update(status="completed", token="wrong")
    marker.write_text(json.dumps(payload), encoding="utf-8")
    if not guard_alive:
        live_guard_processes.clear()
        live_guard_processes.update(_reader_snapshot())
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    protection = disk_guard._guard_protection((tmp_path,))
    assert protection.protects(tmp_path / "target/codex-fixture", tmp_path)
    assert len(protection.scopes) == 1
    assert protection.scopes[0].guard.state == "ambiguous"
    assert protection.scopes[0].guard.reason.startswith("marker_invalid:")
    assert custody.has_active_guard_marker(marker.parent)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset())


def test_stale_orphan_does_not_protect_disk_or_publish_terminal_evidence(
    tmp_path, live_guard_processes
):
    marker, env, summary = _launch(tmp_path)
    before = marker.read_bytes()
    live_guard_processes.clear()
    live_guard_processes.update(_reader_snapshot())

    protection = disk_guard._guard_protection((tmp_path,))
    assert not protection.scopes
    assert not protection.protects(tmp_path / "target/codex-fixture", tmp_path)
    assert marker.read_bytes() == before, "observation must not reconcile a record"
    assert not custody.read_marker_record(marker).terminal
    assert custody.has_active_guard_marker(marker.parent)
    assert bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset()) == []


def test_preflight_unreadable_marker_directory_remains_a_conflict(
    tmp_path, monkeypatch
):
    marker, _, _ = _launch(tmp_path)
    original = os.scandir

    def unreadable(path):
        if path == marker.parent:
            raise PermissionError("unreadable custody")
        return original(path)

    install_module_view(monkeypatch, "os", os, custody, scandir=unreadable)
    conflicts = preflight._active_build_guards((marker.parent,), live_pids=frozenset())
    assert len(conflicts) == 1
    assert "unreadable custody" in conflicts[0]["error"]


@pytest.mark.parametrize("registration_fails", [False, True])
def test_pytest_config_owns_only_actual_managed_scratch_lifetime(
    tmp_path, monkeypatch, registration_fails
):
    from molt.checkout_state import (
        checkout_scratch_state_dir,
        checkout_state_retirement,
    )

    checkout = tmp_path / "project"
    checkout.mkdir()
    artifact = tmp_path / "artifacts"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact))
    monkeypatch.setattr(bootstrap, "ROOT", checkout)
    cache = checkout_scratch_state_dir("pytest-cache", checkout, os.environ)
    home = checkout_scratch_state_dir("hypothesis-home", checkout, os.environ)
    from _pytest.config import get_config

    config = get_config()
    config.parse(["-o", f"cache_dir={cache}", str(checkout)])
    if registration_fails:

        def refuse(cleanup):
            raise RuntimeError("independent cleanup registration failure")

        monkeypatch.setattr(config, "add_cleanup", refuse)
    try:
        if registration_fails:
            with pytest.raises(RuntimeError, match="independent cleanup registration"):
                bootstrap._hold_pytest_checkout_state(config, ())
        else:
            bootstrap._hold_pytest_checkout_state(config, ())
        for root in (cache, home):
            with checkout_state_retirement(root) as admitted:
                assert admitted is registration_fails
    finally:
        config._ensure_unconfigure()
    for root in (cache, home):
        with checkout_state_retirement(root) as admitted:
            assert admitted
