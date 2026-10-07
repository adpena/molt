from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

from tools import check_cargo_locks


def _fake_cargo(monkeypatch, *, stale: set[str]) -> list[list[str]]:
    calls: list[list[str]] = []

    def run(command, **_kwargs):
        calls.append(list(command))
        if command[0] == "git":
            return SimpleNamespace(
                returncode=0,
                stdout="Cargo.lock\0fuzz/Cargo.lock\0runtime/x/Cargo.lock\0",
                stderr="",
            )
        manifest = Path(command[command.index("--manifest-path") + 1])
        if manifest.parent.name in stale:
            return SimpleNamespace(
                returncode=101,
                stdout="",
                stderr="error: cannot update the lock file because --locked was passed\n"
                "help: refresh it\n",
            )
        return SimpleNamespace(returncode=0, stdout="{}", stderr="")

    monkeypatch.setattr(check_cargo_locks.process_guard, "run_completed_command", run)
    return calls


def test_every_tracked_lockfile_resolves_locked(monkeypatch, tmp_path) -> None:
    calls = _fake_cargo(monkeypatch, stale=set())
    assert check_cargo_locks.stale_lockfiles(tmp_path) == []
    resolved = [call for call in calls if call[0] == "cargo"]
    assert len(resolved) == 3
    assert all("--locked" in call and "--no-deps" not in call for call in resolved)


def test_a_stale_lockfile_is_reported_with_its_cause(monkeypatch, tmp_path) -> None:
    _fake_cargo(monkeypatch, stale={"fuzz"})
    assert check_cargo_locks.stale_lockfiles(tmp_path) == [
        "fuzz/Cargo.lock: error: cannot update the lock file because --locked was passed"
    ]
    assert check_cargo_locks.main(["--root", str(tmp_path)]) == 1
