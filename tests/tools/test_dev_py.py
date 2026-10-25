from __future__ import annotations

import json
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import custody_layout
from molt import dx as molt_dx
from tools import dev
from tests.python_environment_test_support import materialize_development_environment


def _fake_dx_env(module):
    return {
        "PATH": "",
        "PYTHONPATH": str(module.ROOT / "src"),
        "UV_PROJECT_ENVIRONMENT": str(module.ROOT / ".venv"),
    }


@pytest.mark.parametrize("command", ["doctor", "setup", "update", "validate"])
def test_dev_py_maintenance_uses_declared_frontend(monkeypatch, command) -> None:
    module = dev
    calls: list[tuple[list[str], str | None, bool]] = []
    monkeypatch.setattr(
        module.target_python_policy,
        "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS",
        ("3.15", "3.13"),
    )

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", command, "--check", "--all"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            ["python", "-m", "molt.cli", command, "--check", "--all"],
            None,
            False,
        )
    ]


def test_dev_py_clean_artifacts_dispatches_to_cleanup_tool(monkeypatch) -> None:
    module = dev
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
    module = dev
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

    # Critical-root validation includes the complete source audit. Dispatch it
    # once instead of repeating the full inventory for the default command.
    assert len(stdlib_calls) == 1
    assert "--critical-allowlist" in stdlib_calls[0]
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
    module = dev
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
    module = dev
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
    module = dev
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
    module = dev
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
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
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
    module = dev
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

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["status"] == "error"
    assert payload["git_status"] is None
    assert [step["returncode"] for step in payload["steps"]] == [0, 17]
    assert "gate command failed" in payload["errors"][0]


def test_dev_py_command_refs_fail_loudly_on_bad_config() -> None:
    module = dev

    with pytest.raises(RuntimeError, match="Missing .*missing"):
        module._split_command_sequence("@missing", "root", commands={})

    with pytest.raises(RuntimeError, match="Cyclic"):
        module._split_command_sequence("@a", "root", commands={"a": "@a"})


def test_dev_py_canonical_env_keeps_backend_daemon_enabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = dev
    monkeypatch.delenv("MOLT_BACKEND_DAEMON", raising=False)

    env = module._canonical_env()

    assert env["MOLT_BACKEND_DAEMON"] == "1"


@pytest.mark.parametrize(
    "catalog", [("3.12", "3.13", "3.14", "3.15"), ("3.15", "3.12", "3.17")]
)
@pytest.mark.parametrize("flags", [[], ["--random-order", "--random-seed", "17"]])
def test_dev_py_test_argv_is_consumed_by_real_runner(
    monkeypatch, flags, catalog
) -> None:
    from tools import dev_test_runner

    module = dev
    monkeypatch.setattr(
        module.target_python_policy, "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS", catalog
    )
    batches = []

    def capture_batch(args, python=None, env=None, tty=False):
        batches.append((list(args), python))
        # The driver never raises the ambient capability tier (HF-44): tests
        # that need capabilities declare them, so local runs match CI.
        assert "MOLT_CAPABILITY_TIER" not in env

    monkeypatch.delenv("MOLT_CAPABILITY_TIER", raising=False)
    monkeypatch.delenv("MOLT_PYTEST_RANDOM_ORDER", raising=False)
    monkeypatch.delenv("MOLT_PYTEST_RANDOM_SEED", raising=False)
    monkeypatch.delenv("MOLT_VENV", raising=False)
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(module, "run_uv", capture_batch)
    monkeypatch.setattr(module.sys, "argv", ["tools/dev.py", "test", *flags])
    module.main()

    assert [python for _args, python in batches] == list(catalog)
    assert sum("--check-verified-subset" in args for args, _python in batches) == 1
    for index, (args, _python) in enumerate(batches):
        assert args == [
            "python",
            "tools/dev_test_runner.py",
            *(["--check-verified-subset"] if index == 0 else []),
            *flags,
        ]
    for index, (args, _python) in enumerate(batches):
        commands = []
        monkeypatch.setattr(
            dev_test_runner, "_run", lambda cmd: commands.append(list(cmd))
        )
        monkeypatch.setattr(dev_test_runner.sys, "argv", args[1:])
        dev_test_runner.main()
        assert commands[0][:2] == ["pytest", "-q"]
        expected_tail = (
            [[dev_test_runner.sys.executable, "tools/verified_subset.py", "check"]]
            if index == 0
            else []
        )
        assert commands[1:] == expected_tail


def _isolated_checkout(module, monkeypatch, tmp_path: Path) -> Path:
    """Point dev.py at a throwaway clone, so its run context never writes
    target, caches or uv environments into the real checkout."""
    clone = tmp_path / "clone"
    clone.mkdir()
    monkeypatch.setattr(module, "ROOT", clone)
    monkeypatch.setattr(module, "DX", module.DxProject(clone))
    return clone


def test_dev_py_run_uv_installs_canonical_guard_env(monkeypatch, tmp_path) -> None:
    module = dev
    clone = _isolated_checkout(module, monkeypatch, tmp_path)
    image = materialize_development_environment(
        clone, clone, python="3.12", select=True
    )
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
        ["python", "-c", "print('ok')"],
        python="3.12",
        env={
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(module.ROOT),
        },
    )

    assert len(calls) == 1
    cmd, env, limits = calls[0]
    assert cmd == [
        "uv",
        "run",
        "--active",
        "--project",
        str(module.ROOT),
        "--python",
        str(image.python_executable),
        "--no-sync",
        "--no-config",
        "--",
        str(image.python_executable),
        "-B",
        "-X",
        f"pycache_prefix={image.root / 'bytecode'}",
        "-c",
        "print('ok')",
    ]
    assert limits is fake_limits
    assert env["MOLT_EXT_ROOT"] == str(module.ROOT)
    # A generated dev session id is custody metadata, not a request for a cold
    # per-session Cargo target. Ordinary dev.py commands reuse the persistent
    # target so warm incremental state survives across commands.
    assert env["CARGO_TARGET_DIR"] == str(module.ROOT / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(module.ROOT / ".molt_cache")
    module._run_repo_cmd(["python", "-IE", "-c", "pass"], env, tty=False)
    assert calls[1][0] == [
        str(image.python_executable),
        "-B",
        "-X",
        f"pycache_prefix={image.root / 'bytecode'}",
        "-IE",
        "-c",
        "pass",
    ]
    with pytest.raises(RuntimeError, match="explicitly borrowed MOLT_VENV"):
        module._run_repo_cmd(
            ["python", "-c", "pass"],
            {**env, "MOLT_VENV": str(tmp_path / "external")},
            tty=False,
        )
    assert len(calls) == 2, "an external request must not silently use ambient PATH"
    # Scratch is <artifact root>/tmp unless that sits inside the checkout, as
    # it does here; the custody layout then places it out of tree.
    scratch = custody_layout.scratch_root(module.ROOT, module.ROOT)
    assert env["MOLT_DIFF_ROOT"] == str(scratch / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(scratch)
    assert env["UV_CACHE_DIR"] == str(module.ROOT / ".uv-cache")
    assert env["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    assert env["PIP_CACHE_DIR"] == str(module.ROOT / ".pip-cache")
    assert env["PYTHONPYCACHEPREFIX"] == str(image.root / "bytecode")
    assert env["TMPDIR"] == str(scratch)
    assert env["TMP"] == env["TMPDIR"]
    assert env["TEMP"] == env["TMPDIR"]
    assert env["MOLT_SESSION_ID"].startswith("dev-")
    assert env["MOLT_SESSION_ID_GENERATED"] == "1"


def test_dev_py_run_uv_preserves_explicit_canonical_roots(
    monkeypatch, tmp_path
) -> None:
    module = dev
    clone = _isolated_checkout(module, monkeypatch, tmp_path)
    calls: list[tuple[list[str], dict[str, str]]] = []
    explicit_root = tmp_path / "external-root"
    explicit_cache = tmp_path / "cache"
    image = materialize_development_environment(clone, explicit_root, select=True)
    # Alias preservation is part of dispatch: the synthetic image must actually
    # supply the requested entry, rather than relying on a canonical fallback.
    alias = image.python_executable.with_name(
        "python3.exe" if os.name == "nt" else "python3"
    )
    alias.write_bytes(image.python_executable.read_bytes())

    def fake_check_call_guarded(cmd, env, *, limits=None):
        del limits
        calls.append((list(cmd), dict(env)))

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
        },
    )

    assert len(calls) == 1
    command, env = calls[0]
    assert command[:2] == ["uv", "run"]
    assert "--no-sync" in command
    assert command[-6:] == [
        str(alias),
        "-B",
        "-X",
        f"pycache_prefix={image.root / 'bytecode'}",
        "-c",
        "print('ok')",
    ]
    assert env["MOLT_EXT_ROOT"] == str(explicit_root)
    assert env["MOLT_CACHE"] == str(explicit_cache)
    assert env["MOLT_SESSION_ID"] == "caller-session"
    assert "MOLT_SESSION_ID_GENERATED" not in env
    # A pinned session scopes this checkout's target under the root (HF-45).
    assert env["CARGO_TARGET_DIR"] == str(
        explicit_root
        / "target"
        / "checkouts"
        / molt_dx.checkout_component(clone)
        / "sessions"
        / "caller-session"
    )


def test_dev_py_tty_uses_guard_when_memory_guard_enabled(monkeypatch) -> None:
    module = dev
    calls: list[tuple[str, list[str]]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append(("guarded", list(cmd)))

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "1"}, tty=True
    )

    assert calls == [("guarded", ["pytest", "-q"])]


def test_dev_py_tty_uses_guard_when_legacy_disable_env_is_set(monkeypatch) -> None:
    module = dev
    calls: list[tuple[str, list[str]]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append(("guarded", list(cmd)))

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "0"}, tty=True
    )

    assert calls == [("guarded", ["pytest", "-q"])]


def test_dev_py_uv_no_sync_uses_exact_projection_without_runtime_reprobe(
    monkeypatch, tmp_path
) -> None:
    from molt.cli import source_build_environment as publisher
    from molt.checkout_provisioning import ProvisioningError

    clone = _isolated_checkout(dev, monkeypatch, tmp_path)
    image = materialize_development_environment(
        clone, clone, python="3.12", select=True
    )
    next_image = materialize_development_environment(
        clone, clone, python="3.13", select=True
    )
    calls = []
    monkeypatch.setattr(
        dev, "_check_call_guarded", lambda cmd, env, **kwargs: calls.append(cmd)
    )

    def forbid_runtime_probe(*args, **kwargs):
        raise AssertionError("a current selector must not reprobe or publish")

    monkeypatch.setattr(publisher, "development_environment", forbid_runtime_probe)
    env = {"MOLT_EXT_ROOT": str(clone), "UV_PROJECT_ENVIRONMENT": str(image.root)}
    dev.run_uv(["python", "--version"], python="3.12", env=env)
    assert len(calls) == 1 and "--no-sync" in calls[0] and "--no-config" in calls[0]
    with pytest.raises(ProvisioningError, match="different requested Python"):
        dev.run_uv(["python", "--version"], python="3.13", env=env)
    assert len(calls) == 1, "a conflicting exact request must never launch or resync"
    # Installation reaches the existing ensure owner directly. It must not
    # require another managed Python child to select the first image.
    monkeypatch.setattr(dev, "_canonical_env", lambda: env)
    monkeypatch.setattr(dev.sys, "argv", ["tools/dev.py", "install"])
    dev.main()
    assert len(calls) == 1
    # A requested matrix intent is bound before DX can rediscover the default
    # image. Both current projections stay reusable without a runtime probe.
    dev.run_uv(
        ["python", "--version"], python="3.13", env={"MOLT_EXT_ROOT": str(clone)}
    )
    assert calls[-1][calls[-1].index("--python") + 1] == str(
        next_image.python_executable
    )
