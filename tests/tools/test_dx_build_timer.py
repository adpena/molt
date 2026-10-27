from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from tests.process_guard_common import start_owned_test_process
from tools import dx_build_timer
from molt.cli.build_request import BuildRequest

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


REPO_ROOT = Path(__file__).resolve().parents[2]


def _build_success(cmd, content: bytes = b"mock artifact") -> str:
    """Simulate only the CLI boundary; the timer observes real fixture files."""
    output = Path(cmd[cmd.index("--output") + 1])
    output.write_bytes(content)
    if "--json" not in cmd:
        return f"Successfully built {output}\n"
    return json.dumps(
        {
            "command": "build",
            "status": "ok",
            "data": {
                "output": str(output),
                "cache": {"hit": False},
                "observed_toolchain": {
                    "artifact": {
                        "identity": {
                            "sha256": hashlib.sha256(content).hexdigest(),
                            "size": len(content),
                        }
                    }
                },
            },
        }
    )


def _mock_build_execution(monkeypatch, module, completed_process):
    """Keep both guard routes mocked even when proof-queue flags are inherited."""
    routes = []

    def per_phase(cmd, **kwargs):
        routes.append("per-phase")
        return completed_process(cmd, **kwargs)

    def inside_outer(cmd, env, cwd, **kwargs):
        routes.append("outer")
        result = completed_process(cmd, env=env, cwd=cwd, **kwargs)
        return result, result.elapsed_s

    def unexpected_spawn(*_args, **_kwargs):
        raise AssertionError("timer fixture leaked a real subprocess launch")

    monkeypatch.setattr(
        module.harness_memory_guard, "guarded_completed_process", per_phase
    )
    monkeypatch.setattr(module, "_run_completed_inside_active_guard", inside_outer)
    monkeypatch.setattr(module, "_drain_current_session_backend_daemons", lambda _: 0)
    monkeypatch.setattr(
        module, "_COMMANDS", SimpleNamespace(start_owned=unexpected_spawn)
    )
    return routes


def test_run_uses_shared_memory_guard(monkeypatch, tmp_path: Path) -> None:
    module = dx_build_timer
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0, stdout="ok\n", stderr="err\n", elapsed_s=0.125
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc, elapsed, tail = module._run(
        ["cargo", "build"],
        {"CARGO_TARGET_DIR": str(tmp_path / "target")},
        tmp_path,
    )

    assert rc == 0
    assert elapsed == 0.125
    assert tail == "err"
    assert calls == [
        {
            "cmd": ["cargo", "build"],
            "cwd": tmp_path,
            "env": {"CARGO_TARGET_DIR": str(tmp_path / "target")},
            "capture_output": True,
            "text": True,
            "prefix": "MOLT_DX_BUILD",
            "progress_label": None,
        }
    ]


def test_a_dead_guards_leftover_variables_do_not_enable_reuse() -> None:
    # A real child that has exited and been reaped names a guard that runs no
    # more; reuse would leave the build's descendants with no guard at all.
    exited = start_owned_test_process([sys.executable, "-c", "pass"])
    exited.wait()
    env = {
        "MOLT_MEMORY_GUARD_ACTIVE": "1",
        "MOLT_MEMORY_GUARD_PID": str(exited.pid),
        "MOLT_DX_BUILD_TIMER_REUSE_OUTER_GUARD": "1",
    }
    assert dx_build_timer._outer_memory_guard_reuse_enabled(env) is False
    env["MOLT_MEMORY_GUARD_PID"] = str(os.getpid())
    assert dx_build_timer._outer_memory_guard_reuse_enabled(env) is True


def test_run_can_reuse_outer_memory_guard_for_molt_build_phases(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = dx_build_timer

    def unexpected_inner_guard(*_args, **_kwargs):
        raise AssertionError("molt-build phase should reuse the active outer guard")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        unexpected_inner_guard,
    )
    env = os.environ.copy()
    # A real guard names its own live pid; this process stands in for it.
    env["MOLT_MEMORY_GUARD_ACTIVE"] = "1"
    env["MOLT_MEMORY_GUARD_PID"] = str(os.getpid())
    env["MOLT_DX_BUILD_TIMER_REUSE_OUTER_GUARD"] = "1"

    proc, elapsed = module._run_completed(
        [
            sys.executable,
            "-c",
            "import os, sys; print(os.environ['MOLT_MEMORY_GUARD_ACTIVE']); print('err', file=sys.stderr)",
        ],
        env,
        tmp_path,
        prefer_outer_guard_when_active=True,
    )

    assert proc.returncode == 0
    assert elapsed >= 0
    assert proc.stdout == "1\n"
    assert proc.stderr == "err\n"


@pytest.mark.parametrize("failure", [None, "launch", "stream", "cleanup"])
def test_outer_timer_holds_selected_state_until_child_cleanup(
    tmp_path, monkeypatch, request, failure
):
    import gc
    import subprocess
    import weakref
    from molt.build_state_layout import build_state_root
    from molt.checkout_state import checkout_state_retirement
    from molt.dx import checkout_component
    from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
    from tests.process_guard_common import HUNG_CHILD_SOURCE, install_module_view

    module = dx_build_timer
    artifact = tmp_path / "artifacts"
    parent = artifact / "target" / "checkouts" / checkout_component(module.REPO_ROOT)
    target = parent / "sessions" / "timer-lifetime"
    target.mkdir(parents=True)
    env = ProcessEnvironmentBuilder(os.environ)
    for key in (
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_BUILD_STATE_DIR",
    ):
        env.pop(key, None)
    env.update({"MOLT_EXT_ROOT": str(artifact), "CARGO_TARGET_DIR": str(target)})
    environment = ProcessEnvironment(env)
    control = build_state_root(
        project_root=module.REPO_ROOT, cargo_target=target, environment=environment
    )
    protected = (parent, target, control)
    commands = module._COMMANDS
    processes = []
    owner_refs = []

    def stop_survivors() -> None:
        # The child never exits by itself: stop one the cleanup under test left.
        for process in processes:
            if process.poll() is None:
                try:
                    commands.wait_owned(process, timeout=0.1, terminate_timeout=5.0)
                except subprocess.TimeoutExpired:
                    pass

    request.addfinalizer(stop_survivors)
    if failure == "cleanup":
        real_atexit = module.atexit

        def register(callback):
            owner_refs.append(weakref.ref(callback))
            return real_atexit.register(callback)

        install_module_view(
            monkeypatch, "atexit", real_atexit, module, register=register
        )

    def start(*args, **kwargs):
        for path in protected:
            with checkout_state_retirement(path) as admitted:
                assert not admitted
        process = commands.start_owned(*args, **kwargs)
        processes.append(process)
        return process

    def failed_wait(*args, **kwargs):
        raise OSError("cleanup fault")

    monkeypatch.setattr(
        module,
        "_COMMANDS",
        SimpleNamespace(
            start_owned=start,
            wait_owned=failed_wait if failure == "cleanup" else commands.wait_owned,
        ),
    )
    if failure in {"stream", "cleanup"}:

        def interrupted_print(*_args, **_kwargs):
            raise RuntimeError("stream fault")

        monkeypatch.setattr(module, "print", interrupted_print, raising=False)
        monkeypatch.setattr(
            module.harness_memory_guard,
            "_subprocess_keepalive_interval_secs",
            lambda *_args, **_kwargs: 0.01,
        )
    command = (
        [str(tmp_path / "missing-timer-child")]
        if failure == "launch"
        else [
            sys.executable,
            "-I",
            "-S",
            "-c",
            HUNG_CHILD_SOURCE if failure is not None else "print('timed')",
        ]
    )
    try:
        if failure is None:
            completed, _elapsed = module._run_completed_inside_active_guard(
                command, environment, tmp_path
            )
            assert completed.returncode == 0
            assert completed.stdout == "timed\n"
        else:
            with pytest.raises(
                FileNotFoundError if failure == "launch" else RuntimeError,
                match=None if failure == "launch" else "stream fault",
            ) as caught:
                module._run_completed_inside_active_guard(
                    command, environment, tmp_path, progress_label="fixture child"
                )
            if failure == "cleanup":
                assert any(
                    "child cleanup failed" in note and "cleanup fault" in note
                    for note in caught.value.__notes__
                )
                gc.collect()
                assert processes[0].poll() is None
                assert owner_refs[0]() is not None
                for path in protected:
                    with checkout_state_retirement(path) as admitted:
                        assert not admitted
    finally:
        monkeypatch.setattr(module, "_COMMANDS", commands)
        if owner_refs and (owner := owner_refs[0]()) is not None:
            owner()
        elif processes and processes[0].poll() is None:
            try:
                commands.wait_owned(processes[0], timeout=1.0, terminate_timeout=1.0)
            except subprocess.TimeoutExpired:
                assert processes[0].poll() is not None
    assert all(process.poll() is not None for process in processes)
    for path in protected:
        with checkout_state_retirement(path) as admitted:
            assert admitted


def test_touch_journal_restores_and_recovers_crash_left_marker(tmp_path: Path) -> None:
    module = dx_build_timer
    source = tmp_path / "value_range.rs"
    original = b"fn value_range() {}\n"
    source.write_bytes(original)
    journal = module.TouchJournal(tmp_path / "target" / ".dx_build_timer_touches.json")

    entry = journal.touch(source)
    assert source.read_bytes() == original + module.TOUCH_MARKER
    journal.restore(entry)
    assert source.read_bytes() == original
    assert not journal.path.exists()

    journal.touch(source)
    assert source.read_bytes() == original + module.TOUCH_MARKER
    module.TouchJournal(journal.path).recover()
    assert source.read_bytes() == original
    assert not journal.path.exists()


def test_touch_journal_refuses_to_overwrite_external_edit(tmp_path: Path) -> None:
    module = dx_build_timer
    source = tmp_path / "function_compiler.rs"
    source.write_bytes(b"fn before() {}\n")
    journal = module.TouchJournal(tmp_path / "target" / ".dx_build_timer_touches.json")

    entry = journal.touch(source)
    source.write_bytes(b"fn edited_elsewhere() {}\n")

    with pytest.raises(RuntimeError, match="content changed outside dx_build_timer"):
        journal.restore(entry)
    assert json.loads(journal.path.read_text(encoding="utf-8"))["entries"] == [entry]


def test_default_touch_files_track_current_split_modules() -> None:
    module = dx_build_timer
    touch_files = module._touch_files(REPO_ROOT)

    assert touch_files["value_range"] == (
        REPO_ROOT / "runtime/molt-passes/src/tir/passes/value_range/mod.rs"
    )
    assert touch_files["value_range"].exists()
    assert touch_files["function_compiler"].exists()
    assert touch_files["modules"].exists()
    assert (
        touch_files["gvn"]
        == REPO_ROOT / "runtime/molt-passes/src/tir/passes/gvn/mod.rs"
    )
    assert touch_files["gvn"].exists()
    assert (
        module._scenario_preflight_errors(
            [
                "inc-value_range",
                "inc-gvn",
                "inc-function_compiler",
                "inc-modules",
                "test-lib",
            ],
            touch_files,
        )
        == []
    )


def test_scenario_preflight_fails_before_prime_for_stale_touch_path(
    tmp_path: Path,
) -> None:
    module = dx_build_timer
    errors = module._scenario_preflight_errors(
        ["inc-value_range", "molt-build-unknown", "unknown-shape"],
        {"value_range": tmp_path / "missing_value_range.rs"},
    )

    assert len(errors) == 3
    assert (
        "scenario=inc-value_range touch_key=value_range missing touch_path="
        in errors[0]
    )
    assert (
        "scenario=molt-build-unknown unknown molt build target; choices=" in errors[1]
    )
    assert "unknown scenario: unknown-shape" == errors[2]


def test_write_snapshot_records_active_command(tmp_path: Path) -> None:
    module = dx_build_timer
    out = tmp_path / "timer.json"
    args = SimpleNamespace(
        profile="release-fast",
        test_profile="dev-fast",
        package="molt-backend",
        bin_name="molt-backend",
        features="native-backend",
        runs=2,
        target_dir=str(tmp_path / "target"),
        json_out=str(out),
    )

    module._write_snapshot(
        args,
        {"inc-value_range": {"samples_sec": [1.25], "rc": 0}},
        cargo_version="cargo 1.96.1",
        prime={"elapsed_sec": 0.5, "rc": 0, "cmd": ["cargo", "build"]},
        active={"label": "test-lib", "run": 1, "cmd": ["cargo", "test"]},
    )

    payload = json.loads(out.read_text(encoding="utf-8"))
    assert payload["meta"]["target_dir"] == str(tmp_path / "target")
    assert payload["meta"]["bin"] == "molt-backend"
    assert payload["meta"]["profile"] == "release-fast"
    assert payload["meta"]["test_profile"] == "dev-fast"
    assert payload["prime"]["elapsed_sec"] == 0.5
    assert payload["active"] == {
        "label": "test-lib",
        "run": 1,
        "cmd": ["cargo", "test"],
    }
    assert payload["results"]["inc-value_range"]["samples_sec"] == [1.25]


def test_build_cmd_scopes_to_daemon_binary() -> None:
    module = dx_build_timer
    args = SimpleNamespace(
        profile="release-fast",
        package="molt-backend",
        bin_name="molt-backend",
        features="native-backend",
    )

    assert module._build_cmd(args) == [
        "cargo",
        "build",
        "--profile",
        "release-fast",
        "-p",
        "molt-backend",
        "--bin",
        "molt-backend",
        "--features",
        "native-backend",
    ]


def test_test_lib_cmd_scopes_to_library_target() -> None:
    module = dx_build_timer
    args = SimpleNamespace(
        profile="release-fast",
        test_profile="dev-fast",
        package="molt-backend",
        features="native-backend",
    )

    assert module._test_build_cmd(args) == [
        "cargo",
        "test",
        "--profile",
        "dev-fast",
        "-p",
        "molt-backend",
        "--features",
        "native-backend",
        "--lib",
        "--no-run",
    ]


def test_molt_build_command_wires_split_runtime_diagnostics(tmp_path: Path) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    out_dir = tmp_path / "out"
    diagnostics = tmp_path / "diagnostics.json"
    python = module._molt_build_python_executable()

    assert module._molt_build_command(
        source=source,
        target="wasm-split",
        profile="cloudflare",
        out_dir=out_dir,
        output=out_dir / "program.wasm",
        measurement=module.MoltMeasurement.ATTRIBUTION,
        cache_dir=tmp_path / "cache",
        diagnostics_file=diagnostics,
    ) == [
        python,
        "-m",
        "molt.cli",
        "build",
        str(source),
        "--target",
        "wasm",
        "--profile",
        "cloudflare",
        "--out-dir",
        str(out_dir),
        "--cache-dir",
        str(tmp_path / "cache"),
        "--output",
        str(out_dir / "program.wasm"),
        "--cache-report",
        "--json",
        "--diagnostics",
        "--diagnostics-file",
        str(diagnostics),
        "--split-runtime",
    ]


def test_molt_build_python_prefers_uv_project_environment(tmp_path: Path) -> None:
    module = dx_build_timer
    uv_env = tmp_path / "uv-project"
    python = uv_env / "Scripts" / "python.exe"
    python.parent.mkdir(parents=True)
    python.write_text("", encoding="utf-8")

    assert module._molt_build_python_executable(
        {
            "UV_PROJECT_ENVIRONMENT": str(uv_env),
            "VIRTUAL_ENV": str(tmp_path / "other-env"),
        }
    ) == str(python)


@pytest.mark.parametrize("config_key", ["cache_dir", "cache-dir"])
def test_trial_cache_overrides_project_config_through_real_dispatch(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    config_key: str,
) -> None:
    from molt.cli import entrypoint_dispatch, entrypoint_parser
    from molt.cli.build_output_layout import _resolve_cache_root

    module = dx_build_timer
    trial = tmp_path / "trial-cache"
    shared = tmp_path / "shared-project-cache"
    monkeypatch.setenv("MOLT_CACHE", str(trial))
    command = module._molt_build_command(
        source=tmp_path / "hello.py",
        target="native",
        profile="dev",
        out_dir=tmp_path / "out",
        output=tmp_path / "out" / "program.bin",
        measurement=module.MoltMeasurement.LATENCY,
        cache_dir=trial,
        diagnostics_file=tmp_path / "diagnostics.json",
    )
    resolved = []

    def observe_build(request: BuildRequest) -> int:
        resolved.append(_resolve_cache_root(tmp_path, request.cache_dir))
        return 0

    # Negative control: environment-only isolation loses to this project config.
    flag = command.index("--cache-dir")
    without_explicit_cache = command[:flag] + command[flag + 2 :]
    for candidate in (without_explicit_cache, command):
        parser = entrypoint_parser._build_entrypoint_parser()
        args = parser.parse_args(candidate[3:])
        assert (
            entrypoint_dispatch._dispatch_entrypoint_command(
                args,
                parser=parser,
                build_fn=observe_build,
                config_root=tmp_path,
                config={},
                build_cfg={config_key: str(shared)},
                run_cfg={},
                compare_cfg={},
                test_cfg={},
                diff_cfg={},
                extension_cfg={},
                publish_cfg={},
                cfg_capabilities=None,
            )
            == 0
        )
    assert resolved == [shared, trial]


def test_molt_build_output_root_defaults_to_json_stem_for_evidence_custody(
    tmp_path: Path,
) -> None:
    module = dx_build_timer

    assert (
        module._molt_build_output_root(
            SimpleNamespace(
                target_dir=str(tmp_path / "target"),
                json_out=str(tmp_path / "proof" / "row.json"),
                molt_output_root=None,
            )
        )
        == (tmp_path / "proof" / "row.molt-builds").resolve()
    )

    assert (
        module._molt_build_output_root(
            SimpleNamespace(
                target_dir=str(tmp_path / "target"),
                json_out=str(tmp_path / "proof" / "row.json"),
                molt_output_root=str(tmp_path / "explicit"),
            )
        )
        == (tmp_path / "explicit").resolve()
    )


def test_main_repairs_target_after_restored_touch(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = dx_build_timer
    target = tmp_path / "target"
    out = tmp_path / "timer.json"
    source = tmp_path / "value_range.rs"
    original = b"fn value_range() {}\n"
    source.write_bytes(original)
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        stdout = "cargo 1.96.1\n" if list(cmd) == ["cargo", "--version"] else ""
        return SimpleNamespace(
            returncode=0,
            stdout=stdout,
            stderr="",
            elapsed_s=float(len(calls)),
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setattr(
        module,
        "_touch_files",
        lambda _repo_root: {"value_range": source},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "1",
            "--target-dir",
            str(target),
            "--scenarios",
            "test-lib",
            "--json-out",
            str(out),
        ],
    )

    assert module.main() == 0

    payload = json.loads(out.read_text(encoding="utf-8"))
    assert source.read_bytes() == original
    assert not (target / ".dx_build_timer_touches.json").exists()
    assert [call["progress_label"] for call in calls] == [
        None,
        "dx-build prime",
        "dx-build test-lib run 1/1",
        "dx-build test-lib repair 1/1",
    ]
    assert payload["results"]["test-lib"]["samples_sec"] == [3.0]
    assert payload["results"]["test-lib"]["repair_samples_sec"] == [4.0]
    assert payload["results"]["test-lib"]["repair_rc"] == 0
    assert payload["results"]["test-lib"]["repair_cmd"] == module._build_cmd(
        SimpleNamespace(
            profile="release-fast",
            package="molt-backend",
            bin_name="molt-backend",
            features="native-backend",
        )
    )


@pytest.mark.parametrize("outer_guard", [False, True])
def test_main_molt_build_scenario_skips_daemon_prime_and_records_phases(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    outer_guard: bool,
) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    target = tmp_path / "target"
    output_root = tmp_path / "molt-builds"
    out = tmp_path / "timer.json"
    calls: list[dict[str, object]] = []
    drain_envs: list[dict[str, str]] = []
    guard_routes = []
    # Only mocked child boundaries below observe this synthetic guard policy.
    monkeypatch.setenv("MOLT_MEMORY_GUARD_ACTIVE", "1" if outer_guard else "0")
    monkeypatch.setenv("MOLT_MEMORY_GUARD_PID", str(os.getpid()))
    monkeypatch.setenv("MOLT_DX_BUILD_TIMER_REUSE_OUTER_GUARD", "1")
    monkeypatch.setenv("MOLT_PROOF_QUEUE", "0")

    def fake_guarded_completed_process(cmd, **kwargs):
        guard_routes.append("per-phase")
        calls.append({"cmd": list(cmd), **kwargs})
        stdout = (
            "cargo 1.96.1\n"
            if list(cmd) == ["cargo", "--version"]
            else _build_success(cmd)
        )
        if list(cmd) != ["cargo", "--version"]:
            output = Path(cmd[cmd.index("--output") + 1])
            (output.parent / "unrelated.log").write_bytes(b"x" * 8192)
        return SimpleNamespace(
            returncode=0,
            stdout=stdout,
            stderr="",
            elapsed_s=float(len(calls)),
        )

    def fake_outer_guard(cmd, env, cwd, **kwargs):
        result = fake_guarded_completed_process(cmd, env=env, cwd=cwd, **kwargs)
        guard_routes[-1] = "outer"
        return result, result.elapsed_s

    def fake_drain_current_session_backend_daemons(env):
        drain_envs.append(dict(env))
        return 2

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setattr(
        module,
        "_drain_current_session_backend_daemons",
        fake_drain_current_session_backend_daemons,
    )
    monkeypatch.setattr(module, "_run_completed_inside_active_guard", fake_outer_guard)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "1",
            "--target-dir",
            str(target),
            "--scenarios",
            "molt-build-native",
            "--molt-source",
            str(source),
            "--molt-output-root",
            str(output_root),
            "--json-out",
            str(out),
        ],
    )

    assert module.main() == 0

    payload = json.loads(out.read_text(encoding="utf-8"))
    assert "prime" not in payload
    assert len(drain_envs) == 1
    assert [call["progress_label"] for call in calls] == [
        None,
        "dx-build molt-build-native cold run 1/1",
        "dx-build molt-build-native warm run 1/1",
    ]
    result = payload["results"]["molt-build-native"]
    assert result["target"] == "native"
    assert result["profile"] == "dev"
    assert result["backend_daemons_drained"] == 2
    assert guard_routes == ["per-phase"] + ["outer" if outer_guard else "per-phase"] * 2
    assert result["daemon_policy"].startswith(
        "outer-guard-reuse" if outer_guard else "per-phase-guard"
    )
    assert result["daemon_policy"] in result["cold_scope"]
    if not outer_guard:
        assert "daemon retention not assumed" in result["cold_scope"]
    assert [phase["phase"] for phase in result["phases"]] == ["cold", "warm"]
    assert [phase["returncode"] for phase in result["phases"]] == [0, 0]
    assert result["measurement"] == "latency"
    assert source.read_bytes() == b"print(42)\n"
    python = module._molt_build_python_executable()
    for phase in result["phases"]:
        command = phase["command"]
        assert command[:4] == [python, "-m", "molt.cli", "build"]
        assert "--output" in command
        assert not {
            "--cache-report",
            "--json",
            "--diagnostics",
            "--diagnostics-file",
        } & set(command)
        assert "[tool.molt]" in (
            Path(phase["source"]).parent / "pyproject.toml"
        ).read_text(encoding="utf-8")
        assert phase["measurement"] == "latency"
        assert phase["output_size_bytes"] == len(b"mock artifact")
        assert phase["artifact"]["cache"] is None
        assert phase["artifact"]["observed_toolchain"] is None
        assert phase["artifact"]["correctness_verified"] is False


@pytest.mark.parametrize("proof_queue", [False, True])
def test_main_molt_build_default_output_root_records_real_diagnostics_paths(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    proof_queue: bool,
) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    target = tmp_path / "target"
    out = tmp_path / "proof" / "row.json"
    calls: list[dict[str, object]] = []
    monkeypatch.setenv("MOLT_MEMORY_GUARD_ACTIVE", "1")
    monkeypatch.setenv("MOLT_MEMORY_GUARD_PID", str(os.getpid()))
    monkeypatch.setenv("MOLT_PROOF_QUEUE", "1" if proof_queue else "0")
    monkeypatch.setenv("MOLT_DX_BUILD_TIMER_REUSE_OUTER_GUARD", "0")

    def fake_guarded_completed_process(cmd, **kwargs):
        cmd = list(cmd)
        calls.append({"cmd": cmd, **kwargs})
        if cmd == ["cargo", "--version"]:
            stdout = "cargo 1.96.1\n"
        else:
            diagnostics_file = Path(cmd[cmd.index("--diagnostics-file") + 1])
            diagnostics_file.parent.mkdir(parents=True, exist_ok=True)
            diagnostics_file.write_text('{"ok": true}\n', encoding="utf-8")
            stdout = _build_success(cmd)
        return SimpleNamespace(
            returncode=0,
            stdout=stdout,
            stderr="",
            elapsed_s=float(len(calls)),
        )

    monkeypatch.chdir(tmp_path)
    routes = _mock_build_execution(monkeypatch, module, fake_guarded_completed_process)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "1",
            "--target-dir",
            str(target),
            "--scenarios",
            "molt-build-native",
            "--molt-measurement",
            "attribution",
            "--molt-source",
            str(source),
            "--json-out",
            str(Path("proof") / "row.json"),
        ],
    )

    assert module.main() == 0

    payload = json.loads(out.read_text(encoding="utf-8"))
    expected_root = (tmp_path / "proof" / "row.molt-builds").resolve()
    assert routes == ["per-phase"] + ["outer" if proof_queue else "per-phase"] * 2
    assert payload["meta"]["molt_output_root_resolved"] == str(expected_root)
    for phase in payload["results"]["molt-build-native"]["phases"]:
        diagnostics_path = Path(phase["diagnostics_file"])
        assert diagnostics_path.is_absolute()
        assert diagnostics_path.is_file()
        assert expected_root in diagnostics_path.parents
        assert ".molt_build" not in diagnostics_path.parts


@pytest.mark.parametrize("cold_clean", [False, True])
@pytest.mark.parametrize("relative_override", [None, "MOLT_HOME", "MOLT_CACHE"])
def test_guest_cache_isolation_retains_compiler_home_and_original_witnesses(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    cold_clean: bool,
    relative_override: str | None,
) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    original = b"print(42)\n"
    source.write_bytes(original)
    companion = tmp_path / "edited.py"
    edited = b"print(43)\n"
    companion.write_bytes(edited)
    ambient_cache = tmp_path / "shared-cache"
    ambient_cache.mkdir()
    (ambient_cache / "do-not-delete").write_bytes(b"shared")
    target = tmp_path / "target"
    target.mkdir()
    (target / "compiler").write_bytes(b"provisioned")
    outputs = tmp_path / "outputs"
    retained = outputs / "molt-build-native" / "run-1"
    retained.mkdir(parents=True)
    (retained / "prior-evidence").write_bytes(b"keep")
    out = tmp_path / "result.json"
    monkeypatch.setenv("MOLT_CACHE", str(ambient_cache))
    monkeypatch.delenv("MOLT_HOME", raising=False)
    expected_home = (ambient_cache / "home").resolve()
    if relative_override:
        # CLI children run in the repository, not the launching shell's cwd.
        monkeypatch.chdir(tmp_path)
        monkeypatch.setenv(relative_override, "relative-compiler-location")
        expected_home = REPO_ROOT / "relative-compiler-location"
        if relative_override == "MOLT_CACHE":
            expected_home /= "home"
    observed = []

    def run(cmd, **kwargs):
        if list(cmd) == ["cargo", "--version"]:
            stdout = "cargo fixture"
        else:
            env = kwargs["env"]
            cache = Path(env["MOLT_CACHE"])
            marker = cache / "compiled-before"
            phase = len(observed) % 3
            assert marker.exists() == (phase != 0)
            assert env["MOLT_HOME"] == str(expected_home)
            assert env["CARGO_TARGET_DIR"] == str(target)
            marker.write_bytes(b"compiled")
            assert cache.is_relative_to(outputs)
            observed.append(cache)
            stdout = _build_success(cmd, f"artifact-{len(observed)}".encode())
        return SimpleNamespace(returncode=0, stdout=stdout, stderr="", elapsed_s=0.25)

    _mock_build_execution(monkeypatch, module, run)
    monkeypatch.setattr(module, "_drain_current_session_backend_daemons", lambda _: 0)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "2",
            "--target-dir",
            str(target),
            "--scenarios",
            "molt-build-native",
            "--molt-source",
            str(source),
            "--molt-edited-source",
            str(companion),
            "--molt-output-root",
            str(outputs),
            "--json-out",
            str(out),
            *(["--cold-clean"] if cold_clean else []),
        ],
    )
    assert module.main() == 0
    payload = json.loads(out.read_text(encoding="utf-8"))
    assert payload["provenance"]["authoritative"] is False
    assert payload["provenance"]["requested_profile"] == "dev"
    result = payload["results"]["molt-build-native"]
    assert result["workflow"] == "source-checkout-diagnostic"
    assert result["driver"]["installed_distribution_verified"] is False
    assert observed[0] == observed[1] == observed[2]
    assert observed[3] == observed[4] == observed[5] != observed[0]
    for index, phase in enumerate(result["phases"]):
        witness = Path(phase["source_identity"]["witness"])
        content = witness.read_bytes()
        expected = original if index % 3 != 2 else edited
        assert content == expected
        assert phase["source_identity"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert phase["source_after_sha256"] == phase["source_identity"]["sha256"]
        assert (
            phase["artifact"]["identity"]["sha256"]
            == hashlib.sha256(f"artifact-{index + 1}".encode()).hexdigest()
        )
        assert phase["artifact"]["correctness_verified"] is False
        assert phase["cache_contract"]["empty_before_cold"] is True
        assert not phase["validation_errors"]
    assert source.read_bytes() == original
    assert companion.read_bytes() == edited
    assert (ambient_cache / "do-not-delete").read_bytes() == b"shared"
    assert (target / "compiler").read_bytes() == b"provisioned"
    assert (retained / "prior-evidence").read_bytes() == b"keep"


@pytest.mark.parametrize(
    "defect",
    [
        "source_mutation",
        "artifact_digest_mismatch",
        "missing_artifact",
        "failed_build",
        "invalid_json",
    ],
)
def test_invalid_build_preserves_failure_evidence_and_stops_phases(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    defect: str,
) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    source.write_bytes(b"print(42)\n")
    out = tmp_path / "result.json"
    calls = []

    def run(cmd, **kwargs):
        if list(cmd) == ["cargo", "--version"]:
            return SimpleNamespace(
                returncode=0, stdout="cargo fixture", stderr="", elapsed_s=0.1
            )
        calls.append(list(cmd))
        stdout = _build_success(cmd, b"current")
        artifact = Path(json.loads(stdout)["data"]["output"])
        if defect == "source_mutation":
            Path(cmd[4]).write_bytes(b"print(99)\n")
        elif defect == "artifact_digest_mismatch":
            artifact.write_bytes(b"earlier")
        elif defect == "missing_artifact":
            artifact.unlink()
        elif defect == "invalid_json":
            stdout = "invalid output"
        return SimpleNamespace(
            returncode=7 if defect == "failed_build" else 0,
            stdout=stdout,
            stderr="fixture diagnostic\n",
            elapsed_s=0.1,
        )

    _mock_build_execution(monkeypatch, module, run)
    monkeypatch.setattr(module, "_drain_current_session_backend_daemons", lambda _: 0)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "1",
            "--target-dir",
            str(tmp_path / "target"),
            "--scenarios",
            "molt-build-native",
            "--molt-measurement",
            "attribution",
            "--molt-source",
            str(source),
            "--json-out",
            str(out),
        ],
    )
    assert module.main() == 1
    assert len(calls) == 1
    phase = json.loads(out.read_text(encoding="utf-8"))["results"]["molt-build-native"][
        "phases"
    ][0]
    assert Path(phase["source_identity"]["witness"]).read_bytes() == b"print(42)\n"
    assert (
        Path(phase["stderr_file"]).read_text(encoding="utf-8") == "fixture diagnostic\n"
    )
    assert Path(phase["stdout_file"]).is_file()
    assert phase["artifact"] is None
    if defect == "failed_build":
        assert phase["returncode"] == 7
    else:
        assert phase["returncode"] == 0
        assert phase["validation_errors"]
        if defect == "source_mutation":
            assert "source changed" in phase["validation_errors"][0]
        elif defect == "artifact_digest_mismatch":
            assert "differs from CLI" in phase["validation_errors"][0]


@pytest.mark.parametrize("mutation_point", ["warm_snapshot", "edit_write"])
def test_edit_rejects_source_mutation_without_adopting_it_as_expected(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    mutation_point: str,
) -> None:
    module = dx_build_timer
    source = tmp_path / "hello.py"
    original = b"print(42)\n"
    source.write_bytes(original)
    companion = tmp_path / "edited.py"
    edited = b"print(43)\n"
    companion.write_bytes(edited)
    out = tmp_path / "result.json"
    calls, writes = [], []
    snapshot = module._write_snapshot
    apply_edit = module._apply_python_edit
    injected = False

    def run(cmd, **kwargs):
        if list(cmd) == ["cargo", "--version"]:
            stdout = "cargo fixture"
        else:
            calls.append(list(cmd))
            stdout = _build_success(cmd)
        return SimpleNamespace(returncode=0, stdout=stdout, stderr="", elapsed_s=0.1)

    def inject_at_snapshot(args, results, **kwargs):
        nonlocal injected
        snapshot(args, results, **kwargs)
        phases = results.get("molt-build-native", {}).get("phases", [])
        if mutation_point == "warm_snapshot" and len(phases) == 2 and not injected:
            Path(phases[-1]["source"]).write_bytes(b"print(99)\n")
            injected = True

    def inject_at_write(path, before, after):
        writes.append(path)
        apply_edit(path, before, after)
        # Mutation after the owned transition returns must not become expected.
        if mutation_point == "edit_write":
            path.write_bytes(b"print(99)\n" + after)

    _mock_build_execution(monkeypatch, module, run)
    monkeypatch.setattr(module, "_drain_current_session_backend_daemons", lambda _: 0)
    monkeypatch.setattr(module, "_write_snapshot", inject_at_snapshot)
    monkeypatch.setattr(module, "_apply_python_edit", inject_at_write)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--runs",
            "1",
            "--target-dir",
            str(tmp_path / "target"),
            "--scenarios",
            "molt-build-native",
            "--molt-source",
            str(source),
            "--molt-edited-source",
            str(companion),
            "--json-out",
            str(out),
        ],
    )
    assert module.main() == 1
    assert len(calls) == 2
    assert len(writes) == (0 if mutation_point == "warm_snapshot" else 1)
    result = json.loads(out.read_text(encoding="utf-8"))["results"]["molt-build-native"]
    assert "scenario source changed between build phases" in result["error"]
    assert [phase["phase"] for phase in result["phases"]] == ["cold", "warm"]
    for phase in result["phases"]:
        assert Path(phase["source_identity"]["witness"]).read_bytes() == original
        assert (
            phase["source_identity"]["sha256"] == hashlib.sha256(original).hexdigest()
        )
        assert Path(phase["stdout_file"]).is_file()
    assert source.read_bytes() == original


@pytest.mark.parametrize("measurement", ["latency", "attribution"])
@pytest.mark.parametrize("target", ["native", "wasm", "wasm-split"])
def test_measurement_command_preserves_real_cli_cutoff_admission(
    tmp_path, monkeypatch, measurement, target
):
    from molt.cli import build_command, entrypoint_dispatch, entrypoint_parser

    for variable in (
        "MOLT_CLI_PROFILE",
        "MOLT_BUILD_DIAGNOSTICS",
        "MOLT_BUILD_DIAGNOSTICS_FILE",
        "MOLT_EXTERNAL_STATIC_PACKAGES",
    ):
        monkeypatch.delenv(variable, raising=False)
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    source = tmp_path / "main.py"
    source.write_bytes(b"print(42)\n")
    output = (
        tmp_path / "out" / ("program.bin" if target == "native" else "program.wasm")
    )
    command = dx_build_timer._molt_build_command(
        source=source,
        target=target,
        profile="dev",
        out_dir=output.parent,
        output=output,
        measurement=dx_build_timer.MoltMeasurement(measurement),
        cache_dir=tmp_path / "cache",
        diagnostics_file=tmp_path / "diagnostics.json",
    )
    flags = {"--json", "--cache-report", "--diagnostics", "--diagnostics-file"}
    assert flags.intersection(command) == (
        flags if measurement == "attribution" else set()
    )
    assert command[command.index("--output") + 1] == str(output)
    admitted = []

    def observe(request):
        admitted.append(build_command._early_cutoff_request(request))
        assert request.output == str(output)
        return 0

    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(command[3:])
    assert (
        entrypoint_dispatch._dispatch_entrypoint_command(
            args,
            parser=parser,
            build_fn=observe,
            config_root=tmp_path,
            config={},
            build_cfg={},
            run_cfg={},
            compare_cfg={},
            test_cfg={},
            diff_cfg={},
            extension_cfg={},
            publish_cfg={},
            cfg_capabilities=None,
        )
        == 0
    )
    assert len(admitted) == 1
    assert (admitted[0] is not None) is (
        measurement == "latency" and target == "native"
    )
    assert ("--split-runtime" in command) is (target == "wasm-split")


@pytest.mark.parametrize("measurement", ["latency", "attribution"])
@pytest.mark.parametrize(
    "defect", [None, "missing", "directory", "symlink", "reported_elsewhere"]
)
def test_artifact_observation_binds_exact_owned_output(tmp_path, measurement, defect):
    if measurement == "latency" and defect == "reported_elsewhere":
        # Ordinary stdout is deliberately not an artifact-location authority.
        stdout = '{"command":"build","status":"ok","data":{"output":"foreign"}}'
    else:
        stdout = "ordinary non-JSON compiler output"
    output = tmp_path / "program.bin"
    sibling = tmp_path / "other.bin"
    content = b"owned compiled artifact"
    sibling.write_bytes(content)
    if defect == "directory":
        output.mkdir()
    elif defect == "symlink":
        try:
            output.symlink_to(sibling)
        except OSError:
            pytest.skip("host does not permit symbolic links")
    elif defect != "missing":
        output.write_bytes(content)
    if measurement == "attribution":
        stdout = json.dumps(
            {
                "command": "build",
                "status": "ok",
                "data": {
                    "output": str(
                        sibling if defect == "reported_elsewhere" else output
                    ),
                    "cache": {"hit": False},
                },
            }
        )
    mode = dx_build_timer.MoltMeasurement(measurement)
    if defect in {"missing", "directory", "symlink"} or (
        defect == "reported_elsewhere" and measurement == "attribution"
    ):
        with pytest.raises((OSError, ValueError)):
            dx_build_timer._molt_build_artifact(stdout, output, mode)
    else:
        artifact = dx_build_timer._molt_build_artifact(stdout, output, mode)
        assert artifact["path"] == str(output)
        assert artifact["identity"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert artifact["correctness_verified"] is False
        if measurement == "latency":
            assert artifact["cache"] is None
            assert artifact["observed_toolchain"] is None


@pytest.mark.parametrize(
    "variable",
    [
        "MOLT_CLI_PROFILE",
        "MOLT_BUILD_DIAGNOSTICS",
        "MOLT_BUILD_DIAGNOSTICS_FILE",
    ],
)
def test_latency_refuses_inherited_instrumentation_before_any_command(
    tmp_path, monkeypatch, variable, capsys
):
    source = tmp_path / "main.py"
    source.write_bytes(b"print(42)\n")
    monkeypatch.setenv(variable, "instrumented")
    monkeypatch.setattr(
        dx_build_timer,
        "_run_completed",
        lambda *_a, **_k: pytest.fail(
            "instrumented latency request launched a command"
        ),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--target-dir",
            str(tmp_path / "target"),
            "--scenarios",
            "molt-build-native",
            "--molt-source",
            str(source),
        ],
    )
    assert dx_build_timer.main() == 2
    assert variable in capsys.readouterr().err


def test_measurement_mode_rejects_unknown_value_before_commands(tmp_path, monkeypatch):
    monkeypatch.setattr(
        dx_build_timer,
        "_run_completed",
        lambda *_a, **_k: pytest.fail("unknown mode launched a command"),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "dx_build_timer.py",
            "--target-dir",
            str(tmp_path / "target"),
            "--molt-measurement",
            "fast-ish",
        ],
    )
    with pytest.raises(SystemExit) as failure:
        dx_build_timer.main()
    assert failure.value.code == 2


def test_explicit_edit_preserves_companion_bytes_and_refuses_foreign_original(tmp_path):
    source = tmp_path / "main.py"
    before = b"def answer():\n    return 42\nprint(answer())\n"
    after = b"def answer():\n    return 43\nprint(answer())\n"
    source.write_bytes(before)
    dx_build_timer._apply_python_edit(source, before, after)
    assert source.read_bytes() == after
    foreign = b"print('foreign owner')\n"
    source.write_bytes(foreign)
    with pytest.raises(ValueError):
        dx_build_timer._apply_python_edit(source, before, after)
    assert source.read_bytes() == foreign
