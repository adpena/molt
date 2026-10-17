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
        lambda *args, **kwargs: pytest.fail(
            "explicit Node must not consult pinned custody"
        ),
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
        lambda *args, **kwargs: pytest.fail("invalid explicit Node must not fall back"),
    )
    with pytest.raises(node_runtime.NodeRuntimeError, match="MOLT_NODE_BIN is invalid"):
        node_runtime.resolve_node_runtime(
            source_root=tmp_path, environment={"MOLT_NODE_BIN": "missing-node"}
        )


def test_pinned_node_precedes_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    pinned = tmp_path / "pinned-node"
    monkeypatch.setattr(
        node_runtime, "pinned_executable", lambda *args, **kwargs: pinned
    )
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
    def invalid(*args: object, **kwargs: object) -> Path:
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
    monkeypatch.setattr(node_runtime, "pinned_executable", lambda *args, **kwargs: None)
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
    monkeypatch.setattr(
        node_runtime, "pinned_executable", lambda *args, **kwargs: pinned
    )

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
        node_runtime, "pinned_executable", lambda *args, **kwargs: tmp_path / "node"
    )
    _probe(monkeypatch, "16.20.0")
    with pytest.raises(node_runtime.NodeRuntimeError, match="Node >= 18 is required"):
        node_runtime.resolve_node_runtime(source_root=tmp_path, environment={})


def test_pinned_node_reads_child_tool_state_not_ambient(tmp_path, monkeypatch):
    from tests.test_tool_releases import _installed_demo
    from molt import tool_releases

    discovery = _installed_demo(tmp_path, monkeypatch)
    monkeypatch.setattr(
        tool_releases, "load_tool_releases", lambda _root: {"node": discovery.release}
    )
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "ambient"))
    _probe(monkeypatch)
    selected = {"MOLT_TARGET_ROOT": str(tmp_path / "target-root")}
    actual = node_runtime.resolve_node_runtime(
        source_root=tmp_path, environment=selected
    )
    assert actual.path == discovery.executable
    assert not (tmp_path / "ambient").exists()


def test_benchmark_admission_hashes_and_probes_once(monkeypatch, tmp_path):
    import hashlib

    image = tmp_path / "selected-node"
    image.write_bytes(b"independent execution image")
    monkeypatch.setattr(node_runtime, "resolve_executable", lambda *a, **kw: image)
    probes = _probe(monkeypatch)
    plan = node_runtime.admit_node_execution(
        source_root=tmp_path, environment={"MOLT_NODE_BIN": str(image)}
    )
    for _ in range(5):
        plan.verify()
    assert probes == [(str(image), "-p", "process.versions.node")]
    assert plan.as_record() == {
        "kind": "node",
        "executable": str(image),
        "version": "24.1.0",
        "identity": {
            "entrypoint": image.name,
            "content_filename": image.name,
            "size": len(b"independent execution image"),
            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
        },
    }


def test_ordinary_resolver_does_no_benchmark_identity_work(monkeypatch, tmp_path):
    image = tmp_path / "ordinary-node"
    monkeypatch.setattr(node_runtime, "resolve_executable", lambda *a, **kw: image)
    monkeypatch.setattr(
        node_runtime,
        "executable_content_identity",
        lambda *a, **kw: pytest.fail("ordinary runner must not hash Node"),
    )
    monkeypatch.setattr(
        node_runtime,
        "stable_regular_file_version",
        lambda *a, **kw: pytest.fail("ordinary runner must not fence Node"),
    )
    _probe(monkeypatch)
    assert (
        node_runtime.resolve_node_runtime(
            source_root=tmp_path, environment={"MOLT_NODE_BIN": str(image)}
        ).path
        == image
    )


@pytest.mark.parametrize("during_probe", [False, True])
def test_admission_rejects_transient_image_replacement(
    monkeypatch, tmp_path, during_probe
):
    image = tmp_path / "node"
    original = b"original image"
    image.write_bytes(original)
    monkeypatch.setattr(node_runtime, "resolve_executable", lambda *a, **kw: image)

    def replace_and_restore():
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"differentimage")
        replacement.replace(image)
        image.write_bytes(original)

    def probe(command, **kwargs):
        if during_probe:
            replace_and_restore()
        return subprocess.CompletedProcess(command, 0, "24.1.0\n", "")

    monkeypatch.setattr(node_runtime.process_guard, "run_completed_command", probe)
    if during_probe:
        with pytest.raises(node_runtime.NodeRuntimeError, match="changed"):
            node_runtime.admit_node_execution(
                source_root=tmp_path, environment={"MOLT_NODE_BIN": str(image)}
            )
    else:
        plan = node_runtime.admit_node_execution(
            source_root=tmp_path, environment={"MOLT_NODE_BIN": str(image)}
        )
        replace_and_restore()
        with pytest.raises(node_runtime.NodeRuntimeError, match="changed"):
            plan.verify()


def test_admitted_path_does_not_follow_retargeted_selection(monkeypatch, tmp_path):
    first, second, selected = (tmp_path / name for name in ("first", "second", "node"))
    first.write_bytes(b"first image")
    second.write_bytes(b"second image")
    try:
        selected.symlink_to(first)
    except OSError:
        pytest.skip("host cannot create an executable-selection symlink")
    monkeypatch.setattr(node_runtime, "resolve_executable", lambda *a, **kw: selected)
    probes = _probe(monkeypatch)
    plan = node_runtime.admit_node_execution(
        source_root=tmp_path, environment={"MOLT_NODE_BIN": str(selected)}
    )
    selected.unlink()
    selected.symlink_to(second)
    plan.verify()
    assert plan.runtime.path == first
    assert probes == [(str(first), "-p", "process.versions.node")]
