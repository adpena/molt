from __future__ import annotations

import importlib.util
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
    assert touch_files["gvn"].exists()
    assert module._scenario_preflight_errors(
        [
            "inc-value_range",
            "inc-gvn",
            "inc-function_compiler",
            "inc-modules",
            "test-lib",
        ],
        touch_files,
    ) == []


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
        "scenario=molt-build-unknown unknown molt build target; choices="
        in errors[1]
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
        "--cache-report",
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

    assert (
        module._molt_build_python_executable(
            {
                "UV_PROJECT_ENVIRONMENT": str(uv_env),
                "VIRTUAL_ENV": str(tmp_path / "other-env"),
            }
        )
        == str(python)
    )


def test_molt_build_output_root_defaults_to_json_stem_for_evidence_custody(
    tmp_path: Path,
) -> None:
    module = _load_dx_build_timer()

    assert module._molt_build_output_root(
        SimpleNamespace(
            target_dir=str(tmp_path / "target"),
            json_out=str(tmp_path / "proof" / "row.json"),
            molt_output_root=None,
        )
    ) == (tmp_path / "proof" / "row.molt-builds").resolve()

    assert module._molt_build_output_root(
        SimpleNamespace(
            target_dir=str(tmp_path / "target"),
            json_out=str(tmp_path / "proof" / "row.json"),
            molt_output_root=str(tmp_path / "explicit"),
        )
    ) == (tmp_path / "explicit").resolve()


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


def test_main_molt_build_scenario_skips_daemon_prime_and_records_phases(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    target = tmp_path / "target"
    output_root = tmp_path / "molt-builds"
    out = tmp_path / "timer.json"
    calls: list[dict[str, object]] = []
    drain_envs: list[dict[str, str]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        stdout = "cargo 1.96.1\n" if list(cmd) == ["cargo", "--version"] else ""
        return SimpleNamespace(
            returncode=0,
            stdout=stdout,
            stderr="",
            elapsed_s=float(len(calls)),
        )

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


def test_main_molt_build_default_output_root_records_real_diagnostics_paths(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = _load_dx_build_timer()
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    target = tmp_path / "target"
    out = tmp_path / "proof" / "row.json"
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        cmd = list(cmd)
        calls.append({"cmd": cmd, **kwargs})
        if cmd == ["cargo", "--version"]:
            stdout = "cargo 1.96.1\n"
        else:
            diagnostics_file = Path(cmd[cmd.index("--diagnostics-file") + 1])
            diagnostics_file.parent.mkdir(parents=True, exist_ok=True)
            diagnostics_file.write_text('{"ok": true}\n', encoding="utf-8")
            stdout = ""
        return SimpleNamespace(
            returncode=0,
            stdout=stdout,
            stderr="",
            elapsed_s=float(len(calls)),
        )

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
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
    assert payload["meta"]["molt_output_root_resolved"] == str(expected_root)
    for phase in payload["results"]["molt-build-native"]["phases"]:
        diagnostics_path = Path(phase["diagnostics_file"])
        assert diagnostics_path.is_absolute()
        assert diagnostics_path.is_file()
        assert expected_root in diagnostics_path.parents
        assert ".molt_build" not in diagnostics_path.parts
