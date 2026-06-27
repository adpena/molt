from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
DEV_PY = REPO_ROOT / "tools" / "dev.py"


def _load_dev_py():
    spec = importlib.util.spec_from_file_location("molt_tools_dev_py", DEV_PY)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _fake_dx_env(module):
    return {
        "PATH": "",
        "PYTHONPATH": str(module.ROOT / "src"),
        "UV_PROJECT_ENVIRONMENT": str(module.ROOT / ".venv"),
    }


def test_dev_py_update_dispatches_to_cli(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "update", "--check", "--all"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            ["python", "-m", "molt.cli", "update", "--check", "--all"],
            module.TEST_PYTHONS[0],
            False,
        )
    ]


def test_dev_py_clean_artifacts_dispatches_to_cleanup_tool(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []
    create_dirs_values: list[bool] = []
    fake_env = _fake_dx_env(module)

    def fake_canonical_env(*, create_dirs=True):
        create_dirs_values.append(create_dirs)
        return dict(fake_env)

    monkeypatch.setattr(
        module,
        "_canonical_env",
        fake_canonical_env,
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "clean-artifacts", "--apply"],
        raising=True,
    )

    module.main()

    assert calls == [
        [
            str(module.DX.project_python(fake_env)),
            "tools/artifact_cleanup.py",
            "--apply",
        ]
    ]
    assert create_dirs_values == [False]


def test_dev_py_lint_uses_documented_stdlib_intrinsic_gates(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: _fake_dx_env(module),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda _env: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "lint"],
        raising=True,
    )
    module.main()

    stdlib_calls = [
        args
        for args in calls
        if len(args) > 1 and args[1] == "tools/check_stdlib_intrinsics.py"
    ]

    assert any("--fallback-intrinsic-backed-only" in args for args in stdlib_calls)
    assert any("--critical-allowlist" in args for args in stdlib_calls)
    assert any(
        len(args) > 1 and args[1] == "tools/check_subprocess_guard_coverage.py"
        for args in calls
    )
    assert any(
        len(args) > 1 and args[1] == "tools/check_memory_guard_wiring.py"
        for args in calls
    )
    assert calls[0][1:4] == ["-m", "ruff", "check"]
    assert calls[1][1:5] == ["-m", "ruff", "format", "--check"]


def test_dev_py_bench_defaults_to_guarded_smoke_command(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []
    fake_env = _fake_dx_env(module)

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: dict(fake_env),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda _env: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(module.sys, "argv", ["tools/dev.py", "bench"], raising=True)

    module.main()

    assert calls == [
        [
            str(module.DX.project_python(fake_env)),
            "-m",
            "molt.cli",
            "bench",
            "--",
            "--smoke",
            "--warmup",
            "1",
            "--json-out",
            "bench/results/dev-bench-smoke.json",
        ]
    ]


def test_dev_py_bench_forwards_explicit_molt_bench_args(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: _fake_dx_env(module),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda _env: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "bench", "--wasm", "--", "--smoke"],
        raising=True,
    )

    module.main()

    assert calls == [
        [
            str(module.ROOT / ".venv" / "bin" / "python3"),
            "-m",
            "molt.cli",
            "bench",
            "--wasm",
            "--",
            "--smoke",
        ]
    ]


def test_dev_py_clippy_expands_backend_and_tir_ratchets(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: _fake_dx_env(module),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(module.sys, "argv", ["tools/dev.py", "clippy"], raising=True)

    module.main()

    assert calls == [
        [
            "cargo",
            "clippy",
            "-p",
            "molt-backend",
            "--features",
            "native-backend",
            "--",
            "-D",
            "warnings",
        ],
        [
            "cargo",
            "clippy",
            "-p",
            "molt-tir",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    ]


def test_dev_py_gates_expand_pyproject_command_refs(monkeypatch, tmp_path) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []
    summary_path = tmp_path / "dev-gates-summary.json"

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: _fake_dx_env(module),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda _env: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )

    fake_limits = object()
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_from_env",
        lambda prefix, env: fake_limits,
        raising=True,
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_summary",
        lambda limits: {"enabled": True, "marker": limits is fake_limits},
        raising=True,
    )

    def fake_status_run(cmd, **kwargs):
        assert cmd == ["git", "status", "--short"]
        assert kwargs["prefix"] == "MOLT_TEST_SUITE"
        assert kwargs["limits"] is fake_limits
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_status_run,
        raising=True,
    )

    module._run_dx_gates(
        ["--allow-dirty", "--summary-out", str(summary_path)],
        tty=False,
    )

    assert calls[:3] == [
        [
            "cargo",
            "clippy",
            "-p",
            "molt-backend",
            "--features",
            "native-backend",
            "--",
            "-D",
            "warnings",
        ],
        [
            "cargo",
            "clippy",
            "-p",
            "molt-tir",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        ["cargo", "deny", "check"],
    ]
    assert calls[3][0:4] == ["cargo", "build", "--profile", "release-fast"]
    assert calls[4][0:6] == [
        "cargo",
        "test",
        "--profile",
        "release-fast",
        "-p",
        "molt-backend",
    ]
    assert calls[5][1:4] == ["-m", "pytest", "tests/compliance/"]
    payload = json.loads(summary_path.read_text())
    assert payload["status"] == "ok"
    assert payload["summary_path"] == str(summary_path)
    assert payload["allow_dirty"] is True
    assert payload["memory_guard"]["MOLT_TEST_SUITE"] == {
        "enabled": True,
        "marker": True,
    }
    assert [step["returncode"] for step in payload["steps"]] == [0, 0, 0, 0, 0, 0]
    assert payload["git_status"]["stdout"] == ""
    assert payload["errors"] == []


def test_dev_py_gates_writes_error_summary_on_failed_gate(
    monkeypatch, tmp_path
) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []
    summary_path = tmp_path / "failed-gates.json"

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: _fake_dx_env(module),
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda _env: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )

    def fake_run_repo_cmd(cmd, _env, *, tty):
        calls.append(list(cmd))
        if len(calls) == 2:
            raise module.subprocess.CalledProcessError(17, cmd)

    monkeypatch.setattr(module, "_run_repo_cmd", fake_run_repo_cmd, raising=True)

    fake_limits = object()
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_from_env",
        lambda prefix, env: fake_limits,
        raising=True,
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_summary",
        lambda limits: {"enabled": True},
        raising=True,
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("git status should not run after a failed gate")
        ),
        raising=True,
    )

    with pytest.raises(module.subprocess.CalledProcessError):
        module._run_dx_gates(["--summary-out", str(summary_path)], tty=False)

    payload = json.loads(summary_path.read_text())
    assert payload["status"] == "error"
    assert payload["git_status"] is None
    assert [step["returncode"] for step in payload["steps"]] == [0, 17]
    assert "gate command failed" in payload["errors"][0]


def test_dev_py_command_refs_fail_loudly_on_bad_config() -> None:
    module = _load_dev_py()

    with pytest.raises(RuntimeError, match="Missing .*missing"):
        module._split_command_sequence("@missing", "root", commands={})

    with pytest.raises(RuntimeError, match="Cyclic"):
        module._split_command_sequence("@a", "root", commands={"a": "@a"})


def test_dev_py_canonical_env_keeps_backend_daemon_enabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_dev_py()
    monkeypatch.delenv("MOLT_BACKEND_DAEMON", raising=False)

    env = module._canonical_env()

    assert env["MOLT_BACKEND_DAEMON"] == "1"


def test_dev_py_test_forwards_random_order_flags(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "test", "--random-order", "--random-seed", "17"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            [
                "python",
                "tools/dev_test_runner.py",
                "--verified-subset",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[0],
            False,
        ),
        (
            [
                "python",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[1],
            False,
        ),
        (
            [
                "python",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[2],
            False,
        ),
    ]


def test_dev_py_run_uv_installs_canonical_guard_env(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], dict[str, str], object | None]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append((list(cmd), dict(env), limits))

    fake_limits = object()
    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_from_env",
        lambda prefix, env: fake_limits,
        raising=True,
    )

    module.run_uv(
        ["python3", "-c", "print('ok')"],
        python="3.12",
        env={
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(module.ROOT),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
    )

    assert len(calls) == 1
    cmd, env, limits = calls[0]
    assert cmd == [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-c",
        "print('ok')",
    ]
    assert limits is fake_limits
    assert env["MOLT_EXT_ROOT"] == str(module.ROOT)
    assert env["CARGO_TARGET_DIR"] == str(
        module.ROOT / "target" / "sessions" / env["MOLT_SESSION_ID"]
    )
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(module.ROOT / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(module.ROOT / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(module.ROOT / "tmp")
    assert env["UV_CACHE_DIR"] == str(module.ROOT / ".uv-cache")
    assert env["UV_PROJECT_ENVIRONMENT"].startswith(
        str(module.ROOT / "tmp" / "uv-project-envs")
    )
    assert env["PIP_CACHE_DIR"] == str(module.ROOT / ".pip-cache")
    assert env["PYTHONPYCACHEPREFIX"] == str(module.ROOT / "tmp" / "pycache")
    assert env["TMPDIR"] == str(module.ROOT / "tmp")
    assert env["TMP"] == env["TMPDIR"]
    assert env["TEMP"] == env["TMPDIR"]
    assert env["MOLT_SESSION_ID"].startswith("dev-")


def test_dev_py_run_uv_preserves_explicit_canonical_roots(
    monkeypatch, tmp_path
) -> None:
    module = _load_dev_py()
    calls: list[dict[str, str]] = []
    explicit_root = tmp_path / "external-root"
    explicit_cache = tmp_path / "cache"

    def fake_check_call_guarded(_cmd, env, *, limits=None):
        del limits
        calls.append(dict(env))

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_from_env",
        lambda prefix, env: object(),
        raising=True,
    )

    module.run_uv(
        ["python3", "-c", "print('ok')"],
        env={
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(explicit_root),
            "MOLT_CACHE": str(explicit_cache),
            "MOLT_SESSION_ID": "caller-session",
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
    )

    assert len(calls) == 1
    env = calls[0]
    assert env["MOLT_EXT_ROOT"] == str(explicit_root)
    assert env["MOLT_CACHE"] == str(explicit_cache)
    assert env["MOLT_SESSION_ID"] == "caller-session"
    assert env["CARGO_TARGET_DIR"] == str(
        explicit_root / "target" / "sessions" / "caller-session"
    )


def test_dev_py_tty_uses_guard_when_memory_guard_enabled(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[str, list[str]]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append(("guarded", list(cmd)))

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "1"}, tty=True
    )

    assert calls == [("guarded", ["pytest", "-q"])]


def test_dev_py_tty_uses_guard_when_legacy_disable_env_is_set(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[str, list[str]]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append(("guarded", list(cmd)))

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "0"}, tty=True
    )

    assert calls == [("guarded", ["pytest", "-q"])]


def test_dev_py_uv_no_sync_version_probe_uses_memory_guard(monkeypatch) -> None:
    module = _load_dev_py()
    fake_python = module.ROOT / "pyproject.toml"
    fake_limits = object()
    calls: list[tuple[list[str], dict[str, object]]] = []

    monkeypatch.setattr(
        module,
        "_uv_project_python",
        lambda _env=None: fake_python,
        raising=True,
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "limits_from_env",
        lambda prefix, env: fake_limits,
        raising=True,
    )

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append((list(cmd), dict(kwargs)))
        return SimpleNamespace(returncode=0, stdout="3.12\n", stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
        raising=True,
    )

    assert module._uv_project_env_matches_python(
        "3.12",
        {
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(module.ROOT),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
    )

    assert len(calls) == 1
    cmd, kwargs = calls[0]
    assert cmd == [
        str(fake_python),
        "-c",
        "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
    ]
    assert kwargs["prefix"] == "MOLT_TEST_SUITE"
    assert kwargs["cwd"] == module.ROOT
    assert kwargs["capture_output"] is True
    assert kwargs["text"] is True
    assert kwargs["limits"] is fake_limits
    assert kwargs["env"]["MOLT_EXT_ROOT"] == str(module.ROOT)


def test_dev_py_uv_no_sync_normalization_uses_guarded_probe(monkeypatch) -> None:
    module = _load_dev_py()
    probes: list[tuple[str | None, dict[str, str]]] = []

    def fake_probe(requested, env):
        probes.append((requested, dict(env)))
        return requested == "3.12"

    monkeypatch.setattr(
        module,
        "_uv_project_env_matches_python",
        fake_probe,
        raising=True,
    )

    matching = module._normalized_uv_run_env({"UV_NO_SYNC": "1"}, python="3.12")
    mismatched = module._normalized_uv_run_env({"UV_NO_SYNC": "1"}, python="3.13")

    assert matching["UV_NO_SYNC"] == "1"
    assert "UV_NO_SYNC" not in mismatched
    assert probes == [
        ("3.12", {"UV_NO_SYNC": "1"}),
        ("3.13", {"UV_NO_SYNC": "1"}),
    ]


def test_dx_normalized_uv_no_sync_requires_guarded_probe() -> None:
    module = _load_dev_py()

    with pytest.raises(RuntimeError, match="guarded project Python version probe"):
        module.DX.normalized_uv_run_env({"UV_NO_SYNC": "1"}, python="3.12")
