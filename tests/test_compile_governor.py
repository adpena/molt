from __future__ import annotations

from pathlib import Path

import pytest

import tools.compile_governor as compile_governor


@pytest.mark.skipif(
    compile_governor.fcntl is None, reason="compile governor slots require posix flock"
)
def test_compile_slot_allows_high_load_with_available_slot(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(compile_governor, "_count_active_compile_processes", lambda: 0)
    monkeypatch.setattr(compile_governor, "_load_1m", lambda: 999.0)
    env = {
        "MOLT_COMPILE_GUARD_DIR": str(tmp_path / "guard"),
        "MOLT_COMPILE_GUARD_WAIT_SEC": "0.2",
        "MOLT_COMPILE_GUARD_POLL_SEC": "0.05",
        "MOLT_COMPILE_GUARD_MAX_SLOTS": "1",
        "MOLT_COMPILE_GUARD_MAX_LOAD": "1",
    }
    with compile_governor.compile_slot(env=env, label="high-load-free-slot") as lease:
        assert lease.slot_index == 0


@pytest.mark.skipif(
    compile_governor.fcntl is None, reason="compile governor slots require posix flock"
)
def test_compile_slot_enforces_single_slot(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(compile_governor, "_count_active_compile_processes", lambda: 0)
    monkeypatch.setattr(compile_governor, "_load_1m", lambda: 0.0)
    env = {
        "MOLT_COMPILE_GUARD_DIR": str(tmp_path / "guard"),
        "MOLT_COMPILE_GUARD_WAIT_SEC": "0.2",
        "MOLT_COMPILE_GUARD_POLL_SEC": "0.05",
        "MOLT_COMPILE_GUARD_MAX_SLOTS": "1",
        "MOLT_COMPILE_GUARD_MAX_ACTIVE_PROCS": "8",
        "MOLT_COMPILE_GUARD_MAX_LOAD": "999",
    }

    with compile_governor.compile_slot(env=env, label="first"):
        with pytest.raises(
            RuntimeError, match="Timed out waiting for compile capacity"
        ):
            compile_governor.acquire_compile_slot(env=env, label="second")
    lease = compile_governor.acquire_compile_slot(env=env, label="after-release")
    assert lease.slot_index == 0
    lease.release()


def test_compile_slot_can_be_disabled(tmp_path: Path) -> None:
    env = {
        "MOLT_COMPILE_GUARD_DIR": str(tmp_path / "guard"),
        "MOLT_COMPILE_GUARD_ENABLED": "0",
    }
    lease = compile_governor.acquire_compile_slot(env=env, label="disabled")
    assert lease.slot_index is None
    lease.release()


@pytest.mark.skipif(
    compile_governor.fcntl is None, reason="compile governor slots require posix flock"
)
def test_compile_slot_waits_for_active_process_budget(
    tmp_path: Path,
    monkeypatch,
) -> None:
    active_samples = iter((5, 0))
    monkeypatch.setattr(
        compile_governor,
        "_count_active_compile_processes",
        lambda: next(active_samples),
    )
    monkeypatch.setattr(compile_governor, "_load_1m", lambda: 0.0)
    env = {
        "MOLT_COMPILE_GUARD_DIR": str(tmp_path / "guard"),
        "MOLT_COMPILE_GUARD_WAIT_SEC": "1.0",
        "MOLT_COMPILE_GUARD_POLL_SEC": "0.01",
        "MOLT_COMPILE_GUARD_MAX_SLOTS": "2",
        "MOLT_COMPILE_GUARD_MAX_ACTIVE_PROCS": "1",
        "MOLT_COMPILE_GUARD_MAX_LOAD": "999",
    }

    with compile_governor.compile_slot(env=env, label="budgeted") as lease:
        assert lease.slot_index in {0, 1}
        assert lease.waited_seconds >= 0.0
