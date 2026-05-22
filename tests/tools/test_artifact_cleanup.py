from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace


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


def test_extra_pathspecs_reject_stateful_roots() -> None:
    module = _load_artifact_cleanup()

    for pathspec in [".venv/", ".omx/cache", "third_party/tool"]:
        try:
            module.validate_extra_pathspecs([pathspec])
        except ValueError as exc:
            assert "stateful data" in str(exc)
        else:
            raise AssertionError(f"{pathspec} should have been rejected")


def test_extra_pathspecs_reject_nonliteral_paths() -> None:
    module = _load_artifact_cleanup()

    for pathspec in ["/tmp/cache", "../tmp", "tests/**/__pycache__/", ":(glob)tmp/*"]:
        try:
            module.validate_extra_pathspecs([pathspec])
        except ValueError:
            pass
        else:
            raise AssertionError(f"{pathspec} should have been rejected")


def test_repo_root_must_be_this_checkout(tmp_path: Path) -> None:
    module = _load_artifact_cleanup()
    other_repo = tmp_path / "other"
    other_repo.mkdir()
    (other_repo / "pyproject.toml").write_text("[project]\nname = 'molt'\n")

    try:
        module.validate_repo_root(other_repo)
    except ValueError as exc:
        assert "not this Molt checkout" in str(exc)
    else:
        raise AssertionError("foreign repo root should have been rejected")


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
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main([])

    assert rc == 0
    assert calls[0]["cmd"] == module.build_git_clean_command(
        apply=False,
        pathspecs=module.default_pathspecs(),
    )
    assert calls[0]["prefix"] == "MOLT_DEV_CLEANUP"
    assert calls[0]["cwd"] == module.REPO_ROOT
    assert calls[0]["capture_output"] is False
    assert calls[0]["env"]["MOLT_EXT_ROOT"] == str(module.REPO_ROOT)


def test_main_rejects_stateful_extra_before_git_clean(monkeypatch) -> None:
    module = _load_artifact_cleanup()

    def fail_run(*_args, **_kwargs):
        raise AssertionError("git clean must not run for rejected extra pathspecs")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fail_run,
    )

    rc = module.main(["--extra-path", ".venv/"])

    assert rc == 2


def test_main_apply_accepts_sentinel_kill_report(monkeypatch) -> None:
    module = _load_artifact_cleanup()
    calls: list[list[str]] = []

    def fake_guarded_completed_process(cmd, **_kwargs):
        calls.append(list(cmd))
        rc = 1 if "process_sentinel.py" in cmd[1] else 0
        return SimpleNamespace(returncode=rc)

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--apply", "--kill-processes"])

    assert rc == 0
    assert calls[0][1].endswith("tools/process_sentinel.py")
    assert calls[1] == module.build_git_clean_command(
        apply=True,
        pathspecs=module.default_pathspecs(),
    )
