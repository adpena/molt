"""Teeth for the PreToolUse bash_guard (APPARATUS Wave 1, A1).

Proves the pure ``decide()`` BLOCKS every guarded class and ALLOWS the safe
forms + the cwd-differs (linked-worktree) case + the override.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks.bash_guard as bg  # noqa: E402


def _d(cmd, *, shared=False, queue=False, https=False, env=None):
    return bg.decide(
        cmd,
        in_shared_checkout=shared,
        queue_live=queue,
        origin_is_https=https,
        env=env,
    )


# --- (a) destructive git on the shared checkout ---------------------------


def test_reset_hard_blocked_on_shared_checkout():
    d = _d("git reset --hard HEAD", shared=True)
    assert d.block and d.rule == "destructive-git-shared-checkout"


def test_reset_hard_allowed_when_cwd_differs_linked_worktree():
    # cwd provably != shared root -> the same command is SAFE.
    d = _d("git reset --hard HEAD", shared=False)
    assert not d.block


def test_checkout_pathspec_and_clean_and_stash_and_restore_blocked_on_shared():
    for cmd in (
        "git checkout -- src/foo.rs",
        "git checkout -f",
        "git clean -fd",
        "git clean -fdx",
        "git stash drop",
        "git stash pop",
        "git restore src/foo.rs",
        "git branch -D somebranch",
    ):
        assert _d(cmd, shared=True).block, cmd


def test_plain_branch_switch_allowed_on_shared():
    # A pure branch switch overwrites nothing; must pass.
    assert not _d("git checkout main", shared=True).block
    assert not _d("git switch feature", shared=True).block


def test_destructive_git_in_compound_command_detected():
    d = _d("cd runtime && git reset --hard origin/main", shared=True)
    assert d.block and d.rule == "destructive-git-shared-checkout"


# --- (b) git add / commit sweeps (M20) ------------------------------------


def test_git_add_all_and_dot_blocked():
    assert _d("git add -A").rule == "git-add-sweep"
    assert _d("git add .").rule == "git-add-sweep"
    assert _d("git add --all").rule == "git-add-sweep"
    assert _d("git add -u").rule == "git-add-sweep"


def test_git_commit_all_blocked():
    assert _d('git commit -am "msg"').rule == "git-commit-all-sweep"
    assert _d('git commit -a -m "msg"').rule == "git-commit-all-sweep"


def test_add_then_unscoped_commit_blocked():
    d = _d('git add foo.rs && git commit -m "msg"')
    assert d.block and d.rule == "git-add-commit-sweep"


def test_scoped_commit_forms_allowed():
    # The M20-compliant forms must pass.
    assert not _d('git commit -m "msg" -- foo.rs').block
    assert not _d('git add -- foo.rs && git commit -m "msg" -- foo.rs').block
    assert not _d("git commit -- foo.rs").block


def test_add_specific_path_alone_allowed():
    assert not _d("git add -- foo.rs bar.rs").block


# --- (c) heavy build bypassing a live queue (M27) -------------------------


def test_cargo_build_blocked_when_queue_live():
    assert _d("cargo build --release", queue=True).rule == "build-bypasses-queue"
    assert _d("cargo test -p molt-runtime", queue=True).rule == "build-bypasses-queue"


def test_molt_build_blocked_when_queue_live():
    assert _d("molt build --release app.py", queue=True).rule == "build-bypasses-queue"


def test_cargo_build_allowed_when_queue_idle():
    assert not _d("cargo build --release", queue=False).block


def test_build_routed_through_queue_allowed():
    # A build invoked THROUGH the governed path is fine even with a live queue.
    assert not _d(
        "python tools/proof_queue.py cargo -- build --release", queue=True
    ).block


# --- (d) https push to origin (M19) ---------------------------------------


def test_https_push_blocked_by_remote_url():
    assert _d("git push https://github.com/adpena/molt.git main").rule == "https-push"


def test_push_to_origin_blocked_when_origin_is_https():
    assert _d("git push origin main", https=True).rule == "https-push"


def test_ssh_push_allowed():
    assert not _d("git push origin main", https=False).block
    assert not _d("git push git@github.com:adpena/molt.git main").block


# --- override token (audited allow) ---------------------------------------


def test_override_inline_neutralizes_block():
    d = _d("MOLT_GUARD_OK=1 git reset --hard HEAD", shared=True)
    assert not d.block and d.override


def test_override_env_neutralizes_block():
    d = _d("git reset --hard HEAD", shared=True, env={"MOLT_GUARD_OK": "1"})
    assert not d.block and d.override


def test_empty_command_no_block():
    assert not _d("").block
    assert not _d("   ").block


# --- wrapper wiring: block emits exit 2; allow emits exit 0 ----------------


def _run_with_stdin(monkeypatch, payload):
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(payload)))
    return bg.run()


def test_run_allows_non_bash_tool(monkeypatch):
    code = _run_with_stdin(monkeypatch, {"tool_name": "Read", "tool_input": {}})
    assert code == 0


def test_run_fast_path_allows_plain_command(monkeypatch):
    code = _run_with_stdin(
        monkeypatch, {"tool_name": "Bash", "tool_input": {"command": "ls -la"}}
    )
    assert code == 0


def test_run_blocks_destructive_git_returns_exit_2(monkeypatch, tmp_path):
    # Force the guard to treat the cwd as the shared checkout.
    monkeypatch.setattr(bg._common, "is_linked_worktree", lambda root: False)
    monkeypatch.setattr(bg._common, "repo_root", lambda cwd=None: tmp_path)
    code = _run_with_stdin(
        monkeypatch,
        {
            "tool_name": "Bash",
            "tool_input": {"command": "git reset --hard HEAD"},
            "cwd": str(tmp_path),
        },
    )
    assert code == 2


def test_wrapper_and_queue_substrings_cannot_hide_heavy_builds():
    for command in (
        "python tools/venv_exec.py cargo build",
        "python -m tools.venv_exec -- cargo test",
        "cargo build && echo proof_queue",
        "echo venv_exec.py; cargo check",
        "python tools/proof_queue.py list; cargo build",
        "cargo build --manifest-path proof_queue/Cargo.toml",
        "python -m molt.cli build examples/hello.py",
        "uv run cargo build",
    ):
        decision = _d(command, queue=True)
        assert decision.block and decision.rule == "build-bypasses-queue", command


def test_actual_queue_payload_and_leaf_wrapper_do_not_look_like_direct_builds():
    assert not _d("python tools/proof_queue.py cargo -- build", queue=True).block
    assert not _d("python tools/venv_exec.py -- python -c 'print(1)'", queue=True).block


def test_cargo_operation_family_cannot_hide_behind_selector_or_global_options():
    prefixes = (
        "",
        "python tools/venv_exec.py -- ",
        "python -m tools.venv_exec -- ",
        "python tools/uv_project_env.py --python 3.12 --purpose hook-test -- ",
        "python -m tools.uv_project_env --python 3.14 --purpose hook-test -- ",
        "uv run ",
    )
    operations = (
        "build",
        "b",
        "test",
        "t",
        "check",
        "c",
        "run",
        "r",
        "clippy",
        "bench",
        "rustc",
        "doc",
        "rustdoc",
        "install",
    )
    global_options = (
        "+stable ",
        "--locked ",
        "--color always ",
        "--config build.jobs=1 ",
        "+nightly --offline --color=always ",
        "-Z unstable-options ",
        "-C . ",
    )
    for prefix in prefixes:
        for options in global_options:
            for operation in operations:
                command = f"{prefix}cargo {options}{operation}"
                decision = _d(command, queue=True)
                assert decision.block and decision.rule == "build-bypasses-queue", (
                    command
                )


def test_cargo_option_operands_are_not_operations_and_idle_queue_is_preserved():
    for command in (
        "cargo --config build metadata",
        "cargo +stable --color test metadata",
        "python tools/venv_exec.py -- cargo --config build metadata",
        "python tools/uv_project_env.py --python 3.12 -- cargo +stable --color test metadata",
    ):
        assert not _d(command, queue=True).block, command
    assert not _d(
        "python tools/venv_exec.py -- cargo +stable --color always build", queue=False
    ).block


def test_malformed_cargo_global_options_fail_closed_during_live_queue():
    for prefix in (
        "",
        "python tools/venv_exec.py -- ",
        "python tools/uv_project_env.py --python 3.12 -- ",
    ):
        for payload in ("cargo +", "cargo --color", "cargo --config"):
            decision = _d(prefix + payload, queue=True)
            assert decision.block and decision.rule == "build-bypasses-queue", (
                prefix + payload
            )


def test_override_text_and_unrelated_assignments_cannot_grant_execution_waiver():
    for heavy in (
        "cargo +stable build",
        "python tools/venv_exec.py -- cargo build",
        "python tools/uv_project_env.py --python 3.12 -- cargo build",
    ):
        for text in (
            "echo MOLT_GUARD_OK",
            "echo 'MOLT_GUARD_OK=1'",
            "MOLT_GUARD_OK=1 echo harmless",
            'echo "MOLT_GUARD_OK=1"',
        ):
            for command in (f"{heavy} && {text}", f"{text}; {heavy}"):
                decision = _d(command, queue=True)
                assert decision.block and not decision.override, command
    assert _d("git reset --hard && echo 'MOLT_GUARD_OK=1'", shared=True).block


def test_inline_override_is_scoped_to_actual_assignment_and_obeys_environment_precedence():
    assert not _d("MOLT_GUARD_OK=1 cargo +stable build", queue=True).block
    assert not _d('MOLT_GUARD_OK="1" cargo build', queue=True).block
    decision = _d("MOLT_GUARD_OK=0 cargo build", queue=True, env={"MOLT_GUARD_OK": "1"})
    assert decision.block and not decision.override
    assert not _d('"MOLT_GUARD_OK=1" echo harmless', queue=True).override
    assert _d("MOLT_GUARD_OK=1 echo harmless; cargo build", queue=True).block
