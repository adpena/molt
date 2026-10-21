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
    # A root that vendors nothing, so only the stale check runs.
    (tmp_path / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    for directory in ("fuzz", "runtime/x"):
        manifest = tmp_path / directory / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text("[workspace]\n", encoding="utf-8")
    _fake_cargo(monkeypatch, stale={"fuzz"})
    assert check_cargo_locks.stale_lockfiles(tmp_path) == [
        "fuzz/Cargo.lock: error: cannot update the lock file because --locked was passed"
    ]
    assert check_cargo_locks.main(["--root", str(tmp_path)]) == 1


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def test_missing_workspace_boundary_fails_before_any_cargo_resolution(
    monkeypatch, tmp_path, capsys
) -> None:
    _write(tmp_path / "Cargo.toml", "[workspace]\nmembers=[]\n")
    lock = _write(tmp_path / "standalone/Cargo.lock", "version=4\n")
    _write(tmp_path / "standalone/Cargo.toml", '[package]\nname="standalone"\n')
    monkeypatch.setattr(check_cargo_locks, "tracked_lockfiles", lambda _root: [lock])

    def no_cargo(*_args, **_kwargs):
        raise AssertionError("Cargo must not resolve an unbounded project")

    monkeypatch.setattr(
        check_cargo_locks.process_guard,
        "run_completed_command",
        no_cargo,
    )
    assert check_cargo_locks.main(["--root", str(tmp_path)]) == 1
    assert "explicit [workspace]" in capsys.readouterr().err


def _vendored_repo(tmp_path: Path, workspace_patch: str, locked: str) -> Path:
    """A root that vendors object 0.40.0, and one standalone workspace `ws`."""
    _write(
        tmp_path / "Cargo.toml",
        '[workspace]\nmembers = []\n[patch.crates-io]\nobject = { path = "vendor/rust/object" }\n',
    )
    _write(
        tmp_path / "vendor/rust/object/Cargo.toml",
        '[package]\nname = "object"\nversion = "0.40.0"\n',
    )
    _write(tmp_path / "ws/Cargo.toml", f"[workspace]\n{workspace_patch}")
    return _write(tmp_path / "ws/Cargo.lock", f"version = 4\n{locked}")


_REGISTRY = 'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
_PATCH = '[patch.crates-io]\nobject = {{ path = "{path}" }}\n'


def test_a_standalone_workspace_must_patch_the_vendored_crates(
    monkeypatch, tmp_path
) -> None:
    cases = {
        # crates.io's copy of a version the vendored patch covers: the defect.
        "registry": (
            "",
            f'[[package]]\nname = "object"\nversion = "0.40.1"\n{_REGISTRY}',
            [
                "ws/Cargo.lock: object 0.40.1 resolves from "
                "registry+https://github.com/rust-lang/crates.io-index, "
                "not vendor/rust/object"
            ],
        ),
        "patched": (
            _PATCH.format(path="../vendor/rust/object"),
            '[[package]]\nname = "object"\nversion = "0.40.0"\n',
            [],
        ),
        "other_directory": (
            _PATCH.format(path="../elsewhere/object"),
            '[[package]]\nname = "object"\nversion = "0.40.0"\n',
            ["ws/Cargo.lock: object 0.40.0 is not patched to vendor/rust/object"],
        ),
        # Cargo cannot patch another caret range, so 0.39 stays on crates.io.
        "other_range": (
            "",
            f'[[package]]\nname = "object"\nversion = "0.39.1"\n{_REGISTRY}',
            [],
        ),
    }
    for name, (workspace_patch, locked, expected) in cases.items():
        root = tmp_path / name
        lockfile = _vendored_repo(root, workspace_patch, locked)
        monkeypatch.setattr(
            check_cargo_locks, "tracked_lockfiles", lambda _root, lock=lockfile: [lock]
        )
        assert check_cargo_locks.unpatched_vendored_crates(root) == expected, name


def test_an_unpatched_workspace_fails_before_cargo_runs(monkeypatch, tmp_path) -> None:
    lockfile = _vendored_repo(
        tmp_path, "", f'[[package]]\nname = "object"\nversion = "0.40.0"\n{_REGISTRY}'
    )
    monkeypatch.setattr(
        check_cargo_locks, "tracked_lockfiles", lambda _root: [lockfile]
    )
    calls = _fake_cargo(monkeypatch, stale=set())
    assert check_cargo_locks.main(["--root", str(tmp_path)]) == 1
    assert calls == []


def test_caret_buckets_follow_cargo_compatibility() -> None:
    bucket = check_cargo_locks._caret_bucket
    assert bucket("1.2.3") == bucket("1.9.0") != bucket("2.0.0")
    assert bucket("0.40.0") == bucket("0.40.7") != bucket("0.41.0")
    assert bucket("0.0.3") != bucket("0.0.4")
