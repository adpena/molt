from __future__ import annotations

from molt.compilation_target import TargetKind, admit_target

from molt.cli import wasm_link_inputs
from molt.tool_releases import run_command
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract

import importlib
import json
import os
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
import platform

import pytest

from molt.cli import process_execution, quality_commands, script_commands
from molt.node_runtime import NodeRuntime
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from molt.llvm_toolchain import LlvmBackendPin
from tests.cli.process_guard import run_cli_test_process
from tests.process_guard_common import install_module_view
from molt.cli import deps as cli_deps
from molt.cli import debug_helpers as cli_debug_helpers
from molt.cli import backend_cache as cli_backend_cache
from molt.cli import wrapper_build as cli_wrapper_build
from molt import process_environment, toolchain_identity
from molt.cli.toolchain_validation import _MaintenanceStep, _ValidationStep
from molt.cli.wrapper_build import _WrapperBuildContract

# These cases build synthetic projects and assert developer-host roots and
# guard limits; hosted custody has its own cases in
# tests/test_dx_run_context.py, and each limit case sets the caps it tests.
pytestmark = pytest.mark.usefixtures("developer_host_context", "no_ambient_guard_caps")


ROOT = Path(__file__).resolve().parents[2]
COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")
COMMAND_RUNTIME = importlib.import_module("molt.cli.command_runtime")
CARGO_EXECUTION = importlib.import_module("molt.cli.cargo_execution")
SETUP_READINESS = importlib.import_module("molt.cli.setup_readiness")
TOOLCHAIN_VALIDATION = importlib.import_module("molt.cli.toolchain_validation")
RUNTIME_WASM_VALIDATION = importlib.import_module("molt.cli.runtime_wasm_validation")
WASM_TOOLCHAIN = importlib.import_module("molt.cli.wasm_toolchain")


@pytest.fixture(autouse=True)
def _isolated_sccache(monkeypatch, tmp_path, developer_host_context):
    from tests.sccache_test_support import isolate_sccache_environment

    isolate_sccache_environment(monkeypatch, tmp_path)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "command-artifacts"))


def _base_env(run_context: dict[str, str]) -> dict[str, str]:
    # The child runs on the real checkout, so it gets the session's run
    # context back (`checkout_run_context`) on top of the test's settings.
    env = {**os.environ, **run_context}
    env["PYTHONPATH"] = str(ROOT / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _patch_memory_guard_loader(
    monkeypatch: pytest.MonkeyPatch,
    loader: object,
) -> None:
    monkeypatch.setattr(
        COMMAND_RUNTIME,
        "_load_cli_harness_memory_guard",
        loader,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_load_cli_harness_memory_guard",
        loader,
        raising=True,
    )


def _run_cli(
    args: list[str], run_context: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [sys.executable, "-m", "molt.cli", *args],
        cwd=ROOT,
        env=_base_env(run_context),
        capture_output=True,
        text=True,
        check=False,
    )


def _run_dev(
    args: list[str], run_context: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [sys.executable, "tools/dev.py", *args],
        cwd=ROOT,
        env=_base_env(run_context),
        capture_output=True,
        text=True,
        check=False,
    )


def _fake_cli_harness(
    calls: list[dict[str, object]],
    *,
    result_factory=None,
):
    class FakeContext:
        def __init__(
            self,
            prefix: str,
            env: ProcessEnvironment | None,
            repo_root: Path,
        ) -> None:
            self.prefix = prefix
            assert env is None or isinstance(env, ProcessEnvironment)
            self.env = env
            self.repo_root = repo_root

        @classmethod
        def from_env(
            cls,
            prefix: str,
            env: ProcessEnvironment | None,
            *,
            repo_root: Path,
        ):
            calls.append(
                {
                    "method": "context",
                    "prefix": prefix,
                    "env": None if env is None else env.child_block(),
                    "repo_root": repo_root,
                }
            )
            return cls(prefix, env, repo_root)

        def run(self, cmd: list[str], **kwargs: object):
            calls.append(
                {
                    "method": "run",
                    "cmd": cmd,
                    "prefix": self.prefix,
                    "env": None if self.env is None else self.env.child_block(),
                    "repo_root": self.repo_root,
                    **kwargs,
                }
            )
            if result_factory is not None:
                return result_factory(cmd)
            return subprocess.CompletedProcess(cmd, 0, "stdout\n", "stderr\n")

    class FakeMemoryGuard:
        NameRule = process_environment.NameRule
        ProcessEnvironment = process_environment.ProcessEnvironment
        HarnessExecutionContext = FakeContext

        @staticmethod
        def limits_from_env(prefix: str, env: dict[str, str] | None = None):
            return {
                "prefix": prefix,
                "env_has_session": bool(env and env.get("MOLT_SESSION_ID")),
            }

        @staticmethod
        def limits_summary(limits):
            return {
                "enabled": True,
                "max_process_rss_gb": 1.0,
                "max_total_rss_gb": 2.0,
                "max_global_rss_gb": 3.0,
                "child_rlimit_gb": 4.0,
                "prefix": limits["prefix"],
                "env_has_session": limits["env_has_session"],
            }

    return FakeMemoryGuard


def test_cli_setup_json_reports_actions_and_environment(
    checkout_run_context: dict[str, str],
) -> None:
    res = _run_cli(["setup", "--json"], checkout_run_context)
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "setup"
    assert payload["status"] in {"ok", "error"}
    data = payload["data"]
    assert isinstance(data.get("checks"), list)
    assert isinstance(data.get("environment"), dict)
    assert isinstance(data.get("actions"), list)
    assert "CARGO_TARGET_DIR" in data["environment"]
    assert "MOLT_CACHE" in data["environment"]


def test_cli_validate_check_json_reports_canonical_matrix(
    checkout_run_context: dict[str, str],
) -> None:
    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "smoke"], checkout_run_context
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["check_only"] is True
    assert data["memory_guard"]["MOLT_BENCH"]["enabled"] is True
    assert data["memory_guard"]["MOLT_CONFORMANCE"]["enabled"] is True
    assert data["memory_guard"]["MOLT_TEST_SUITE"]["enabled"] is True
    steps = data["steps"]
    assert isinstance(steps, list)
    names = {entry["name"] for entry in steps}
    assert "cli-run-json" in names
    assert "cli-command-json" in names
    assert "subprocess-guard-audit" in names
    assert "memory-guard-wiring-audit" in names
    assert "custody-proof" in names
    assert "native-parity" in names
    assert "wasm-parity" in names
    assert "luau-support-matrix" in names
    assert "luau-compile-smoke" in names
    assert "luau-runner-available" in names
    assert "luau-ord-at-parity" in names
    assert "luau-rust-regressions" in names
    assert "conformance-smoke" in names
    assert "bench-smoke" in names
    cli_command_step = next(
        entry for entry in steps if entry["name"] == "cli-command-json"
    )
    cli_command_expr = cli_command_step["cmd"][cli_command_step["cmd"].index("-k") + 1]
    assert "test_cli_build_json_binary_executes_for_native_profiles" in cli_command_expr
    assert "test_cli_compare_json" in cli_command_expr
    assert "test_cli_run_exec_eval_raise_runtime_error" in cli_command_expr
    bench_step = next(entry for entry in steps if entry["name"] == "bench-smoke")
    assert bench_step["memory_guard_prefix"] == "MOLT_BENCH"
    assert "--warmup" in bench_step["cmd"]
    assert bench_step["cmd"][bench_step["cmd"].index("--warmup") + 1] == "1"
    guard_audit_step = next(
        entry for entry in steps if entry["name"] == "subprocess-guard-audit"
    )
    assert guard_audit_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert guard_audit_step["category"] == "command"
    assert "luau" in guard_audit_step["backends"]
    memory_guard_audit_step = next(
        entry for entry in steps if entry["name"] == "memory-guard-wiring-audit"
    )
    assert memory_guard_audit_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert memory_guard_audit_step["category"] == "command"
    assert "luau" in memory_guard_audit_step["backends"]
    custody_step = next(entry for entry in steps if entry["name"] == "custody-proof")
    assert custody_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert custody_step["category"] == "command"
    assert "tests/tools/test_process_sentinel.py" in custody_step["cmd"]
    assert "tests/tools/test_memory_guard_windows_sampling.py" in custody_step["cmd"]
    luau_compile_step = next(
        entry for entry in steps if entry["name"] == "luau-compile-smoke"
    )
    assert luau_compile_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "--target" in luau_compile_step["cmd"]
    assert luau_compile_step["cmd"][luau_compile_step["cmd"].index("--target") + 1] == (
        "luau"
    )
    assert "--profile" in luau_compile_step["cmd"]
    assert luau_compile_step["cmd"][
        luau_compile_step["cmd"].index("--profile") + 1
    ] == ("release")
    luau_output = Path(luau_compile_step["cmd"][-1])
    assert luau_output.parts[-3:] == ("validate", "luau-smoke", "hello.luau")
    # Validation output is scratch, never written into the checkout.
    assert not luau_output.is_relative_to(ROOT.resolve())
    luau_runner_step = next(
        entry for entry in steps if entry["name"] == "luau-runner-available"
    )
    assert luau_runner_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "shutil.which('luau')" in luau_runner_step["cmd"][-1]
    luau_rust_step = next(
        entry for entry in steps if entry["name"] == "luau-rust-regressions"
    )
    assert luau_rust_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "molt-backend-luau" in luau_rust_step["cmd"]
    assert "luau-backend" in luau_rust_step["cmd"]
    assert "--lib" in luau_rust_step["cmd"]
    assert "luau::tests::" in luau_rust_step["cmd"]


def test_cli_validate_custody_proof_suite_reports_only_custody_step(
    checkout_run_context: dict[str, str],
) -> None:
    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "custody-proof"],
        checkout_run_context,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    data = payload["data"]

    assert data["suite"] == "custody-proof"
    assert sorted(data["memory_guard"]) == ["MOLT_TEST_SUITE"]
    steps = data["steps"]
    assert [entry["name"] for entry in steps] == ["custody-proof"]
    custody_step = steps[0]
    assert custody_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert custody_step["category"] == "command"
    assert "tests/test_memory_guard_wiring.py" in custody_step["cmd"]
    assert "tests/tools/test_memory_guard_windows_sampling.py" in custody_step["cmd"]
    assert "tests/tools/test_process_sentinel.py" in custody_step["cmd"]


def test_cli_validate_luau_backend_filter_reports_guarded_luau_steps(
    checkout_run_context: dict[str, str],
) -> None:
    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "smoke", "--backend", "luau"],
        checkout_run_context,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    data = payload["data"]
    assert data["backend"] == "luau"
    assert data["memory_guard"]["MOLT_TEST_SUITE"]["enabled"] is True
    steps = data["steps"]
    names = {entry["name"] for entry in steps}
    assert names == {
        "subprocess-guard-audit",
        "memory-guard-wiring-audit",
        "custody-proof",
        "luau-support-matrix",
        "luau-compile-smoke",
        "luau-runner-available",
        "luau-ord-at-parity",
        "luau-rust-regressions",
    }
    assert all(entry["memory_guard_prefix"] == "MOLT_TEST_SUITE" for entry in steps)
    assert all("luau" in entry["backends"] for entry in steps)


@pytest.mark.parametrize(
    "bypass", ["MOLT_SKIP_BINARY_VALIDITY_CHECK", "MOLT_SKIP_CARGO_LOCK"]
)
def test_cli_validate_rejects_proof_bypass_environment(
    monkeypatch: pytest.MonkeyPatch,
    bypass: str,
    checkout_run_context: dict[str, str],
) -> None:
    monkeypatch.setenv(bypass, "1")

    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "smoke"], checkout_run_context
    )

    assert res.returncode == 2
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["status"] == "error"
    assert f"{bypass}=1 disables" in payload["errors"][0]


def test_cli_validate_allows_no_build_policy_without_bypassing_admission(
    monkeypatch: pytest.MonkeyPatch,
    checkout_run_context: dict[str, str],
) -> None:
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "smoke"], checkout_run_context
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["data"]["steps"]


def test_cli_validate_check_json_writes_explicit_summary_out(
    tmp_path: Path, checkout_run_context: dict[str, str]
) -> None:
    summary_path = tmp_path / "validate-plan.json"

    res = _run_cli(
        [
            "validate",
            "--check",
            "--json",
            "--suite",
            "smoke",
            "--summary-out",
            str(summary_path),
        ],
        checkout_run_context,
    )

    assert res.returncode == 0, res.stderr
    stdout_payload = json.loads(res.stdout)
    file_payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert file_payload == stdout_payload
    assert stdout_payload["data"]["check_only"] is True
    assert stdout_payload["data"]["summary_path"] == str(summary_path)


def test_cli_run_command_uses_memory_guard_prefix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:

    calls: list[dict[str, object]] = []

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("guarded CLI command used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(calls),
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, COMMAND_RUNTIME, run=fail_raw_run
    )

    rc = process_execution._run_command(
        ["python3", "-c", "print('ok')"],
        cwd=ROOT,
        env=ProcessEnvironment({"PATH": "/usr/bin"}),
        memory_guard_prefix="MOLT_BENCH",
    )

    assert rc == 0
    assert calls[0] == {
        "method": "context",
        "prefix": "MOLT_BENCH",
        "env": {"PATH": "/usr/bin"},
        "repo_root": ROOT,
    }
    assert calls[1]["method"] == "run"
    assert calls[1]["prefix"] == "MOLT_BENCH"
    assert calls[1]["cwd"] == ROOT
    assert calls[1]["capture_output"] is False


@pytest.mark.parametrize("closed", [False, True])
def test_cli_timed_command_uses_memory_guard_elapsed(
    monkeypatch: pytest.MonkeyPatch,
    closed: bool,
) -> None:

    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        result = subprocess.CompletedProcess(cmd, 0, "stdout\n", "stderr\n")
        result.descendants_closed = closed
        result.elapsed_s = 0.125  # type: ignore[attr-defined]
        return result

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("guarded timed CLI command used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls, result_factory=result_factory
        ),
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, COMMAND_RUNTIME, run=fail_raw_run
    )

    result = process_execution._run_command_timed(
        ["python3", "-c", "print('ok')"],
        cwd=ROOT,
        env=ProcessEnvironment({"PATH": "/usr/bin"}),
        capture_output=True,
        memory_guard_prefix="MOLT_CLI",
    )

    assert result.descendants_closed is closed
    assert result.returncode == 0
    assert result.stdout == "stdout\n"
    assert result.stderr == "stderr\n"
    assert result.duration_s == pytest.approx(0.125)
    assert calls[0]["prefix"] == "MOLT_CLI"
    assert calls[1]["prefix"] == "MOLT_CLI"
    assert calls[1]["capture_output"] is True


def test_cli_guard_preserves_operator_limits_for_sanitized_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:

    calls: list[dict[str, object]] = []

    monkeypatch.setenv("MOLT_CLI_MAX_PROCESS_RSS_GB", "0.05")
    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    result = COMMAND_RUNTIME._run_completed_command(
        ["python3", "-c", "pass"],
        cwd=ROOT,
        env=ProcessEnvironment({"PATH": "/usr/bin"}),
        capture_output=True,
        memory_guard_prefix="MOLT_CLI",
    )

    assert result.returncode == 0
    assert calls[0]["env"] == {
        "PATH": "/usr/bin",
        "MOLT_CLI_MAX_PROCESS_RSS_GB": "0.05",
    }
    assert calls[1]["env"] == calls[0]["env"]
    assert calls[1]["repo_root"] == ROOT


def test_cli_cargo_build_helper_uses_default_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
    admitted_build_capacity: None,
    tmp_path: Path,
) -> None:

    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        run_count = sum(1 for call in calls if call["method"] == "run")
        return subprocess.CompletedProcess(
            cmd,
            1 if run_count == 1 else 0,
            "",
            "sccache: error: cache unavailable",
        )

    def fail_raw_subprocess_run(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("cargo helper used raw subprocess.run")

    monkeypatch.setenv("MOLT_BUILD_MAX_PROCESS_RSS_GB", "0.25")
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        COMMAND_RUNTIME,
        run=fail_raw_subprocess_run,
    )
    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls, result_factory=result_factory
        ),
    )

    result = CARGO_EXECUTION._run_cargo_with_sccache_retry(
        ["cargo", "build"],
        cwd=ROOT,
        env={
            "PATH": "/usr/bin",
            "RUSTC_WRAPPER": "/usr/bin/sccache",
            "CARGO_TARGET_DIR": str(tmp_path / "test-cargo-target"),
        },
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 0
    run_calls = [call for call in calls if call["method"] == "run"]
    assert [call["prefix"] for call in run_calls] == ["MOLT_BUILD", "MOLT_BUILD"]
    assert run_calls[0]["env"] == {
        "PATH": "/usr/bin",
        "RUSTC_WRAPPER": "/usr/bin/sccache",
        "CARGO_TARGET_DIR": str(tmp_path / "test-cargo-target"),
        "MOLT_BUILD_MAX_PROCESS_RSS_GB": "0.25",
    }
    assert run_calls[1]["env"] == {
        "PATH": "/usr/bin",
        "CARGO_TARGET_DIR": str(tmp_path / "test-cargo-target"),
        "MOLT_BUILD_MAX_PROCESS_RSS_GB": "0.25",
    }


def test_maybe_enable_sccache_installs_shared_dx_cache_defaults(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    short_cargo_cache: Path,
) -> None:
    artifact_root = short_cargo_cache.parent / "artifacts"
    env = ProcessEnvironmentBuilder(
        {
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(artifact_root),
            "MOLT_USE_SCCACHE": "1",
        }
    )
    monkeypatch.setattr(
        CARGO_EXECUTION, "pinned_sccache", lambda _env: "/usr/bin/sccache"
    )

    CARGO_EXECUTION._maybe_enable_sccache(env)

    assert env["RUSTC_WRAPPER"] == "/usr/bin/sccache"
    assert env["SCCACHE_DIR"] == str((artifact_root / ".sccache").resolve())
    assert env["SCCACHE_CACHE_SIZE"] == "10G"


def test_cli_wrapper_build_uses_default_memory_guard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:

    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    output = tmp_path / "main_molt"
    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        payload = {
            "status": "ok",
            "data": {
                "output": str(output),
                "consumer_output": str(output),
                "artifacts": {},
            },
        }
        result = subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        result.elapsed_s = 0.25  # type: ignore[attr-defined]
        return result

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("wrapper build used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls, result_factory=result_factory
        ),
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, COMMAND_RUNTIME, run=fail_raw_run
    )

    contract, duration, error = cli_wrapper_build._run_wrapper_build(
        file_path=str(entry),
        module=None,
        build_args=[],
        env=ProcessEnvironment({"PATH": "/usr/bin"}),
        project_root=tmp_path,
        json_output=True,
        command="run",
        verbose=False,
    )

    assert error is None
    assert contract is not None
    assert contract.consumer_output == output
    assert duration == pytest.approx(0.25)
    run_calls = [call for call in calls if call["method"] == "run"]
    assert run_calls[0]["prefix"] == "MOLT_CLI"
    assert run_calls[0]["capture_output"] is True
    assert run_calls[0]["cmd"][:4] == [sys.executable, "-m", "molt.cli", "build"]
    assert "--json" in run_calls[0]["cmd"]


def test_cli_build_toolchain_probes_use_memory_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:

    calls: list[dict[str, object]] = []
    rustc = tmp_path / "rustc"
    rustc.write_bytes(b"fixture selected compiler; never executed")
    target_libdir = tmp_path / "rust-target" / "lib"
    target_libdir.mkdir(parents=True)

    def fake_run_completed_command(cmd: list[str], **kwargs: object):
        calls.append({"cmd": cmd, **kwargs})
        executable = Path(cmd[0]).name
        if executable == "git":
            stdout = "abc123\n"
        elif executable == "rustc":
            stdout = f"{target_libdir}\n" if "--print" in cmd else "rustc 1.91.0\n"
        else:
            stdout = ""
        return subprocess.CompletedProcess(cmd, 0, stdout, "")

    monkeypatch.setattr(
        wasm_link_inputs,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "find_executable",
        lambda name, *, cwd, environment: rustc if name == "rustc" else None,
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_rustup_proxy",
        lambda selected, *, role, root, env: selected,
    )
    monkeypatch.setattr(
        COMPILER_METADATA,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        COMPILER_METADATA,
        "_rustc_version_cache_path",
        lambda identity_digest: tmp_path / f"rustc-{identity_digest}.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_VALIDATION,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    # Of the modules this test drives, only compiler_metadata looks tools up,
    # through the executable search authority.
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **_kwargs: (
            Path(f"/usr/bin/{name}")
            if name in {"rustc", "wasm-tools", "nm", "llvm-ar"}
            else None
        ),
    )
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)

    wasm_path = tmp_path / "module.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")
    obj_path = tmp_path / "module.o"
    obj_path.write_bytes(b"")

    COMPILER_METADATA._rustc_version.cache_clear()
    wasm_link_inputs.clear_rust_target_libdir_cache()

    assert COMPILER_METADATA._git_rev(ROOT) == "abc123"
    assert COMPILER_METADATA._rustc_version() == "rustc 1.91.0"
    monkeypatch.setattr(
        RUNTIME_WASM_VALIDATION,
        "run_pinned_tool",
        lambda _name, args, *, run, **kwargs: run(["wasm-tools", *args], **kwargs),
    )
    assert RUNTIME_WASM_VALIDATION._validate_wasm_structural(wasm_path) is None
    assert (
        wasm_link_inputs.rust_target_libdir("wasm32-wasip1") == target_libdir.resolve()
    )
    assert (
        cli_backend_cache._is_valid_cached_backend_artifact(
            obj_path,
            artifact_contract=resolve_backend_artifact_contract(
                target=admit_target("native"), emit_mode="obj"
            ),
        )
        is False
    )

    prefixes = [call["memory_guard_prefix"] for call in calls]
    assert prefixes[0] == "MOLT_CLI"
    assert set(prefixes[1:]) == {"MOLT_BUILD"}


def test_runtime_wasm_structural_validation_fails_loud_without_wasm_tools(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    wasm_path = tmp_path / "runtime.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")
    from molt.tool_releases import ToolReleaseError

    def unavailable(*_args, **_kwargs):
        raise ToolReleaseError("wasm-tools is required; reuse is disabled")

    monkeypatch.setattr(RUNTIME_WASM_VALIDATION, "run_pinned_tool", unavailable)

    error = RUNTIME_WASM_VALIDATION._validate_wasm_structural(wasm_path)

    assert error is not None
    assert "wasm-tools is required" in error
    assert "reuse is disabled" in error
    assert (
        "wasm-tools is required"
        in RUNTIME_WASM_VALIDATION._reusable_wasm_artifact_validation_error(wasm_path)
    )


@pytest.mark.parametrize("available", [False, True])
@pytest.mark.parametrize("installed", [False, True], ids=["source", "installed"])
def test_raw_wasm_readiness_requires_attested_validator(
    monkeypatch, available, installed
):
    from types import SimpleNamespace
    from molt.release_lanes import ReleaseLane
    from molt.tool_releases import ToolReleaseError

    distribution = (
        SimpleNamespace(
            verify_runtime=lambda: None,
            runtime={"cells": []},
            runtime_root=ROOT,
        )
        if installed
        else None
    )
    monkeypatch.setattr(
        SETUP_READINESS, "installed_compiler", lambda _root: distribution
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "installed_runtime_profile_readiness",
        lambda _installed: {
            ReleaseLane("wasm", "wasm", "dev", "dev-fast", "dev-fast"): True
        },
    )
    install_module_view(
        monkeypatch, "shutil", shutil, SETUP_READINESS, which=lambda name: name
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_run_completed_command",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(
            cmd, 0, "rustc 1.99.1\n", ""
        ),
    )
    monkeypatch.setattr(
        SETUP_READINESS, "_detect_llvm_backend_toolchain", lambda _root: (None, None)
    )
    monkeypatch.setattr(
        SETUP_READINESS.wasm_toolchain,
        "rust_target_readiness_error",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        SETUP_READINESS.wasm_toolchain,
        "resolve_wasm_linker",
        lambda: SimpleNamespace(diagnostic="admitted linker"),
    )

    def require(name):
        assert name == "wasm-tools"
        if not available:
            raise ToolReleaseError("missing attested validator; provision wasm-tools")
        return SimpleNamespace(
            executable=Path("validator"),
            release=SimpleNamespace(version="test-release"),
            executable_sha256="a" * 64,
        )

    monkeypatch.setattr(SETUP_READINESS, "require_pinned_tool", require)
    report = SETUP_READINESS._build_toolchain_report(ROOT)
    check = next(check for check in report.checks if check["name"] == "wasm-tools")
    assert check["ok"] is available
    assert report.backends["wasm"] is available
    assert report.backends["linked-wasm"] is available
    if not available:
        assert "missing attested validator" in check["detail"]
        assert check["advice"]


def test_cli_diff_command_uses_diff_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(
        script_commands, "compiler_source_root", lambda: ROOT, raising=True
    )
    monkeypatch.setattr(script_commands, "_run_command", fake_run_command, raising=True)

    assert script_commands.diff(None, None) == 0

    assert calls
    assert calls[0]["memory_guard_prefix"] == "MOLT_DIFF"
    assert calls[0]["cmd"][:2] == [sys.executable, "tests/molt_diff.py"]


def test_cli_compare_uses_diff_memory_guard_for_children(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    built_binary = project / "build" / "main_molt"
    built_binary.parent.mkdir(parents=True, exist_ok=True)
    built_binary.write_text("", encoding="utf-8")

    prefixes: list[object] = []
    commands: list[list[str]] = []

    def fake_run_command_timed(
        cmd: list[str], **kwargs: object
    ) -> process_execution._TimedResult:
        prefixes.append(kwargs.get("memory_guard_prefix"))
        commands.append(list(cmd))
        if len(prefixes) == 1:
            return process_execution._TimedResult(
                0, "ok\n", "", 0.01, descendants_closed=True
            )
        if len(prefixes) == 2:
            return process_execution._TimedResult(
                0,
                json.dumps({"data": {"output": str(built_binary)}}),
                "",
                0.02,
                descendants_closed=True,
            )
        return process_execution._TimedResult(
            0, "ok\n", "", 0.01, descendants_closed=True
        )

    monkeypatch.setattr(
        script_commands, "_find_project_root", lambda start: project, raising=True
    )
    monkeypatch.setattr(
        script_commands,
        "compiler_source_root",
        lambda: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        script_commands,
        "resolve_python_selector",
        lambda exe, **_kwargs: ("py", "-3.14"),
        raising=True,
    )
    from molt.python_interpreter import PythonInterpreter
    from molt.python_environment_location import _active_environment_prefix

    monkeypatch.setattr(
        script_commands,
        "probe_python_command",
        lambda command, **kwargs: PythonInterpreter(
            tuple(command), "oracle-python", "3.14.9", "CPython"
        ),
    )
    monkeypatch.setattr(
        script_commands,
        "_resolve_binary_output",
        lambda output: built_binary,
        raising=True,
    )
    monkeypatch.setattr(
        script_commands,
        "_run_command_timed",
        fake_run_command_timed,
        raising=True,
    )
    from molt import process_guard

    monkeypatch.setattr(process_guard, "run_completed_command", fake_run_command_timed)

    rc = script_commands.compare(str(entry), None, "3.14", [])

    assert rc == 0
    assert prefixes == ["MOLT_DIFF", "MOLT_DIFF", "MOLT_DIFF"]
    assert commands[0][:4] == ["py", "-3.14", "-B", "-X"]
    assert commands[0][4].startswith("pycache_prefix=")
    assert not Path(commands[0][4].split("=", 1)[1]).parent.exists()
    assert commands[0][5:] == [str(entry)]
    assert commands[1][:7] == [
        sys.executable,
        "-B",
        "-X",
        f"pycache_prefix={_active_environment_prefix() / 'bytecode'}",
        "-m",
        "molt.cli",
        "build",
    ]


@pytest.mark.parametrize(
    "target", [TargetKind.WASM, TargetKind.WASI, TargetKind.LUAU, TargetKind.MLIR]
)
@pytest.mark.parametrize(
    "json_output,timing,returncode",
    [
        (False, False, 0),
        (False, True, 7),
        (True, False, 0),
        (True, True, 0),
        (True, True, 7),
    ],
)
def test_cli_cross_run_uses_cross_memory_guard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    target: TargetKind,
    json_output: bool,
    timing: bool,
    returncode: int,
) -> None:
    from molt.cli import build_inputs as cli_build_inputs

    project = tmp_path / "project"
    project.mkdir()
    entry = project / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    artifact = project / (
        "out.mlir"
        if target is TargetKind.MLIR
        else "out.luau"
        if target is TargetKind.LUAU
        else "out.wasm"
    )
    artifact.write_text("", encoding="utf-8")
    manifest = project / "manifest.json"
    manifest.write_text("{}", encoding="utf-8")

    class BuildEntry:
        source_path = entry

    build_calls: list[dict[str, object]] = []
    run_calls: list[dict[str, object]] = []

    def fake_run_wrapper_build(**kwargs: object):
        build_calls.append(kwargs)
        return (
            _WrapperBuildContract(
                output=artifact,
                consumer_output=artifact,
                bundle_root=None,
                artifacts={"linked_wasm": artifact, "manifest": manifest},
            ),
            0.01,
            None,
        )

    def fake_run_completed_command(cmd: list[str], **kwargs: object):
        run_calls.append({"cmd": cmd, **kwargs})
        host_stderr = "guest stderr\n"
        if "--guest-stderr" in cmd:
            capture = Path(cmd[cmd.index("--guest-stderr") + 1])
            assert capture.read_bytes() == b""
            capture.write_bytes(b"guest stderr\n")
            host_stderr = "ExperimentalWarning: host\n"
        result = subprocess.CompletedProcess(
            cmd,
            returncode,
            "guest output ☃\n" if kwargs["capture_output"] else None,
            host_stderr if kwargs["capture_output"] else None,
        )
        result.elapsed_s = 0.02  # type: ignore[attr-defined]
        result.descendants_closed = True
        return result

    monkeypatch.setattr(
        script_commands, "_find_project_root", lambda start: project, raising=True
    )
    monkeypatch.setattr(
        script_commands,
        "compiler_source_root",
        lambda: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        cli_build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kwargs: (BuildEntry(), None),
        raising=True,
    )
    monkeypatch.setattr(
        script_commands, "_run_wrapper_build", fake_run_wrapper_build, raising=True
    )
    monkeypatch.setattr(
        process_execution,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        script_commands,
        "resolve_node_runtime",
        lambda **kwargs: NodeRuntime(Path("/usr/bin/node"), "24.0.0", 24),
        raising=True,
    )

    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        script_commands,
        which=lambda name: "/usr/bin/lune" if name == "lune" else None,
    )
    rc = script_commands._run_script_cross(
        target,
        str(entry),
        None,
        [],
        build_args=["--target", target.value],
        json_output=json_output,
        timing=timing,
        verbose=True,
    )

    assert rc == (0 if target is TargetKind.MLIR else returncode)
    expected_build_args = ["--target", target.value]
    if target in {TargetKind.WASM, TargetKind.WASI}:
        expected_build_args += ["--linked", "--require-linked"]
    assert build_calls[0]["build_args"] == expected_build_args
    assert build_calls[0]["memory_guard_prefix"] == "MOLT_CROSS"
    captured = capsys.readouterr()
    if target is TargetKind.MLIR:
        assert run_calls == []
    else:
        assert len(run_calls) == 1
        assert run_calls[0]["memory_guard_prefix"] == "MOLT_CROSS"
        assert run_calls[0]["capture_output"] is json_output
    if json_output:
        assert captured.err == ""
        payload = json.loads(captured.out)
        assert payload["command"] == "run"
        assert payload["status"] == ("ok" if rc == 0 else "error")
        if target is TargetKind.MLIR:
            expected: dict[str, object] = {"output": str(artifact), "executed": False}
            if timing:
                expected["timing"] = {"build_s": 0.01, "total_s": 0.01}
        else:
            expected = {
                "returncode": returncode,
                "stdout": "guest output ☃\n",
                "stderr": "guest stderr\n",
            }
            if target in {TargetKind.WASM, TargetKind.WASI}:
                expected["host_stderr"] = "ExperimentalWarning: host\n"
                command = run_calls[0]["cmd"]
                assert not Path(
                    command[command.index("--guest-stderr") + 1]
                ).parent.exists()
            if timing:
                expected["timing"] = {"build_s": 0.01, "run_s": 0.02, "total_s": 0.03}
        assert payload["data"] == expected
    else:
        assert captured.out == ""
        if timing:
            assert (
                "timing: build" if target is TargetKind.MLIR else "Timing (compiled):"
            ) in captured.err


def test_cli_bench_outer_process_uses_bench_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(
        quality_commands, "_run_command", fake_run_command, raising=True
    )

    assert quality_commands.bench(wasm=False, bench_args=["--smoke"]) == 0

    assert calls
    assert calls[0]["memory_guard_prefix"] == "MOLT_BENCH"
    assert "tools/bench.py" in calls[0]["cmd"]


def test_cli_pytest_uses_shared_uv_project_python_authority(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt.dx import DxProject
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    image = materialize_development_environment(
        ROOT,
        tmp_path / "artifacts",
        groups=DxProject(ROOT).dependency_groups(),
        runnable=True,
    )
    monkeypatch.delenv("MOLT_VENV", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(image.root))
    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(quality_commands, "compiler_source_root", lambda: ROOT)
    monkeypatch.setattr(quality_commands, "_run_command", fake_run_command)
    assert quality_commands.test("pytest", None, None, []) == 0
    assert calls[0]["cmd"] == [
        str(image.python_executable),
        "-B",
        "-X",
        f"pycache_prefix={image.root / 'bytecode'}",
        "-m",
        "pytest",
        "-q",
    ]
    assert calls[0]["env"]["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    assert calls[0]["env"]["PYTHONPYCACHEPREFIX"] == str(image.root / "bytecode")
    assert calls[0]["memory_guard_prefix"] == "MOLT_TEST_SUITE"


def test_cli_validate_uses_family_memory_guard_prefixes(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:

    calls: list[dict[str, object]] = []
    steps = [
        _ValidationStep(
            "conformance-step",
            ["python3", "-c", "pass"],
            ROOT,
            "conformance",
            ("native",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "bench-step",
            ["python3", "-c", "pass"],
            ROOT,
            "benchmark",
            ("native",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "correctness-step",
            ["python3", "-c", "pass"],
            ROOT,
            "correctness",
            ("native",),
            ("dev",),
            "smoke",
        ),
    ]

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "compiler_source_root",
        lambda: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_validate_steps",
        lambda root, suite, backend, profile: steps,
        raising=True,
    )
    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    summary_path = tmp_path / "validate-summary.json"

    assert (
        TOOLCHAIN_VALIDATION.validate(
            suite="smoke",
            json_output=True,
            summary_out=str(summary_path),
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert json.loads(summary_path.read_text(encoding="utf-8")) == payload
    assert payload["data"]["summary_path"] == str(summary_path)
    assert payload["data"]["check_only"] is False
    assert isinstance(payload["data"]["elapsed_s"], float)
    assert payload["data"]["memory_guard"] == {
        "MOLT_BENCH": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_BENCH",
            "env_has_session": True,
        },
        "MOLT_CONFORMANCE": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_CONFORMANCE",
            "env_has_session": True,
        },
        "MOLT_TEST_SUITE": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_TEST_SUITE",
            "env_has_session": True,
        },
    }
    prefixes = [call["prefix"] for call in calls if call["method"] == "run"]
    assert prefixes == ["MOLT_CONFORMANCE", "MOLT_BENCH", "MOLT_TEST_SUITE"]


def test_cli_validate_defaults_execution_summary_to_logs(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:

    calls: list[dict[str, object]] = []
    steps = [
        _ValidationStep(
            "correctness-step",
            ["python3", "-c", "pass"],
            tmp_path,
            "correctness",
            ("native",),
            ("dev",),
            "smoke",
        )
    ]

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "compiler_source_root",
        lambda: tmp_path,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_require_molt_root",
        lambda *args: None,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_validate_steps",
        lambda root, suite, backend, profile: steps,
        raising=True,
    )
    _patch_memory_guard_loader(
        monkeypatch,
        lambda cwd, *, guard_source_root=None: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    assert (
        TOOLCHAIN_VALIDATION.validate(
            suite="smoke",
            backend="native",
            profile="dev",
            json_output=True,
        )
        == 0
    )

    summary_path = tmp_path / "logs" / "validate-smoke-native-dev.json"
    payload = json.loads(capsys.readouterr().out)
    assert json.loads(summary_path.read_text(encoding="utf-8")) == payload
    assert payload["data"]["summary_path"] == str(summary_path)
    assert payload["data"]["results"][0]["name"] == "correctness-step"


def test_tools_dev_validate_delegates_to_canonical_cli(
    checkout_run_context: dict[str, str],
) -> None:
    # Exercise the real externally provisioned bootstrap role on every host,
    # including local runs whose outer context selected a managed image.
    borrowed_context = {**checkout_run_context, "MOLT_VENV": sys.prefix}
    res = _run_dev(["validate", "--check"], borrowed_context)
    assert res.returncode == 0, res.stderr
    assert "validate" in res.stdout.lower() or "validate" in res.stderr.lower()


def test_cli_lint_uses_shared_dx_planner(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt.dx import DxProject
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    image = materialize_development_environment(
        ROOT,
        tmp_path / "artifacts",
        groups=DxProject(ROOT).dependency_groups(),
        runnable=True,
    )
    monkeypatch.delenv("MOLT_VENV", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(image.root))
    # This module clears hosted custody. Artifact placement does not select
    # durable tools, so give this real DX writer its own toolchain root too.
    toolchain_root = tmp_path / "toolchain-root"
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(toolchain_root))
    # Command data is this test's input; DX parsing, image loading and launch
    # assembly retain their real owners.
    monkeypatch.setattr(
        DxProject,
        "commands",
        lambda self: {"lint": "{project_python} -I -m ruff check ."},
    )
    calls: list[list[str]] = []

    def fake_run_completed(cmd, **kwargs):
        calls.append(list(cmd))
        assert kwargs["cwd"] == ROOT
        assert kwargs["capture_output"] is False
        assert kwargs["memory_guard_prefix"] == "MOLT_CLI"
        assert kwargs["env"]["PYTHONPYCACHEPREFIX"] == str(image.root / "bytecode")
        assert kwargs["env"]["MOLT_TARGET_ROOT"] == str(toolchain_root)
        assert toolchain_root.is_dir()
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(quality_commands, "compiler_source_root", lambda: ROOT)
    monkeypatch.setattr(quality_commands, "_run_completed_command", fake_run_completed)
    assert quality_commands.lint(json_output=False, verbose=False) == 0
    assert calls == [
        [
            str(image.python_executable),
            "-B",
            "-X",
            f"pycache_prefix={image.root / 'bytecode'}",
            "-I",
            "-m",
            "ruff",
            "check",
            ".",
        ]
    ]


@pytest.mark.parametrize("failure", [False, True])
def test_cli_update_steps_use_memory_guard(
    monkeypatch: pytest.MonkeyPatch, capsys, failure: bool
) -> None:

    calls: list[dict[str, object]] = []
    steps = [
        _MaintenanceStep("probe", ["python3", "-c", "print('ok')"], ROOT, "toolchain"),
        _MaintenanceStep(
            "npm-update-project",
            [*run_command("npm"), "update", "--package-lock-only", "--ignore-scripts"],
            ROOT / "deploy/cloudflare",
            "lock",
        ),
        _MaintenanceStep("later", ["uv", "lock", "-U"], ROOT, "lock"),
    ]

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return subprocess.CompletedProcess(
            cmd,
            9 if failure and tuple(cmd[:6]) == run_command("npm") else 0,
            "ok\n",
            "",
        )

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "compiler_source_root",
        lambda: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_update_steps",
        lambda *_args, **_kwargs: (steps, []),
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_run_completed_command",
        fake_run_completed,
        raising=True,
    )

    assert TOOLCHAIN_VALIDATION.update_repo(json_output=True) == (9 if failure else 0)
    assert calls == [
        {
            "cmd": step.cmd,
            "cwd": step.cwd,
            "capture_output": True,
            "env": None,
            "memory_guard_prefix": "MOLT_CLI",
        }
        for step in (steps[:2] if failure else steps)
    ]
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == ("error" if failure else "ok")
    if failure:
        assert payload["errors"] == ["npm-update-project failed with exit code 9"]
        assert [row["name"] for row in payload["data"]["results"]] == [
            "probe",
            "npm-update-project",
        ]


@pytest.mark.parametrize("unnamed", [False, True])
def test_cli_update_shows_the_lock_advisory_audit(
    monkeypatch: pytest.MonkeyPatch, capsys, unnamed: bool
) -> None:
    """The audit's report reaches the operator whether it passes (named
    advisories are warnings) or fails (an advisory no ledger row names)."""
    report = (
        "WARNING web/package-lock.json: left 1.0.0 carries GHSA-aaaa; named by HF-1\n"
    )
    steps = [
        _MaintenanceStep("later", ["uv", "lock", "-U"], ROOT, "lock"),
        _MaintenanceStep(
            "lock-advisory-audit",
            [sys.executable, "tools/pin_freshness.py", "--audit"],
            ROOT,
            "audit",
        ),
    ]

    def fake_run_completed(cmd, **kwargs):
        audit = cmd[-1] == "--audit"
        return subprocess.CompletedProcess(
            cmd, 1 if audit and unnamed else 0, report if audit else "", ""
        )

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION, "compiler_source_root", lambda: ROOT, raising=True
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_update_steps",
        lambda *_args, **_kwargs: (steps, []),
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION, "_run_completed_command", fake_run_completed, raising=True
    )

    assert TOOLCHAIN_VALIDATION.update_repo(json_output=False) == int(unnamed)
    captured = capsys.readouterr()
    assert report in (captured.err if unnamed else captured.out)


def test_cli_update_check_does_not_execute_or_mutate_dependencies(monkeypatch, capsys):
    projects = (
        ROOT,
        ROOT / "deploy/browser/simd-ops-rs",
        ROOT / "fuzz",
        ROOT / "runtime/molt-backend-mlir",
        ROOT / "runtime/molt-backend/fuzz",
        ROOT / "runtime/molt-runtime/fuzz",
        ROOT / "tools/proof_supervisor",
    )
    npm_projects = (
        ROOT / "deploy/cloudflare",
        ROOT / "examples/edgebox/packages/edgebox-cloudflare",
        ROOT / "tools/browser_asset_graph",
    )
    paths = [ROOT / "pyproject.toml", ROOT / "uv.lock"]
    paths.extend(
        project / name for project in projects for name in ("Cargo.toml", "Cargo.lock")
    )
    paths.extend(
        project / name
        for project in npm_projects
        for name in ("package.json", "package-lock.json")
    )
    original = {path: path.read_bytes() for path in paths}
    environment = dict(os.environ)
    monkeypatch.setattr(TOOLCHAIN_VALIDATION, "compiler_source_root", lambda: ROOT)
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_run_completed_command",
        lambda *_a, **_kw: pytest.fail("dry-run executed a dependency update"),
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_admitted_cargo_refusal",
        lambda *_a, **_kw: pytest.fail("dry-run entered command admission"),
    )
    assert (
        TOOLCHAIN_VALIDATION.update_repo(
            json_output=True,
            check_only=True,
            include_toolchains=False,
            include_locks=True,
            include_manifests=True,
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["check_only"] is True
    assert {path: path.read_bytes() for path in paths} == original
    assert dict(os.environ) == environment


@pytest.mark.parametrize("failure", ["missing-checkout", "missing-git", "unsafe-git"])
def test_update_manifest_inventory_refuses_before_execution(
    tmp_path, monkeypatch, capsys, failure
):
    from tools import dependency_manifests

    root = tmp_path / "source"
    for relative in (
        "Cargo.toml",
        "runtime/molt-runtime/Cargo.toml",
        "runtime/molt-backend/Cargo.toml",
        "src/molt/cli/__init__.py",
    ):
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    monkeypatch.setattr(TOOLCHAIN_VALIDATION, "compiler_source_root", lambda: root)
    if failure != "missing-checkout":
        error = (
            FileNotFoundError("git executable missing")
            if failure == "missing-git"
            else subprocess.CalledProcessError(
                128, ["git", "ls-files"], stderr="fatal: detected dubious ownership"
            )
        )

        def refuse_inventory(*_a, **_kw):
            raise error

        monkeypatch.setattr(
            dependency_manifests, "cargo_workspace_dirs", refuse_inventory
        )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_run_completed_command",
        lambda *_a, **_kw: pytest.fail("inventory refusal executed an update"),
    )
    assert TOOLCHAIN_VALIDATION.update_repo(json_output=True, check_only=True) != 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error" and payload["command"] == "update"
    assert "accessible Git checkout and Git executable" in payload["errors"][0]
    if failure == "unsafe-git":
        assert "dubious ownership" in payload["errors"][0]


def test_update_plan_bootstraps_missing_cargo_tool_helpers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    present = {
        "cargo": "cargo",
        "rustup": "rustup",
        "wasm-tools": "wasm-tools",
    }

    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        TOOLCHAIN_VALIDATION,
        which=lambda name: present.get(name),
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_detect_llvm_backend_toolchain",
        lambda _root: (22, None),
        raising=True,
    )

    steps, warnings = TOOLCHAIN_VALIDATION._planned_update_steps(
        ROOT,
        include_toolchains=True,
        include_locks=False,
        include_manifests=False,
    )

    names = {step.name for step in steps}
    assert "cargo-install-wasm-pack" in names
    assert "cargo-install-wasm-tools" not in names
    assert any("-m tools.bootstrap_llvm" in warning for warning in warnings)


def test_llvm_backend_advice_names_exact_prefix_and_config(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    install_module_view(
        monkeypatch, "platform", platform, SETUP_READINESS, system=lambda: "Windows"
    )

    advice = SETUP_READINESS._llvm_backend_advice(22)

    joined = "\n".join(advice)
    assert "LLVM_SYS_221_PREFIX" in joined
    assert "llvm-config.exe" in joined
    assert "-m tools.bootstrap_llvm" in joined


def test_llvm_report_distinguishes_windows_clang_without_config(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    install_module_view(
        monkeypatch, "platform", platform, SETUP_READINESS, system=lambda: "Windows"
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_required_llvm_backend_pin",
        lambda _root: LlvmBackendPin(
            major=22,
            minor=1,
            inkwell_feature="llvm22-1",
            inkwell_manifest="runtime/molt-backend-native/Cargo.toml",
            llvm_sys_version="221.0.1",
            default_release="22.1.8",
        ),
        raising=True,
    )
    present = {
        "python": "python",
        "uv": "uv",
        "cargo": "cargo",
        "rustup": "rustup",
        "rustc": "rustc",
        "cargo-upgrade": "cargo-upgrade",
        "clang": "C:/Program Files/LLVM/bin/clang.exe",
        "cmake": "cmake",
        "ninja": "ninja",
        "wasm-ld": "wasm-ld",
        "wasm-tools": "wasm-tools",
        "wasm-pack": "wasm-pack",
        "wasmtime": "wasmtime",
        "zig": "zig",
    }
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        SETUP_READINESS,
        which=lambda name: present.get(name),
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "verify_available_llvm_toolchain",
        lambda _root: None,
        raising=True,
    )

    guarded_probe_calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(cmd, **kwargs):
        guarded_probe_calls.append((cmd, kwargs))
        if cmd == ["C:/Program Files/LLVM/bin/clang.exe", "--version"]:
            return subprocess.CompletedProcess(
                cmd,
                0,
                "clang version 22.1.7\nTarget: x86_64-pc-windows-msvc\n",
                "",
            )
        if cmd == ["rustc", "--version"]:
            return subprocess.CompletedProcess(cmd, 0, "rustc 1.96.1\n", "")
        return subprocess.CompletedProcess(cmd, 1, "", "")

    monkeypatch.setattr(
        SETUP_READINESS, "_run_completed_command", fake_run, raising=True
    )

    report = SETUP_READINESS._build_toolchain_report(ROOT)
    llvm = next(
        check for check in report.checks if check["name"] == "llvm-backend-toolchain"
    )

    assert llvm["ok"] is False
    assert "clang is present" in llvm["detail"]
    assert "llvm-config" in llvm["detail"]
    guarded_commands = {tuple(command) for command, _kwargs in guarded_probe_calls}
    assert ("C:/Program Files/LLVM/bin/clang.exe", "--version") in guarded_commands
    assert ("rustc", "--version") in guarded_commands
    assert all(
        kwargs["memory_guard_prefix"] == "MOLT_BUILD"
        for _command, kwargs in guarded_probe_calls
    )


def test_windows_msvc_env_reports_inactive_dev_shell(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    install_module_view(
        monkeypatch, "platform", platform, SETUP_READINESS, system=lambda: "Windows"
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_windows_vsdevcmd_path",
        lambda: Path("C:/VS/Common7/Tools/VsDevCmd.bat"),
        raising=True,
    )
    present = {
        "python": "python",
        "uv": "uv",
        "cargo": "cargo",
        "rustup": "rustup",
        "cargo-upgrade": "cargo-upgrade",
        "clang": "clang",
        "cmake": "cmake",
        "ninja": "ninja",
        "wasm-ld": "wasm-ld",
        "wasm-tools": "wasm-tools",
        "wasm-pack": "wasm-pack",
    }
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        SETUP_READINESS,
        which=lambda name: present.get(name),
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_detect_llvm_backend_toolchain",
        lambda _root: (22, None),
        raising=True,
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_run_completed_command",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            args[0],
            0,
            "clang version 22.0.0\n",
            "",
        ),
        raising=True,
    )

    report = SETUP_READINESS._build_toolchain_report(ROOT)

    msvc = next(check for check in report.checks if check["name"] == "msvc-build-env")
    assert msvc["ok"] is False
    assert "cl.exe is not active" in msvc["detail"]
    assert any("VsDevCmd.bat" in advice for advice in msvc["advice"])


def test_windows_vsdevcmd_advice_uses_shared_installation_selection(
    tmp_path, monkeypatch
):
    from molt import platform_toolchain

    installation = tmp_path / "Visual Studio"
    script = installation / "Common7/Tools/VsDevCmd.bat"
    script.parent.mkdir(parents=True)
    script.write_text("", encoding="utf-8")
    selected = []

    def discover(component, env):
        selected.append(component)
        return installation

    install_module_view(
        monkeypatch, "platform", platform, SETUP_READINESS, system=lambda: "Windows"
    )
    monkeypatch.setattr(platform_toolchain, "visual_studio_installation", discover)
    assert SETUP_READINESS._windows_vsdevcmd_path() == script
    assert selected == ["Microsoft.VisualStudio.Component.VC.Tools.x86.x64"]


def test_update_toolchain_plan_uses_pinned_rust_and_wasi_target(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        TOOLCHAIN_VALIDATION,
        which=lambda name: "rustup" if name == "rustup" else None,
    )

    steps, warnings = TOOLCHAIN_VALIDATION._planned_update_steps(
        ROOT,
        include_toolchains=True,
        include_locks=False,
        include_manifests=False,
    )

    commands = {step.name: step.cmd for step in steps}
    install_cmd = commands["rustup-install-pinned-toolchain"]
    assert install_cmd == WASM_TOOLCHAIN.rustup_toolchain_install_cmd(ROOT)
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text("utf-8"))[
        "toolchain"
    ]["channel"]
    assert channel in install_cmd
    assert "wasm32-wasip1" in install_cmd
    assert "rustup-target-add-wasm32-wasip1" not in commands
    assert all("stable" not in command for cmd in commands.values() for command in cmd)
    assert "cargo is not installed" in "; ".join(warnings)


def test_llvm_detection_rejects_mismatched_config_major(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        SETUP_READINESS,
        "_required_llvm_backend_pin",
        lambda _root: LlvmBackendPin(
            major=22,
            minor=1,
            inkwell_feature="llvm22-1",
            inkwell_manifest="runtime/molt-backend-native/Cargo.toml",
            llvm_sys_version="221.0.1",
            default_release="22.1.8",
        ),
        raising=True,
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "verify_available_llvm_toolchain",
        lambda _root: None,
        raising=True,
    )

    assert SETUP_READINESS._detect_llvm_backend_toolchain(ROOT) == (22, None)


def test_cli_repl_command_delegates_to_guarded_repl(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli
    from molt import repl

    calls: list[dict[str, object]] = []

    def fake_run_repl(**kwargs: object) -> int:
        calls.append(kwargs)
        return 0

    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(repl, "run_repl", fake_run_repl, raising=True)
    monkeypatch.setattr(sys, "argv", ["molt", "repl", "--io-mode", "virtual"])

    assert cli.main() == 0
    assert calls == [
        {
            "capabilities": None,
            "io_mode": "virtual",
            "molt_cmd": [sys.executable, "-m", "molt.cli"],
            "timeout_sec": None,
        }
    ]


def test_cli_install_uses_memory_guard_for_venv_and_uv(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:

    deps = importlib.import_module("molt.cli.deps")
    calls: list[dict[str, object]] = []

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(deps, "_ensure_uv", lambda: "uv", raising=True)
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "user-project"\nversion = "0.0.0"\n', encoding="utf-8"
    )
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        deps, "_run_completed_command", fake_run_completed, raising=True
    )

    assert cli_deps.install(["attrs"], json_output=True) == 0

    assert [call["memory_guard_prefix"] for call in calls] == [
        "MOLT_CLI",
        "MOLT_CLI",
    ]
    assert calls[0]["cmd"][:2] == ["uv", "venv"]
    assert calls[0]["cwd"] == tmp_path
    assert calls[1]["cmd"][:3] == ["uv", "pip", "install"]
    assert calls[1]["cwd"] == tmp_path


@pytest.mark.parametrize("guard_timeout", [True, False])
def test_cli_debug_eval_command_uses_guarded_timeout(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, guard_timeout: bool
) -> None:
    """Only the guard's TimeoutExpired is a timeout; an exit status of 124 is not.

    The old evaluator also read a returned 124 with a guard-like stderr line as
    a timeout, so a command that printed that line and exited 124 was misread.
    """

    calls: list[dict[str, object]] = []

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        if guard_timeout:
            raise subprocess.TimeoutExpired(
                cmd, 1, output="", stderr="memory_guard: timeout after 1.00s\n"
            )
        return subprocess.CompletedProcess(
            cmd,
            124,
            "",
            "memory_guard: timeout after 1.00s\n",
        )

    monkeypatch.setattr(
        cli_debug_helpers, "_run_completed_command", fake_run_completed, raising=True
    )

    result = cli_debug_helpers._run_debug_eval_command(
        "python3 -c pass",
        cwd=tmp_path,
        env_updates={},
        default_manifest={"ok": True},
        timeout_sec=1,
    )

    assert result["returncode"] == 124
    assert result["timed_out"] is guard_timeout
    assert calls == [
        {
            "cmd": ["python3", "-c", "pass"],
            "cwd": tmp_path,
            "env": calls[0]["env"],
            "capture_output": True,
            "timeout": 1,
            "memory_guard_prefix": "MOLT_CLI",
        }
    ]


def test_install_wrappers_require_explicit_dependency_setup() -> None:
    shell_text = (ROOT / "packaging" / "install.sh").read_text(encoding="utf-8")
    powershell_text = (ROOT / "packaging" / "install.ps1").read_text(encoding="utf-8")
    assert '"$molt_bin" setup --strict' not in shell_text
    assert "& $moltcommand setup --strict" not in powershell_text.lower()
    for text in (shell_text, powershell_text):
        assert "setup --install-cli-dependencies" in text
        assert "No CLI dependencies or toolchains were installed" in text


def test_update_provisions_managed_validator_even_with_ambient_tool(monkeypatch):
    monkeypatch.setattr(TOOLCHAIN_VALIDATION, "pinned_executable", lambda *_a: None)
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        TOOLCHAIN_VALIDATION,
        which=lambda name: name if name == "wasm-tools" else None,
    )
    steps, _ = TOOLCHAIN_VALIDATION._planned_update_steps(
        ROOT,
        include_toolchains=True,
        include_locks=False,
        include_manifests=False,
    )
    managed = [step for step in steps if step.name == "provision-pinned-wasm-tools"]
    assert len(managed) == 1
    assert managed[0].cmd == [
        sys.executable,
        "-m",
        "molt.tool_releases",
        "provision",
        "wasm-tools",
        "--repo-root",
        str(ROOT),
    ]
    assert all(step.name != "cargo-install-wasm-tools" for step in steps)


@pytest.mark.parametrize("closure", [True, False, None])
def test_wasm_json_capture_failure_retains_only_unclosed_allocation(
    tmp_path, monkeypatch, closure
):
    from types import SimpleNamespace
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.process_environment import ProcessEnvironment

    owners = []

    def allocate(**kwargs):
        owner = OwnedTemporaryDirectory(dir=tmp_path, **kwargs)
        owners.append(owner)
        return owner

    failure = RuntimeError("guard failure")
    if closure is not None:
        failure.guarded_result = SimpleNamespace(descendants_closed=closure)

    def fail(command, **kwargs):
        path = Path(command[command.index("--guest-stderr") + 1])
        path.write_bytes(b"still-writing")
        raise failure

    monkeypatch.setattr(script_commands, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(script_commands, "_run_command_timed", fail)
    with pytest.raises(RuntimeError, match="guard failure") as caught:
        script_commands._run_program(
            ["node", "runner", "manifest"],
            env=ProcessEnvironment({}),
            cwd=tmp_path,
            json_output=True,
            verbose=False,
            timing=False,
            build_duration_s=0,
            start_receipt=None,
            wasm_guest_stderr=True,
        )
    path = Path(owners[0].name)
    assert path.exists() is (closure is not True)
    if closure is not True:
        assert any(str(path) in note for note in caught.value.__notes__)
        assert (path / "guest-stderr.bin").read_bytes() == b"still-writing"
        owners[0].cleanup()  # synthetic test has no live child


def test_wasm_json_capture_refuses_unclosed_normal_result(tmp_path, monkeypatch):
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.process_environment import ProcessEnvironment

    owners = []

    def allocate(**kwargs):
        owner = OwnedTemporaryDirectory(dir=tmp_path, **kwargs)
        owners.append(owner)
        return owner

    monkeypatch.setattr(script_commands, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(
        script_commands,
        "_run_command_timed",
        lambda *args, **kwargs: process_execution._TimedResult(0, "guest", "host", 0.1),
    )
    with pytest.raises(RuntimeError, match="process-tree closure"):
        script_commands._run_program(
            ["node", "runner", "manifest"],
            env=ProcessEnvironment({}),
            cwd=tmp_path,
            json_output=True,
            verbose=False,
            timing=False,
            build_duration_s=0,
            start_receipt=None,
            wasm_guest_stderr=True,
        )
    assert Path(owners[0].name).exists()
    owners[0].cleanup()
