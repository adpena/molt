from __future__ import annotations

import importlib.util
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_CLEANUP = REPO_ROOT / "tools" / "artifact_cleanup.py"


def _load_artifact_cleanup():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_artifact_cleanup", ARTIFACT_CLEANUP
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_default_pathspecs_exclude_stateful_roots() -> None:
    module = _load_artifact_cleanup()
    defaults = set(module.default_pathspecs())

    for pathspec in module.stateful_pathspecs():
        assert pathspec not in defaults


def test_default_pathspecs_avoid_recursive_globs() -> None:
    module = _load_artifact_cleanup()

    assert all("**" not in pathspec for pathspec in module.default_pathspecs())


def test_git_clean_command_is_dry_run_by_default() -> None:
    module = _load_artifact_cleanup()

    cmd = module.build_git_clean_command(
        apply=False,
        pathspecs=["target/", "tmp/"],
    )

    assert cmd == ["git", "clean", "-ndX", "--", "target/", "tmp/"]


def test_git_clean_command_apply_uses_delete_mode() -> None:
    module = _load_artifact_cleanup()

    cmd = module.build_git_clean_command(
        apply=True,
        pathspecs=["target/", "tmp/"],
    )

    assert cmd == ["git", "clean", "-fdX", "--", "target/", "tmp/"]


def test_main_dry_run_invokes_git_clean_without_process_kill(monkeypatch) -> None:
    module = _load_artifact_cleanup()
    calls: list[list[str]] = []

    def fake_run(cmd, **kwargs):
        calls.append(list(cmd))
        assert kwargs["cwd"] == module.REPO_ROOT
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(module.subprocess, "run", fake_run)

    rc = module.main([])

    assert rc == 0
    assert calls == [
        module.build_git_clean_command(
            apply=False,
            pathspecs=module.default_pathspecs(),
        )
    ]


def test_main_apply_accepts_sentinel_kill_report(monkeypatch) -> None:
    module = _load_artifact_cleanup()
    calls: list[list[str]] = []

    def fake_run(cmd, **kwargs):
        calls.append(list(cmd))
        assert kwargs["cwd"] == module.REPO_ROOT
        rc = 1 if "process_sentinel.py" in cmd[1] else 0
        return subprocess.CompletedProcess(cmd, rc)

    monkeypatch.setattr(module.subprocess, "run", fake_run)

    rc = module.main(["--apply", "--kill-processes"])

    assert rc == 0
    assert calls[0][1].endswith("tools/process_sentinel.py")
    assert calls[1] == module.build_git_clean_command(
        apply=True,
        pathspecs=module.default_pathspecs(),
    )
