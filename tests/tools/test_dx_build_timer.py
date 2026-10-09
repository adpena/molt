from __future__ import annotations

import importlib.util
import hashlib
import inspect
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
DX_BUILD_TIMER = REPO_ROOT / "tools" / "dx_build_timer.py"


def _load_dx_build_timer():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_dx_build_timer",
        DX_BUILD_TIMER,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _build_success(cmd, content: bytes = b"mock artifact") -> str:
    """Simulate only the CLI boundary; the timer observes real fixture files."""
    output = Path(cmd[cmd.index("--out-dir") + 1]) / "program.bin"
    output.write_bytes(content)
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
    module = _load_dx_build_timer()
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


def test_run_can_reuse_outer_memory_guard_for_molt_build_phases(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = _load_dx_build_timer()

    def unexpected_inner_guard(*_args, **_kwargs):
        raise AssertionError("molt-build phase should reuse the active outer guard")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        unexpected_inner_guard,
    )
    env = os.environ.copy()
    env["MOLT_MEMORY_GUARD_ACTIVE"] = "1"
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


def test_touch_journal_restores_and_recovers_crash_left_marker(tmp_path: Path) -> None:
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
    source = tmp_path / "function_compiler.rs"
    source.write_bytes(b"fn before() {}\n")
    journal = module.TouchJournal(tmp_path / "target" / ".dx_build_timer_touches.json")

    entry = journal.touch(source)
    source.write_bytes(b"fn edited_elsewhere() {}\n")

    with pytest.raises(RuntimeError, match="content changed outside dx_build_timer"):
        journal.restore(entry)
    assert json.loads(journal.path.read_text(encoding="utf-8"))["entries"] == [entry]


def test_default_touch_files_track_current_split_modules() -> None:
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    out_dir = tmp_path / "out"
    diagnostics = tmp_path / "diagnostics.json"
    python = module._molt_build_python_executable()

    assert module._molt_build_command(
        source=source,
        target="wasm-split",
        profile="cloudflare",
        out_dir=out_dir,
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
        "--cache-report",
        "--json",
        "--diagnostics",
        "--diagnostics-file",
        str(diagnostics),
        "--split-runtime",
    ]


def test_molt_build_python_prefers_uv_project_environment(tmp_path: Path) -> None:
    module = _load_dx_build_timer()
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
    from molt import cli
    from molt.cli import entrypoint_dispatch, entrypoint_parser
    from molt.cli.build_output_layout import _resolve_cache_root

    module = _load_dx_build_timer()
    trial = tmp_path / "trial-cache"
    shared = tmp_path / "shared-project-cache"
    monkeypatch.setenv("MOLT_CACHE", str(trial))
    command = module._molt_build_command(
        source=tmp_path / "hello.py",
        target="native",
        profile="dev",
        out_dir=tmp_path / "out",
        cache_dir=trial,
        diagnostics_file=tmp_path / "diagnostics.json",
    )
    resolved = []

    def observe_build(*args, **kwargs):
        selected = (
            inspect.signature(cli.build).bind(*args, **kwargs).arguments["cache_dir"]
        )
        resolved.append(_resolve_cache_root(tmp_path, selected))
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
    module = _load_dx_build_timer()

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
    module = _load_dx_build_timer()
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
    module = _load_dx_build_timer()
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
        "dx-build molt-build-native edit run 1/1",
    ]
    result = payload["results"]["molt-build-native"]
    assert result["target"] == "native"
    assert result["profile"] == "dev"
    assert result["backend_daemons_drained"] == 2
    assert guard_routes == ["per-phase"] + ["outer" if outer_guard else "per-phase"] * 3
    assert result["daemon_policy"].startswith(
        "outer-guard-reuse" if outer_guard else "per-phase-guard"
    )
    assert result["daemon_policy"] in result["cold_scope"]
    if not outer_guard:
        assert "daemon retention not assumed" in result["cold_scope"]
    assert [phase["phase"] for phase in result["phases"]] == ["cold", "warm", "edit"]
    assert [phase["returncode"] for phase in result["phases"]] == [0, 0, 0]
    edit_source = Path(result["phases"][2]["source"])
    assert module.PYTHON_TOUCH_MARKER in edit_source.read_text(encoding="utf-8")
    python = module._molt_build_python_executable()
    for phase in result["phases"]:
        command = phase["command"]
        assert command[:4] == [python, "-m", "molt.cli", "build"]
        assert "--cache-report" in command
        assert "--diagnostics-file" in command


@pytest.mark.parametrize("proof_queue", [False, True])
def test_main_molt_build_default_output_root_records_real_diagnostics_paths(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    proof_queue: bool,
) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    target = tmp_path / "target"
    out = tmp_path / "proof" / "row.json"
    calls: list[dict[str, object]] = []
    monkeypatch.setenv("MOLT_MEMORY_GUARD_ACTIVE", "1")
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
            "--molt-source",
            str(source),
            "--json-out",
            str(Path("proof") / "row.json"),
        ],
    )

    assert module.main() == 0

    payload = json.loads(out.read_text(encoding="utf-8"))
    expected_root = (tmp_path / "proof" / "row.molt-builds").resolve()
    assert routes == ["per-phase"] + ["outer" if proof_queue else "per-phase"] * 3
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
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    original = b"print(42)\n"
    source.write_bytes(original)
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
    assert payload["provenance"]["profile"] == "dev"
    result = payload["results"]["molt-build-native"]
    assert result["workflow"] == "source-checkout-diagnostic"
    assert result["driver"]["installed_distribution_verified"] is False
    assert observed[0] == observed[1] == observed[2]
    assert observed[3] == observed[4] == observed[5] != observed[0]
    for index, phase in enumerate(result["phases"]):
        witness = Path(phase["source_identity"]["witness"])
        content = witness.read_bytes()
        expected = (
            original
            if index % 3 != 2
            else original + b"\n__molt_dx_build_timer_edit__ = 1\n"
        )
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
    module = _load_dx_build_timer()
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


@pytest.mark.parametrize("mutation_point", ["warm_snapshot", "edit_append"])
def test_edit_rejects_source_mutation_without_adopting_it_as_expected(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    mutation_point: str,
) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    original = b"print(42)\n"
    source.write_bytes(original)
    out = tmp_path / "result.json"
    calls, appended = [], []
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

    def inject_at_append(path):
        appended.append(path)
        marker = apply_edit(path)
        # Mutation after the write but before its caller reads the result.
        if mutation_point == "edit_append":
            path.write_bytes(b"print(99)\n" + path.read_bytes())
        return marker

    _mock_build_execution(monkeypatch, module, run)
    monkeypatch.setattr(module, "_drain_current_session_backend_daemons", lambda _: 0)
    monkeypatch.setattr(module, "_write_snapshot", inject_at_snapshot)
    monkeypatch.setattr(module, "_apply_python_edit", inject_at_append)
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
            "--json-out",
            str(out),
        ],
    )
    assert module.main() == 1
    assert len(calls) == 2
    assert len(appended) == (0 if mutation_point == "warm_snapshot" else 1)
    result = json.loads(out.read_text(encoding="utf-8"))["results"]["molt-build-native"]
    assert (
        "between build phases" in result["error"]
        if mutation_point == "warm_snapshot"
        else "intended edit" in result["error"]
    )
    assert [phase["phase"] for phase in result["phases"]] == ["cold", "warm"]
    for phase in result["phases"]:
        assert Path(phase["source_identity"]["witness"]).read_bytes() == original
        assert (
            phase["source_identity"]["sha256"] == hashlib.sha256(original).hexdigest()
        )
        assert Path(phase["stdout_file"]).is_file()
    assert source.read_bytes() == original
