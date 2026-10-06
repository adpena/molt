"""Tests for tools/install_git_hooks.py — the idempotent managed-hook installer.

Pins the invariants that keep the gate deployable without breaking commits:
idempotence, --check semantics, foreign-hook preservation+chaining, uninstall
restore, and that we install into .git/hooks (NOT via core.hooksPath, which would
enable the pre-existing pre-commit type-check and block every commit).
"""

from __future__ import annotations

import os
from collections.abc import Sequence
from pathlib import Path

import pytest

from tools.agent_coordination import choose_bash
import tools.install_git_hooks as ig
from tests.process_guard_common import run_guarded_test_process


HOOKS_DIR = Path(__file__).resolve().parents[2] / ".githooks"
HOOK = HOOKS_DIR / "pre-push"
COMMIT_MSG_HOOK = HOOKS_DIR / "commit-msg"
LAUNCH = HOOKS_DIR / "molt-hook-launch.sh"
PRE_PUSH = ig.HOOKS[0]
_HOOK_RUNNER = r"""
case "$OSTYPE" in
  msys*|cygwin*)
    fake_bin="$(cygpath -u "$1")" || exit 2
    hook="$(cygpath -u "$2")" || exit 2
    export FAKE_REPO_ROOT="$(cygpath -m "$3")" || exit 2
    export FAKE_COMMON_DIR="$(cygpath -m "$4")" || exit 2
    export FAKE_UV_CAPTURE="$(cygpath -m "$5")" || exit 2
    selected_bash="$(cygpath -u "$6")" || exit 2
    ;;
  *)
    fake_bin="$1"
    hook="$2"
    export FAKE_REPO_ROOT="$3"
    export FAKE_COMMON_DIR="$4"
    export FAKE_UV_CAPTURE="$5"
    selected_bash="$6"
    ;;
esac
export PATH="$fake_bin:$PATH"
export PYTHONHOME="foreign-python-home"
export PYTHONNOUSERSITE="0"
export PYTHONPATH="foreign-python-path"
shift 6
exec "$selected_bash" "$hook" "$@"
"""


def _git_init(path: Path) -> None:
    run_guarded_test_process(["git", "init", "-q", str(path)], check=True)


def _fake_sources(tmp_path: Path) -> Path:
    """One fake source per managed hook, carrying that hook's marker."""
    directory = tmp_path / ".githooks"
    directory.mkdir(parents=True, exist_ok=True)
    for hook in ig.HOOKS:
        (directory / hook.name).write_text(
            f"#!/usr/bin/env bash\n# {hook.marker} v1\necho {hook.name}; exit 0\n",
            encoding="utf-8",
        )
    return directory


def _write_executable(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    path.chmod(0o755)


def _hook_fixture(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    worktree = tmp_path / "selected worktree"
    main = tmp_path / "common checkout"
    fake_bin = tmp_path / "fake bin"
    capture = tmp_path / "uv launch.txt"
    (worktree / "tools").mkdir(parents=True)
    for tool in ("drift_harvest.py", "check_commit_attribution.py"):
        (worktree / "tools" / tool).write_text(
            "raise AssertionError('fake uv must not execute the tool')\n",
            encoding="utf-8",
        )
    (worktree / ".githooks").mkdir()
    (worktree / ".githooks" / LAUNCH.name).write_text(
        LAUNCH.read_text(encoding="utf-8"), encoding="utf-8", newline="\n"
    )
    (main / ".git").mkdir(parents=True)
    _write_executable(
        fake_bin / "git",
        """#!/usr/bin/env bash
case "$*" in
  "rev-parse --show-toplevel") printf '%s\\n' "$FAKE_REPO_ROOT" ;;
  "rev-parse --git-common-dir") printf '%s\\n' "$FAKE_COMMON_DIR" ;;
  *) exit 2 ;;
esac
""",
    )
    _write_executable(
        fake_bin / "uv",
        """#!/usr/bin/env bash
{
  printf '%s\\n' "$PYTHONPATH"
  printf '%s\\n' "${PYTHONHOME-<unset>}"
  printf '%s\\n' "$PYTHONNOUSERSITE"
  for arg in "$@"; do printf '%s\\n' "$arg"; done
} > "$FAKE_UV_CAPTURE"
""",
    )
    return worktree, main, fake_bin, capture


def _run_hook(
    *,
    worktree: Path,
    main: Path,
    fake_bin: Path,
    capture: Path,
    hook: Path = HOOK,
    args: Sequence[str] = (),
):
    bash = choose_bash()
    if bash is None:
        pytest.skip("a non-WSL Bash is required to exercise the Git hook")
    return run_guarded_test_process(
        [
            bash,
            "-c",
            _HOOK_RUNNER,
            "molt-hook-test",
            str(fake_bin),
            str(hook),
            str(worktree),
            str(main / ".git"),
            str(capture),
            bash,
            *args,
        ],
        check=False,
    )


def test_is_molt_hook_and_chained_wrapper():
    assert ig._is_molt_hook("# molt-drift-gate-hook v1\n", PRE_PUSH)
    assert not ig._is_molt_hook("#!/bin/sh\necho other\n", PRE_PUSH)
    wrapped = ig._chained_wrapper(
        "#!/usr/bin/env bash\n# molt-drift-gate-hook v1\nbody\n", PRE_PUSH
    )
    # shebang stays first; the preserved foreign hook is invoked before the gate body
    assert wrapped.startswith("#!/usr/bin/env bash\n")
    assert "pre-push.local" in wrapped
    assert wrapped.index("pre-push.local") < wrapped.index("body")


def test_install_idempotent_and_check(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", _fake_sources(tmp_path))

    # Not installed yet -> --check fails.
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 1
    # Install every managed hook.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    for hook in ig.HOOKS:
        target = repo / ".git" / "hooks" / hook.name
        assert ig._is_molt_hook(target.read_text(encoding="utf-8"), hook)
    # Idempotent: re-run is a no-op success, and --check now passes.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 0


def test_foreign_hook_preserved_and_chained_then_restored(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", _fake_sources(tmp_path))

    hooks = repo / ".git" / "hooks"
    hooks.mkdir(parents=True, exist_ok=True)
    foreign = hooks / "pre-push"
    foreign.write_text("#!/bin/sh\necho FOREIGN\nexit 0\n", encoding="utf-8")

    # Installing over a foreign hook preserves it as pre-push.local and chains it.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    installed = (hooks / "pre-push").read_text(encoding="utf-8")
    assert ig._is_molt_hook(installed, PRE_PUSH)
    assert "pre-push.local" in installed
    preserved = (hooks / "pre-push.local").read_text(encoding="utf-8")
    assert "FOREIGN" in preserved

    # Refreshes must retain the preserved hook, not replace the chain with a
    # bare drift gate merely because the installed wrapper has our marker.
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 0
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert (hooks / "pre-push").read_text(encoding="utf-8") == installed
    PRE_PUSH.source.write_text(
        PRE_PUSH.source.read_text(encoding="utf-8") + "# revised gate\n",
        encoding="utf-8",
    )
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 1
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    refreshed = (hooks / "pre-push").read_text(encoding="utf-8")
    assert "pre-push.local" in refreshed and "# revised gate" in refreshed
    assert (hooks / "pre-push.local").read_text(encoding="utf-8") == preserved

    # Uninstall restores the foreign hook.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert "FOREIGN" in (hooks / "pre-push").read_text(encoding="utf-8")
    assert not (hooks / "pre-push.local").exists()


def test_uninstall_noop_when_absent(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", _fake_sources(tmp_path))
    # Nothing installed -> uninstall is a clean no-op.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert not (repo / ".git" / "hooks" / "pre-push").exists()


def test_hooks_bind_worktree_startup_before_uv_without_project_sync() -> None:
    launch = LAUNCH.read_text(encoding="utf-8")
    # One launcher binds startup imports to the invoking worktree, then uv runs.
    assert launch.index("export PYTHONPATH=") < launch.index("molt_hook_uv_python()")
    assert 'PYTHONPATH="$python_root/src;$python_root"' in launch
    assert 'PYTHONPATH="$repo_root/src:$repo_root"' in launch
    assert 'cygpath -m "$repo_root"' in launch
    assert "unset PYTHONHOME" in launch
    uv_calls = [
        line.strip()
        for line in launch.splitlines()
        if line.strip().startswith("uv run ")
    ]
    assert uv_calls == [
        'uv run --no-project --offline --no-config --python "$vpy" python "$@"'
    ]
    assert 'vpy="$main_root/.venv/Scripts/python.exe"' in launch
    assert 'vpy="$main_root/.venv/bin/python"' in launch
    assert '"$repo_root/.venv/' not in launch
    # Each hook sources that launcher and starts no Python of its own.
    for hook, call in (
        (HOOK, 'molt_hook_uv_python pre-push "$gate" --gate --no-fetch'),
        (
            COMMIT_MSG_HOOK,
            'molt_hook_uv_python commit-msg "$checker" --message-file "$1"',
        ),
    ):
        source = hook.read_text(encoding="utf-8")
        assert '. "$launch"' in source
        assert call in source
        assert not any(
            line.strip().startswith(("uv run ", "python "))
            for line in source.splitlines()
        )


def test_hook_executes_uv_with_selected_worktree_startup_authority(
    tmp_path: Path,
) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    if os.name == "nt":
        native_python = main / ".venv" / "Scripts" / "python.exe"
        path_separator = ";"
    else:
        native_python = main / ".venv" / "bin" / "python"
        path_separator = ":"
    _write_executable(native_python, "#!/usr/bin/env bash\nexit 99\n")

    completed = _run_hook(
        worktree=worktree,
        main=main,
        fake_bin=fake_bin,
        capture=capture,
    )

    assert completed.returncode == 0, completed.stderr
    lines = capture.read_text(encoding="utf-8").splitlines()
    expected_root = worktree.as_posix() if os.name == "nt" else str(worktree)
    assert lines[:3] == [
        f"{expected_root}/src{path_separator}{expected_root}",
        "<unset>",
        "1",
    ]
    argv = lines[3:]
    assert argv[:5] == [
        "run",
        "--no-project",
        "--offline",
        "--no-config",
        "--python",
    ]
    if os.name == "nt":
        assert (
            argv[5]
            .replace("\\", "/")
            .endswith("/common checkout/.venv/Scripts/python.exe")
        )
    else:
        assert argv[5] == str(native_python)
    assert argv[6:] == [
        "python",
        f"{expected_root}/tools/drift_harvest.py",
        "--gate",
        "--no-fetch",
    ]


def test_hook_rejects_wrong_platform_interpreter_without_uv_fallback(
    tmp_path: Path,
) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    wrong_platform_python = (
        main / ".venv" / "bin" / "python"
        if os.name == "nt"
        else main / ".venv" / "Scripts" / "python.exe"
    )
    _write_executable(wrong_platform_python, "#!/usr/bin/env bash\nexit 99\n")

    completed = _run_hook(
        worktree=worktree,
        main=main,
        fake_bin=fake_bin,
        capture=capture,
    )

    assert completed.returncode == 1
    assert "provision the common checkout's platform-native uv environment" in (
        completed.stderr
    )
    assert not capture.exists()


def test_commit_msg_hook_runs_the_attribution_checker_on_the_message(
    tmp_path: Path,
) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    native_python = (
        main / ".venv" / "Scripts" / "python.exe"
        if os.name == "nt"
        else main / ".venv" / "bin" / "python"
    )
    _write_executable(native_python, "#!/usr/bin/env bash\nexit 99\n")

    completed = _run_hook(
        worktree=worktree,
        main=main,
        fake_bin=fake_bin,
        capture=capture,
        hook=COMMIT_MSG_HOOK,
        args=(".git/COMMIT_EDITMSG",),
    )

    assert completed.returncode == 0, completed.stderr
    argv = capture.read_text(encoding="utf-8").splitlines()[3:]
    expected_root = worktree.as_posix() if os.name == "nt" else str(worktree)
    assert argv[:5] == ["run", "--no-project", "--offline", "--no-config", "--python"]
    assert argv[6:] == [
        "python",
        f"{expected_root}/tools/check_commit_attribution.py",
        "--message-file",
        ".git/COMMIT_EDITMSG",
    ]


def test_hooks_path_warning_only_when_it_shadows_another_directory(
    tmp_path, monkeypatch, capsys
):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", _fake_sources(tmp_path))
    alias = tmp_path / "alias"
    try:
        alias.symlink_to(repo, target_is_directory=True)
    except OSError as exc:  # Windows without the symlink privilege
        pytest.skip(f"host cannot create directory symlinks: {exc}")

    # Same hooks directory spelled through a symlink: nothing is shadowed.
    run_guarded_test_process(
        [
            "git",
            "-C",
            str(repo),
            "config",
            "core.hooksPath",
            str(alias / ".git" / "hooks"),
        ],
        check=True,
    )
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert "WARNING" not in capsys.readouterr().out

    # A different directory really shadows the managed hooks.
    run_guarded_test_process(
        [
            "git",
            "-C",
            str(repo),
            "config",
            "core.hooksPath",
            str(tmp_path / "elsewhere"),
        ],
        check=True,
    )
    for hook in ig.HOOKS:
        (repo / ".git" / "hooks" / hook.name).unlink()
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert "WARNING: core.hooksPath=" in capsys.readouterr().out
