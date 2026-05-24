from __future__ import annotations

from pathlib import Path

from molt.dx import CANONICAL_RUN_ENV_KEYS, RunContext
from tools import run_context_env


def test_run_context_installs_repo_local_defaults(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {"PATH": "/usr/bin"},
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(tmp_path.resolve() / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path.resolve() / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path.resolve() / ".uv-cache")
    assert env["TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["MOLT_SESSION_ID"].startswith("test-")


def test_run_context_preserves_explicit_root_and_session(tmp_path: Path) -> None:
    explicit_root = tmp_path / "external"
    explicit_target = tmp_path / "target-custom"
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {
            "MOLT_EXT_ROOT": str(explicit_root),
            "CARGO_TARGET_DIR": str(explicit_target),
            "MOLT_SESSION_ID": "caller-session",
        },
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(explicit_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["MOLT_SESSION_ID"] == "caller-session"


def test_run_context_can_force_repo_defaults_except_explicit_keys(
    tmp_path: Path,
) -> None:
    ambient_root = tmp_path / "ambient"
    explicit_cache = tmp_path / "cache"
    forced_keys = tuple(key for key in CANONICAL_RUN_ENV_KEYS if key != "MOLT_CACHE")
    env = RunContext(tmp_path, session_prefix="forced").canonical_env(
        {
            "MOLT_EXT_ROOT": str(ambient_root),
            "MOLT_CACHE": str(explicit_cache),
            "MOLT_SESSION_ID": "ambient-session",
        },
        create_dirs=False,
        force_default_keys=forced_keys,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_CACHE"] == str(explicit_cache)
    assert env["MOLT_SESSION_ID"].startswith("forced-")


def test_run_context_shell_exports_are_eval_safe(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").canonical_env(
        {
            "MOLT_SESSION_ID": 'session-"$`\\',
        },
        create_dirs=False,
    )

    shell = run_context_env.emit_shell_exports(env, ("MOLT_SESSION_ID",))

    assert shell == 'export MOLT_SESSION_ID="session-\\"\\$\\`\\\\"'
