"""Tests for tools/install_git_hooks.py — the idempotent managed-hook installer.

Pins the invariants that keep the gate deployable without breaking commits:
idempotence, --check semantics, foreign-hook preservation+chaining, uninstall
restore, and that we install into .git/hooks (NOT via core.hooksPath, which would
enable the pre-existing pre-commit type-check and block every commit).
"""

from __future__ import annotations

import os
import re
from collections.abc import Sequence
from pathlib import Path

import pytest

from tools.agent_coordination import choose_bash
import tools.install_git_hooks as ig
from tests.process_guard_common import run_guarded_test_process


HOOKS_DIR = Path(__file__).resolve().parents[2] / ".githooks"
COMMIT_MSG_HOOK = HOOKS_DIR / "commit-msg"
LAUNCH = HOOKS_DIR / "molt-hook-launch.sh"
PRE_PUSH = next(hook for hook in ig.HOOKS if hook.name == "pre-push")
COMMIT_MSG = next(hook for hook in ig.HOOKS if hook.name == "commit-msg")
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
REAL_GIT="$(command -v git)" || exit 2
export REAL_GIT
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
        if hook.retired:
            continue
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
    for tool in ("check_commit_attribution.py",):
        (worktree / "tools" / tool).write_text(
            "raise AssertionError('fake uv must not execute the tool')\n",
            encoding="utf-8",
        )
    (worktree / ".githooks").mkdir()
    (worktree / ".githooks" / LAUNCH.name).write_text(
        LAUNCH.read_text(encoding="utf-8"), encoding="utf-8", newline="\n"
    )
    (main / ".git").mkdir(parents=True)
    # The worktree is a real repository: the hook's trailer step reads the
    # committer identity and writes the index tree there, never in this checkout.
    _git_init(worktree)
    for key, value in (("user.name", "Molt Dev"), ("user.email", "dev@molt.test")):
        run_guarded_test_process(
            ["git", "-C", str(worktree), "config", key, value], check=True
        )
    (worktree / ".git" / "COMMIT_EDITMSG").write_text(
        "Land the feature\n\nWhy it matters.\n", encoding="utf-8", newline="\n"
    )
    # Only the repository location is faked; every other command is real Git.
    _write_executable(
        fake_bin / "git",
        """#!/usr/bin/env bash
case "$*" in
  "rev-parse --show-toplevel") printf '%s\\n' "$FAKE_REPO_ROOT" ;;
  "rev-parse --git-common-dir") printf '%s\\n' "$FAKE_COMMON_DIR" ;;
  *) exec "$REAL_GIT" "$@" ;;
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
  printf '%s\\n' '---'
} >> "$FAKE_UV_CAPTURE"
""",
    )
    return worktree, main, fake_bin, capture


def _uv_calls(capture: Path) -> list[list[str]]:
    """Each fake uv launch: PYTHONPATH, PYTHONHOME, PYTHONNOUSERSITE, then argv."""
    calls: list[list[str]] = [[]]
    for line in capture.read_text(encoding="utf-8").splitlines():
        if line == "---":
            calls.append([])
        else:
            calls[-1].append(line)
    return [call for call in calls if call]


def _run_hook(
    *,
    worktree: Path,
    main: Path,
    fake_bin: Path,
    capture: Path,
    hook: Path = COMMIT_MSG_HOOK,
    args: Sequence[str] = (".git/COMMIT_EDITMSG",),
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
        cwd=worktree,
        check=False,
    )


def test_is_molt_hook_and_chained_wrapper():
    assert ig._is_molt_hook("# molt-commit-attribution-hook v1\n", COMMIT_MSG)
    assert not ig._is_molt_hook("#!/bin/sh\necho other\n", COMMIT_MSG)
    wrapped = ig._chained_wrapper(
        "#!/usr/bin/env bash\n# molt-commit-attribution-hook v1\nbody\n", COMMIT_MSG
    )
    # shebang stays first; the preserved foreign hook is invoked before the gate body
    assert wrapped.startswith("#!/usr/bin/env bash\n")
    assert "commit-msg.local" in wrapped
    assert wrapped.index("commit-msg.local") < wrapped.index("body")


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
        if hook.retired:
            continue
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
    foreign = hooks / "commit-msg"
    foreign.write_text("#!/bin/sh\necho FOREIGN\nexit 0\n", encoding="utf-8")

    # Installing over a foreign hook preserves it as commit-msg.local and chains it.
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    installed = (hooks / "commit-msg").read_text(encoding="utf-8")
    assert ig._is_molt_hook(installed, COMMIT_MSG)
    assert "commit-msg.local" in installed
    preserved = (hooks / "commit-msg.local").read_text(encoding="utf-8")
    assert "FOREIGN" in preserved

    # Refreshes must retain the preserved hook, not replace the chain with a
    # bare attribution hook merely because the installed wrapper has our marker.
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 0
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert (hooks / "commit-msg").read_text(encoding="utf-8") == installed
    COMMIT_MSG.source.write_text(
        COMMIT_MSG.source.read_text(encoding="utf-8") + "# revised gate\n",
        encoding="utf-8",
    )
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 1
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    refreshed = (hooks / "commit-msg").read_text(encoding="utf-8")
    assert "commit-msg.local" in refreshed and "# revised gate" in refreshed
    assert (hooks / "commit-msg.local").read_text(encoding="utf-8") == preserved

    # Uninstall restores the foreign hook.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert "FOREIGN" in (hooks / "commit-msg").read_text(encoding="utf-8")
    assert not (hooks / "commit-msg.local").exists()


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
    (lines,) = _uv_calls(capture)
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
        f"{expected_root}/tools/check_commit_attribution.py",
        "--message-file",
        ".git/COMMIT_EDITMSG",
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
    (call,) = _uv_calls(capture)
    argv = call[3:]
    expected_root = worktree.as_posix() if os.name == "nt" else str(worktree)
    assert argv[:5] == ["run", "--no-project", "--offline", "--no-config", "--python"]
    assert argv[6:] == [
        "python",
        f"{expected_root}/tools/check_commit_attribution.py",
        "--message-file",
        ".git/COMMIT_EDITMSG",
    ]


def _native_python_stub(main: Path) -> None:
    native_python = (
        main / ".venv" / "Scripts" / "python.exe"
        if os.name == "nt"
        else main / ".venv" / "bin" / "python"
    )
    _write_executable(native_python, "#!/usr/bin/env bash\nexit 99\n")


def _change_ids(message: str) -> list[str]:
    return re.findall(r"^Change-Id: (I[0-9a-f]{40})$", message, flags=re.MULTILINE)


def test_commit_msg_hook_adds_one_stable_change_id(tmp_path: Path) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    _native_python_stub(main)
    message = worktree / ".git" / "COMMIT_EDITMSG"

    first = _run_hook(worktree=worktree, main=main, fake_bin=fake_bin, capture=capture)
    assert first.returncode == 0, first.stderr
    stamped = message.read_text(encoding="utf-8")
    (change_id,) = _change_ids(stamped)
    assert stamped == (
        f"Land the feature\n\nWhy it matters.\n\nChange-Id: {change_id}\n"
    )

    # An amend reruns the hook on the stamped message: the identity holds.
    again = _run_hook(worktree=worktree, main=main, fake_bin=fake_bin, capture=capture)
    assert again.returncode == 0, again.stderr
    assert message.read_text(encoding="utf-8") == stamped


def test_commit_msg_hook_leaves_fold_commits_without_change_id(tmp_path: Path) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    _native_python_stub(main)
    message = worktree / ".git" / "COMMIT_EDITMSG"
    for subject in ("fixup! Land the feature", "squash! Land", "amend! Land"):
        message.write_text(f"{subject}\n", encoding="utf-8", newline="\n")
        completed = _run_hook(
            worktree=worktree, main=main, fake_bin=fake_bin, capture=capture
        )
        assert completed.returncode == 0, completed.stderr
        assert message.read_text(encoding="utf-8") == f"{subject}\n"


def test_commit_msg_hook_refuses_before_stamping_a_rejected_message(
    tmp_path: Path,
) -> None:
    worktree, main, fake_bin, capture = _hook_fixture(tmp_path)
    _native_python_stub(main)
    _write_executable(fake_bin / "uv", "#!/usr/bin/env bash\nexit 7\n")
    message = worktree / ".git" / "COMMIT_EDITMSG"
    before = message.read_text(encoding="utf-8")

    completed = _run_hook(
        worktree=worktree, main=main, fake_bin=fake_bin, capture=capture
    )

    assert completed.returncode == 7
    assert message.read_text(encoding="utf-8") == before


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
        if not hook.retired:
            (repo / ".git" / "hooks" / hook.name).unlink()
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert "WARNING: core.hooksPath=" in capsys.readouterr().out


@pytest.mark.parametrize("foreign", [False, True])
def test_retirement_restores_foreign_hook_and_allows_real_push(tmp_path, foreign):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    hooks = repo / ".git" / "hooks"
    target = hooks / "pre-push"
    preserved = hooks / "pre-push.local"
    # Independently authored old marker: retirement must not need deleted source.
    _write_executable(target, "#!/bin/sh\n# molt-drift-gate-hook v2\nexit 97\n")
    foreign_bytes = b"#!/bin/sh\ncat > foreign-hook-input.txt\nexit 0\n"
    if foreign:
        preserved.write_bytes(foreign_bytes)
        preserved.chmod(0o751)
        foreign_mode = preserved.stat().st_mode
    before = target.read_bytes()
    assert (
        ig.install(check=True, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 1
    )
    assert target.read_bytes() == before
    assert (
        ig.install(check=False, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 0
    )
    if foreign:
        assert target.read_bytes() == foreign_bytes
        assert target.stat().st_mode == foreign_mode
        assert not preserved.exists()
    else:
        assert not target.exists()
    assert (
        ig.install(check=False, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 0
    )
    assert (
        ig.install(check=True, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 0
    )
    assert not PRE_PUSH.source.exists()

    remote = tmp_path / "remote.git"
    run_guarded_test_process(["git", "init", "--bare", "-q", str(remote)], check=True)
    for key, value in (("user.email", "hook@test.invalid"), ("user.name", "Hook test")):
        run_guarded_test_process(
            ["git", "-C", str(repo), "config", key, value], check=True
        )
    (repo / "payload.txt").write_text(
        "independently complete commit\n", encoding="utf-8"
    )
    run_guarded_test_process(["git", "-C", str(repo), "add", "payload.txt"], check=True)
    run_guarded_test_process(
        ["git", "-C", str(repo), "commit", "-qm", "payload"], check=True
    )
    run_guarded_test_process(
        ["git", "-C", str(repo), "push", str(remote), "HEAD:refs/heads/main"],
        check=True,
    )
    head = run_guarded_test_process(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()
    landed = run_guarded_test_process(
        ["git", "--git-dir", str(remote), "rev-parse", "refs/heads/main"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()
    assert landed == head
    if foreign:
        assert f" {head} refs/heads/main " in (
            repo / "foreign-hook-input.txt"
        ).read_text(encoding="utf-8")


@pytest.mark.parametrize("active_foreign", [False, True])
def test_retirement_preserves_independent_foreign_and_orphaned_local(
    tmp_path, active_foreign
):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    hooks = repo / ".git" / "hooks"
    target = hooks / "pre-push"
    preserved = hooks / "pre-push.local"
    saved = b"#!/bin/sh\necho preserved-owner\n"
    current = b"#!/bin/sh\necho independently-installed-owner\n"
    preserved.write_bytes(saved)
    if active_foreign:
        target.write_bytes(current)
    assert ig.install(
        check=True, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)
    ) == (0 if active_foreign else 1)
    assert (
        ig.install(check=False, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 0
    )
    assert target.read_bytes() == (current if active_foreign else saved)
    if active_foreign:
        assert preserved.read_bytes() == saved
    else:
        assert not preserved.exists()


def test_retirement_from_linked_worktree_uses_common_hook_directory(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    run_guarded_test_process(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
        check=True,
    )
    linked = tmp_path / "linked"
    run_guarded_test_process(
        ["git", "-C", str(repo), "worktree", "add", "--detach", str(linked)], check=True
    )
    hook = repo / ".git" / "hooks" / "pre-push"
    _write_executable(hook, "#!/bin/sh\n# molt-drift-gate-hook v1\nexit 98\n")
    assert (
        ig.install(check=True, uninstall=False, repo_root=linked, hooks=(PRE_PUSH,))
        == 1
    )
    assert hook.exists()
    assert (
        ig.install(check=False, uninstall=False, repo_root=linked, hooks=(PRE_PUSH,))
        == 0
    )
    assert not hook.exists()
    assert (
        ig.install(check=True, uninstall=False, repo_root=repo, hooks=(PRE_PUSH,)) == 0
    )


def test_active_hook_does_not_overwrite_conflicting_foreign_backup(
    tmp_path, monkeypatch
):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", _fake_sources(tmp_path))
    hooks = repo / ".git" / "hooks"
    target = hooks / "commit-msg"
    preserved = hooks / "commit-msg.local"
    target.write_bytes(b"#!/bin/sh\necho current\n")
    preserved.write_bytes(b"#!/bin/sh\necho earlier\n")
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 2
    assert target.read_bytes() == b"#!/bin/sh\necho current\n"
    assert preserved.read_bytes() == b"#!/bin/sh\necho earlier\n"
