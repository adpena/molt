from __future__ import annotations

from pathlib import Path
import subprocess

import pytest

from molt import node_runtime


def _probe(
    monkeypatch: pytest.MonkeyPatch, version: str = "24.1.0"
) -> list[tuple[str, ...]]:
    commands: list[tuple[str, ...]] = []

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        assert kwargs["capture_output"] is True
        assert kwargs["timeout"] == 10
        commands.append(tuple(command))
        return subprocess.CompletedProcess(command, 0, version + "\n", "")

    monkeypatch.setattr(node_runtime.process_guard, "run_completed_command", run)
    return commands


def test_explicit_node_precedes_pinned_and_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    explicit = tmp_path / "explicit-node"
    resolved: list[str] = []
    monkeypatch.setattr(
        node_runtime,
        "resolve_executable",
        lambda command, **kwargs: resolved.append(command) or explicit,
    )
    monkeypatch.setattr(
        node_runtime,
        "pinned_executable",
        lambda *args: pytest.fail("explicit Node must not consult pinned custody"),
    )
    commands = _probe(monkeypatch)

    runtime = node_runtime.resolve_node_runtime(
        source_root=tmp_path, environment={"MOLT_NODE_BIN": str(explicit)}
    )

    assert runtime == node_runtime.NodeRuntime(explicit, "24.1.0", 24)
    assert resolved == [str(explicit)]
    assert commands == [(str(explicit), "-p", "process.versions.node")]


def test_invalid_explicit_node_never_falls_back(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    def invalid(*args: object, **kwargs: object) -> Path:
        raise ValueError("missing executable")

    monkeypatch.setattr(node_runtime, "resolve_executable", invalid)
    monkeypatch.setattr(
        node_runtime,
        "pinned_executable",
        lambda *args: pytest.fail("invalid explicit Node must not fall back"),
    )
    with pytest.raises(node_runtime.NodeRuntimeError, match="MOLT_NODE_BIN is invalid"):
        node_runtime.resolve_node_runtime(
            source_root=tmp_path, environment={"MOLT_NODE_BIN": "missing-node"}
        )


def test_pinned_node_precedes_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    pinned = tmp_path / "pinned-node"
    monkeypatch.setattr(node_runtime, "pinned_executable", lambda *args: pinned)
    monkeypatch.setattr(
        node_runtime,
        "resolve_executable",
        lambda *args, **kwargs: pytest.fail("pinned Node must not search PATH"),
    )
    _probe(monkeypatch)

    assert (
        node_runtime.resolve_node_runtime(source_root=tmp_path, environment={}).path
        == pinned
    )


def test_invalid_pinned_custody_is_typed_and_does_not_fall_back(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    def invalid(*args: object) -> Path:
        raise RuntimeError("release attestation drift")

    monkeypatch.setattr(node_runtime, "pinned_executable", invalid)
    monkeypatch.setattr(
        node_runtime,
        "resolve_executable",
        lambda *args, **kwargs: pytest.fail("invalid pinned Node must not search PATH"),
    )
    with pytest.raises(
        node_runtime.NodeRuntimeError, match="release attestation drift"
    ):
        node_runtime.resolve_node_runtime(source_root=tmp_path, environment={})


def test_path_selection_uses_current_environment_without_cache(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(node_runtime, "pinned_executable", lambda *args: None)
    selections: list[str] = []

    def resolve(command: str, *, environment: dict[str, str], label: str) -> Path:
        assert command == "node" and label == "Node host"
        selections.append(environment["PATH"])
        return Path(environment["PATH"]) / "node"

    monkeypatch.setattr(node_runtime, "resolve_executable", resolve)
    _probe(monkeypatch)

    first = node_runtime.resolve_node_runtime(
        source_root=tmp_path, environment={"PATH": str(tmp_path / "first")}
    )
    second = node_runtime.resolve_node_runtime(
        source_root=tmp_path, environment={"PATH": str(tmp_path / "second")}
    )

    assert first.path != second.path
    assert selections == [str(tmp_path / "first"), str(tmp_path / "second")]


def test_node_probe_timeout_is_a_useful_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    pinned = tmp_path / "pinned-node"
    monkeypatch.setattr(node_runtime, "pinned_executable", lambda *args: pinned)

    def timeout(
        command: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        raise subprocess.TimeoutExpired(command, kwargs["timeout"])

    monkeypatch.setattr(node_runtime.process_guard, "run_completed_command", timeout)
    with pytest.raises(node_runtime.NodeRuntimeError, match="Node probe failed"):
        node_runtime.resolve_node_runtime(source_root=tmp_path, environment={})


def test_node_probe_enforces_minimum_major(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        node_runtime, "pinned_executable", lambda *args: tmp_path / "node"
    )
    _probe(monkeypatch, "16.20.0")
    with pytest.raises(node_runtime.NodeRuntimeError, match="Node >= 18 is required"):
        node_runtime.resolve_node_runtime(source_root=tmp_path, environment={})
