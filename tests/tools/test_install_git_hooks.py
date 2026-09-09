"""Tests for tools/install_git_hooks.py — the idempotent pre-push drift-gate installer.

Pins the invariants that keep the gate deployable without breaking commits:
idempotence, --check semantics, foreign-hook preservation+chaining, uninstall
restore, and that we install into .git/hooks (NOT via core.hooksPath, which would
enable the pre-existing pre-commit type-check and block every commit).
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from tools.agent_coordination import choose_bash
import tools.install_git_hooks as ig
from tests.process_guard_common import run_guarded_test_process


HOOK = Path(__file__).resolve().parents[2] / ".githooks" / "pre-push"
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
exec "$selected_bash" "$hook"
"""


def _git_init(path: Path) -> None:
    run_guarded_test_process(["git", "init", "-q", str(path)], check=True)


def _fake_source(tmp_path: Path) -> Path:
    src = tmp_path / ".githooks" / "pre-push"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_text(
        "#!/usr/bin/env bash\n# molt-drift-gate-hook v1\necho gate; exit 0\n",
        encoding="utf-8",
    )
    return src


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
    (worktree / "tools" / "drift_harvest.py").write_text(
        "raise AssertionError('fake uv must not execute the gate')\n",
        encoding="utf-8",
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
            str(HOOK),
            str(worktree),
            str(main / ".git"),
            str(capture),
            bash,
        ],
        check=False,
    )


def test_is_molt_hook_and_chained_wrapper():
    assert ig._is_molt_hook("# molt-drift-gate-hook v1\n")
    assert not ig._is_molt_hook("#!/bin/sh\necho other\n")
    wrapped = ig._chained_wrapper(
        "#!/usr/bin/env bash\n# molt-drift-gate-hook v1\nbody\n"
    )
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

    # Refreshes must retain the preserved hook, not replace the chain with a
    # bare drift gate merely because the installed wrapper has our marker.
    assert ig.install(check=True, uninstall=False, repo_root=repo) == 0
    assert ig.install(check=False, uninstall=False, repo_root=repo) == 0
    assert (hooks / "pre-push").read_text(encoding="utf-8") == installed
    ig.SOURCE.write_text(ig.SOURCE.read_text() + "# revised gate\n", encoding="utf-8")
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
    monkeypatch.setattr(ig, "SOURCE", _fake_source(tmp_path))
    # Nothing installed -> uninstall is a clean no-op.
    assert ig.install(check=False, uninstall=True, repo_root=repo) == 0
    assert not (repo / ".git" / "hooks" / "pre-push").exists()


def test_hook_binds_worktree_startup_before_uv_without_project_sync() -> None:
    source = HOOK.read_text()
    assert source.index("export PYTHONPATH=") < source.index("run_gate()")
    assert 'PYTHONPATH="$python_root/src;$python_root"' in source
    assert 'PYTHONPATH="$repo_root/src:$repo_root"' in source
    assert 'cygpath -m "$repo_root"' in source
    assert "unset PYTHONHOME" in source
    uv_calls = [
        line.strip()
        for line in source.splitlines()
        if line.strip().startswith("uv run ")
    ]
    assert len(uv_calls) == 1
    assert all(
        "--no-project --offline --no-config --python " in line for line in uv_calls
    )
    assert all('python "$gate" --gate --no-fetch' in line for line in uv_calls)
    assert not any(
        line.strip().startswith('python "$gate"') for line in source.splitlines()
    )
    assert 'vpy="$main_root/.venv/Scripts/python.exe"' in source
    assert 'vpy="$main_root/.venv/bin/python"' in source
    assert '"$repo_root/.venv/' not in source


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
