"""Cargo's parallel job budget for runtime builds on this host."""

from __future__ import annotations

from pathlib import Path

import pytest

from molt import host_capacity
from molt.cargo_execution_policy import (
    CARGO_MEMORY_SLICE_JOBS_POLICY,
    cargo_subprocess_environment,
    memory_bounded_cargo_jobs,
)
from molt.cli import cargo_execution as CARGO_EXEC


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
