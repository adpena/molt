"""Cargo's parallel job budget for runtime builds on this host."""

from __future__ import annotations

import pytest

from molt import host_capacity
from molt.cli import cargo_execution as CARGO_EXEC


@pytest.mark.usefixtures("no_ambient_guard_caps")
def test_memory_bounded_cargo_jobs_fits_small_box(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (8 * 1024**3, 8 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 16)
    jobs = CARGO_EXEC._memory_bounded_cargo_jobs()
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
    assert CARGO_EXEC._memory_bounded_cargo_jobs() == 4


def test_cargo_build_env_defaults_jobs_but_respects_override(
    monkeypatch: pytest.MonkeyPatch, developer_host_context: None
) -> None:
    # developer_host_context: no run context, so _cargo_build_env does not
    # materialize the real checkout's roots (a plain clone's toolchain root
    # lies inside the checkout).
    monkeypatch.setattr(CARGO_EXEC, "_memory_bounded_cargo_jobs", lambda: 3)
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    env = CARGO_EXEC._cargo_build_env()
    assert env["CARGO_BUILD_JOBS"] == "3"

    monkeypatch.setenv("CARGO_BUILD_JOBS", "7")
    env2 = CARGO_EXEC._cargo_build_env()
    assert env2["CARGO_BUILD_JOBS"] == "7"
