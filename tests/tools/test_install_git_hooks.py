"""Tests for tools/install_git_hooks.py — the idempotent pre-push drift-gate installer.

Pins the invariants that keep the gate deployable without breaking commits:
idempotence, --check semantics, foreign-hook preservation+chaining, uninstall
restore, and that we install into .git/hooks (NOT via core.hooksPath, which would
enable the pre-existing pre-commit type-check and block every commit).
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import tools.install_git_hooks as ig


def _git_init(path: Path) -> None:
    subprocess.run(["git", "init", "-q", str(path)], check=True)


def _fake_source(tmp_path: Path) -> Path:
    src = tmp_path / ".githooks" / "pre-push"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_text(
        "#!/usr/bin/env bash\n# molt-drift-gate-hook v1\necho gate; exit 0\n",
        encoding="utf-8",
    )
    return src


def test_is_molt_hook_and_chained_wrapper():
    assert ig._is_molt_hook("# molt-drift-gate-hook v1\n")
    assert not ig._is_molt_hook("#!/bin/sh\necho other\n")
    wrapped = ig._chained_wrapper("#!/usr/bin/env bash\n# molt-drift-gate-hook v1\nbody\n")
    # shebang stays first; the preserved foreign hook is invoked before the gate body
    assert wrapped.startswith("#!/usr/bin/env bash\n")
    assert "pre-push.local" in wrapped
    assert wrapped.index("pre-push.local") < wrapped.index("body")


def test_install_idempotent_and_check(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "SOURCE", _fake_source(tmp_path))

    target = repo / ".git" / "hooks" / "pre-push"
    # Not installed yet -> --check fails.
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 1
    # Install.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert ig._is_molt_hook(target.read_text(encoding="utf-8"))
    # Idempotent: re-run is a no-op success, and --check now passes.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 0


def test_foreign_hook_preserved_and_chained_then_restored(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "SOURCE", _fake_source(tmp_path))

    hooks = repo / ".git" / "hooks"
    hooks.mkdir(parents=True, exist_ok=True)
    foreign = hooks / "pre-push"
    foreign.write_text("#!/bin/sh\necho FOREIGN\nexit 0\n", encoding="utf-8")

    # Installing over a foreign hook preserves it as pre-push.local and chains it.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    installed = (hooks / "pre-push").read_text(encoding="utf-8")
    assert ig._is_molt_hook(installed)
    assert "pre-push.local" in installed
    preserved = (hooks / "pre-push.local").read_text(encoding="utf-8")
    assert "FOREIGN" in preserved

    # Uninstall restores the foreign hook.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert "FOREIGN" in (hooks / "pre-push").read_text(encoding="utf-8")
    assert not (hooks / "pre-push.local").exists()


def test_uninstall_noop_when_absent(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "SOURCE", _fake_source(tmp_path))
    # Nothing installed -> uninstall is a clean no-op.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert not (repo / ".git" / "hooks" / "pre-push").exists()
