"""Cargo's parallel job budget for runtime builds on this host."""

from __future__ import annotations

import os
from pathlib import Path
import sys
import time
from types import SimpleNamespace

import pytest

from molt import host_capacity
from molt.cargo_execution_policy import (
    CARGO_MEMORY_SLICE_JOBS_POLICY,
    cargo_subprocess_environment,
    memory_bounded_cargo_jobs,
)
from molt.cli import cargo_execution as CARGO_EXEC
from molt.process_environment import ProcessEnvironment
from molt.process_model import pid_alive


@pytest.mark.usefixtures("no_ambient_guard_caps")
def test_memory_bounded_cargo_jobs_fits_small_box(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (8 * 1024**3, 8 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 16)
    jobs = memory_bounded_cargo_jobs()
    assert jobs is not None
    assert jobs < 16
    assert jobs == 3


@pytest.mark.usefixtures("no_ambient_guard_caps")
def test_memory_bounded_cargo_jobs_capped_by_cpu(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (128 * 1024**3, 128 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 4)
    assert memory_bounded_cargo_jobs() == 4


def test_cargo_build_env_defaults_jobs_but_respects_override(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, developer_host_context: None
) -> None:
    from tests.sccache_test_support import isolate_sccache_environment

    isolate_sccache_environment(monkeypatch, tmp_path)
    monkeypatch.setattr(CARGO_EXEC, "memory_bounded_cargo_jobs", lambda _env: 3)
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    env = CARGO_EXEC._cargo_build_env()
    assert env["CARGO_BUILD_JOBS"] == "3"

    monkeypatch.setenv("CARGO_BUILD_JOBS", "7")
    env2 = CARGO_EXEC._cargo_build_env()
    assert env2["CARGO_BUILD_JOBS"] == "7"


def test_direct_cargo_compiles_fit_the_enclosing_tree_allocation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gib = 1024**3
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (128 * gib, 36 * gib)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 24)
    clippy = ["cargo", "clippy", "--workspace", "--all-targets"]
    delegated = {host_capacity.MEMORY_SLICE_ENV: str(int(5.78 * gib))}

    for settings in ({}, {"CARGO_BUILD_JOBS": "12"}, {"CARGO_BUILD_JOBS": "default"}):
        env, policies = cargo_subprocess_environment(
            clippy, {**delegated, **settings}, cwd=None
        )
        assert env is not None
        assert env["CARGO_BUILD_JOBS"] == "1"
        assert (
            env[host_capacity.MEMORY_SLICE_ENV]
            == delegated[host_capacity.MEMORY_SLICE_ENV]
        )
        assert policies == (CARGO_MEMORY_SLICE_JOBS_POLICY,)

    wider = {
        host_capacity.MEMORY_SLICE_ENV: str(10 * gib),
        "CARGO_BUILD_JOBS": "2",
    }
    for command, settings in (
        (clippy, {}),
        (clippy, {"CARGO_BUILD_JOBS": "12"}),
        (clippy, wider),
        (["cargo", "metadata"], delegated),
    ):
        env, policies = cargo_subprocess_environment(command, settings, cwd=None)
        assert env is not None
        assert dict(env) == settings
        assert policies == ()


def _cargo_plan(
    root: Path, profile: str, target: str, *command: str
) -> SimpleNamespace:
    return SimpleNamespace(
        project_root=root,
        target=target,
        host_target="aarch64-apple-darwin",
        cargo_profile=profile,
        cli_configuration={},
        command=command or ("cargo", "build"),
        environment=ProcessEnvironment(os.environ).updated(
            {
                "CARGO_TARGET_DIR": str(root / "artifacts"),
                "CARGO_BUILD_BUILD_DIR": str(root / "intermediates"),
                "CARGO_BUILD_JOBS": "2",
                "CARGO_MAKEFLAGS": "",
                "MAKEFLAGS": "",
                "MFLAGS": "",
            }
        ),
        verify=lambda: None,
    )


def test_wasm_preparation_checks_host_profile_locks_and_joint_memory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, no_ambient_guard_caps: None
) -> None:
    gib = 1024**3
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (128 * gib, 64 * gib)
    )
    monkeypatch.setenv("MOLT_MAX_CONCURRENT_BUILDS", "2")
    compiler = _cargo_plan(tmp_path, "release-fast", "aarch64-apple-darwin")
    runtime = _cargo_plan(tmp_path, "wasm-release", "wasm32-wasip1")
    assert CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)

    runtime.cargo_profile = "release-fast"
    assert not CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)
    runtime.cargo_profile = "wasm-release"
    intermediates = tmp_path / "intermediates"
    (intermediates / "release-fast").mkdir(parents=True)
    try:
        (intermediates / "wasm-release").symlink_to(
            intermediates / "release-fast", target_is_directory=True
        )
    except OSError as exc:
        pytest.skip(f"cannot create profile directory symlink: {exc}")
    assert not CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)
    (intermediates / "wasm-release").unlink()

    explicit_host = _cargo_plan(
        tmp_path,
        "dev-fast",
        compiler.host_target,
        "cargo",
        "build",
        "--target",
        compiler.host_target,
    )
    target_profile = intermediates / compiler.host_target / explicit_host.cargo_profile
    target_profile.parent.mkdir()
    assert CARGO_EXEC._cargo_plans_can_overlap(compiler, explicit_host)
    target_profile.symlink_to(intermediates / "release-fast", target_is_directory=True)
    assert not CARGO_EXEC._cargo_plans_can_overlap(compiler, explicit_host)

    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(10 * gib))
    assert not CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(16 * gib))
    assert CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)
    monkeypatch.setenv("MOLT_MAX_CONCURRENT_BUILDS", "1")
    assert not CARGO_EXEC._cargo_plans_can_overlap(compiler, runtime)


@pytest.mark.usefixtures("no_ambient_guard_caps")
@pytest.mark.parametrize("mode", ["parallel", "serialized", "busy", "interrupt"])
def test_wasm_preparation_owns_real_overlapping_children(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    gib = 1024**3
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (128 * gib, 64 * gib)
    )
    monkeypatch.setenv("MOLT_MAX_CONCURRENT_BUILDS", "2")
    monkeypatch.setattr(CARGO_EXEC, "_build_slot_dir", lambda: tmp_path / "slots")
    started = tmp_path / "compiler.pid"
    finished = tmp_path / "runtime-finished"
    compiler = _cargo_plan(
        tmp_path,
        "release-fast",
        "aarch64-apple-darwin",
        sys.executable,
        "-c",
        """
import os
import sys
import time
from pathlib import Path

started, finished = map(Path, sys.argv[1:3])
marker = started.with_suffix(".staged")
marker.write_text(str(os.getpid()))
marker.replace(started)
if sys.argv[3] in {"parallel", "interrupt"}:
    while not finished.is_file():
        time.sleep(0.01)
""",
        str(started),
        str(finished),
        mode,
    )
    runtime = _cargo_plan(
        tmp_path,
        "release-fast" if mode == "serialized" else "wasm-release",
        "wasm32-wasip1",
        sys.executable,
        "-c",
        "from pathlib import Path; import sys; Path(sys.argv[1]).touch()",
        str(finished),
    )

    def prepare_runtime() -> None:
        deadline = time.monotonic() + 8
        while not started.is_file() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert started.is_file(), "compiler must start before runtime materialization"
        if mode == "interrupt":
            raise KeyboardInterrupt
        with CARGO_EXEC._build_slot() as slot:
            if mode == "parallel":
                assert slot == 1
            prepared = CARGO_EXEC._run_resolved_cargo_plan(
                runtime, timeout=8, json_output=True, label="fixture runtime"
            )
        assert prepared.returncode == 0

    def build_compiler() -> CARGO_EXEC.CargoExecutionResult:
        return CARGO_EXEC._run_resolved_cargo_plan(
            compiler,
            timeout=15,
            json_output=True,
            label="fixture compiler",
            preparation=CARGO_EXEC._CargoPreparation(runtime, prepare_runtime),
        )

    if mode == "interrupt":
        with pytest.raises(KeyboardInterrupt):
            build_compiler()
    else:
        if mode == "busy":
            with CARGO_EXEC._build_slot() as first, CARGO_EXEC._build_slot() as second:
                assert (first, second) == (0, 1)
                assert build_compiler().returncode == 0
        else:
            assert build_compiler().returncode == 0
        if mode in {"serialized", "busy"}:
            assert not finished.is_file()
            prepare_runtime()
        assert finished.is_file()
    assert not pid_alive(int(started.read_text(encoding="utf-8")))
