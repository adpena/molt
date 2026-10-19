"""Host facts admission shares build custody without joining codegen identity."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import backend_binary, cache_fingerprints, compiler_identity
from molt.cli import compiler_metadata, wasm_facts_tool
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.host_tool_products import HostToolProduct
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.process_environment import ProcessEnvironment
from tests.compiler_identity_helper import (
    compiler_cargo_stdout,
    stub_compiler_admission,
)


def _workspace(root: Path) -> None:
    files = {
        "Cargo.toml": '[workspace]\nmembers=["runtime/molt-backend","runtime/molt-wasm-facts","runtime/shared"]\n',
        "Cargo.lock": 'version=4\n[[package]]\nname="molt-backend"\nversion="0.1.0"\ndependencies=["shared"]\n[[package]]\nname="molt-wasm-facts"\nversion="0.1.0"\ndependencies=["shared"]\n[[package]]\nname="shared"\nversion="0.1.0"\n',
        "runtime/molt-backend/Cargo.toml": '[package]\nname="molt-backend"\nversion="0.1.0"\n[features]\nnative-backend=[]\n[dependencies]\nshared={path="../shared"}\n',
        "runtime/molt-wasm-facts/Cargo.toml": '[package]\nname="molt-wasm-facts"\nversion="0.1.0"\n[features]\ndefault=[]\nhost-tool=[]\n[dependencies]\nshared={path="../shared"}\n',
        "runtime/shared/Cargo.toml": '[package]\nname="shared"\nversion="0.1.0"\n',
        "runtime/molt-backend/src/main.rs": "fn main() { /* compiler */ }\n",
        "runtime/molt-wasm-facts/src/main.rs": "fn main() { /* facts */ }\n",
        "runtime/shared/src/lib.rs": "pub fn shared() {}\n",
        "src/molt/backend_environment.json": "{}\n",
        "src/molt/backend_daemon_protocol_version.txt": "2\n",
    }
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


@pytest.fixture
def source_tool(tmp_path, monkeypatch):
    """Keep closure, hashes, dep-info, receipts and immutable publication real."""
    _workspace(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setattr(
        backend_binary, "_compiler_clean_source_state", lambda root: None
    )
    monkeypatch.setattr(
        compiler_metadata, "_compiler_clean_source_state", lambda root: None
    )
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    state = SimpleNamespace(
        builds=[],
        probes=[],
        probe_error=None,
        probe_timeout=False,
        expected_probe_value="observed-value",
    )
    environment = {
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "MOLT_CARGO_TIMEOUT": "7",
        "MOLT_TEST_BACKEND_INPUT": "observed-value",
    }

    def build(plan, **kwargs):
        command = plan.command
        name = command[command.index("--package") + 1]
        product = HostToolProduct(name)
        assert command[command.index("--bin") + 1] == name
        if product is HostToolProduct.WASM_FACTS:
            assert command[command.index("--features") + 1] == "host-tool"
        assert kwargs["timeout"] == 7
        state.builds.append(product)
        output = (
            tmp_path / "target/dev-fast" / (name + (".exe" if os.name == "nt" else ""))
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(
            (tmp_path / "runtime" / name / "src/main.rs").read_bytes()
            + (tmp_path / "runtime/shared/src/lib.rs").read_bytes()
        )
        output.chmod(0o755)
        return subprocess.CompletedProcess(
            command,
            0,
            compiler_cargo_stdout(tmp_path, output, plan.environment, product=product),
            "",
        )

    def probe(command, *, input, env, **kwargs):
        assert kwargs["timeout"] == 7
        assert env["MOLT_TEST_BACKEND_INPUT"] == state.expected_probe_value
        if command[1] == "--scan-wasm-link-facts":
            assert input is None
            assert Path(command[2]).read_bytes() == b"\0asm\x01\0\0\0"
            assert Path(command[0]).name.startswith("molt-wasm-facts.host_tool")
        else:
            assert command[1] == "--output"
            assert json.loads(input)["metadata"]["target"] == "native"
        state.probes.append(tuple(command))
        if state.probe_timeout:
            raise subprocess.TimeoutExpired(
                command,
                kwargs["timeout"],
                output=b"partial facts stdout",
                stderr=b"partial facts stderr",
            )
        return subprocess.CompletedProcess(
            command,
            1 if state.probe_error else 0,
            b"{}",
            (state.probe_error or "").encode(),
        )

    monkeypatch.setattr(backend_binary, "_run_resolved_cargo_plan", build)
    monkeypatch.setattr(backend_binary, "_run_subprocess_captured_to_tempfiles", probe)
    return tmp_path, environment, state


def _backend(root, environment):
    suffix = ".exe" if os.name == "nt" else ""
    return backend_binary._ensure_backend_binary(
        root / "target/dev-fast" / f"molt-backend.native_backend{suffix}",
        cargo_timeout=7,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=root,
        backend_features=("native-backend",),
        rust_cpu=RustCpuPolicy.BASELINE,
        environment=environment,
    )


def test_facts_only_edit_rebuilds_its_generation_without_renaming_compiler(source_tool):
    root, environment, state = source_tool
    first = wasm_facts_tool.ensure_wasm_facts_tool(
        root, cargo_profile="dev-fast", env=environment
    )
    compiler = _backend(root, environment)
    assert compiler, compiler.message
    original = first.read_bytes()
    assert state.builds == [HostToolProduct.WASM_FACTS, HostToolProduct.BACKEND]
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
        == first
    )
    assert _backend(root, environment).binary == compiler.binary
    assert len(state.builds) == len(state.probes) == 2

    (root / "runtime/molt-wasm-facts/src/main.rs").write_text(
        "fn main() { /* changed facts */ }\n", encoding="utf-8"
    )
    changed = wasm_facts_tool.ensure_wasm_facts_tool(
        root, cargo_profile="dev-fast", env=environment
    )
    unchanged = _backend(root, environment)
    assert changed != first and changed.read_bytes() != original
    assert first.read_bytes() == original
    assert unchanged.binary == compiler.binary
    assert unchanged.cache_compiler_fingerprint == compiler.cache_compiler_fingerprint
    assert state.builds == [
        HostToolProduct.WASM_FACTS,
        HostToolProduct.BACKEND,
        HostToolProduct.WASM_FACTS,
    ]

    (root / "runtime/molt-backend/src/main.rs").write_text(
        "fn main() { /* changed compiler */ }\n", encoding="utf-8"
    )
    rebuilt = _backend(root, environment)
    assert rebuilt and rebuilt.binary != compiler.binary
    assert rebuilt.cache_compiler_fingerprint != compiler.cache_compiler_fingerprint
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
        == changed
    )
    assert state.builds[-1] is HostToolProduct.BACKEND

    # A genuine shared dependency remains in both selected source closures.
    (root / "runtime/shared/src/lib.rs").write_text(
        "pub fn changed_shared() {}\n", encoding="utf-8"
    )
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
        != changed
    )
    assert _backend(root, environment).binary != rebuilt.binary


def test_facts_source_rebuild_policy_uses_explicit_environment(
    source_tool, monkeypatch
):
    root, environment, state = source_tool
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    wasm_facts_tool.ensure_wasm_facts_tool(
        root, cargo_profile="dev-fast", env=environment
    )
    assert len(state.builds) == 1
    (root / "runtime/molt-wasm-facts/src/main.rs").write_text(
        "fn main() { /* new */ }\n", encoding="utf-8"
    )
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD")
    with pytest.raises(ValueError, match="MOLT_SKIP_RUNTIME_REBUILD=1"):
        wasm_facts_tool.ensure_wasm_facts_tool(
            root,
            cargo_profile="dev-fast",
            env={**environment, "MOLT_SKIP_RUNTIME_REBUILD": "1"},
        )
    assert len(state.builds) == 1


def test_facts_observed_receipt_and_probe_use_explicit_environment(
    source_tool, monkeypatch
):
    root, environment, state = source_tool
    monkeypatch.setenv("MOLT_TEST_BACKEND_INPUT", "ambient-poison")
    monkeypatch.setenv("MOLT_CARGO_TIMEOUT", "invalid-ambient-timeout")
    first = wasm_facts_tool.ensure_wasm_facts_tool(
        root, cargo_profile="dev-fast", env=environment
    )
    monkeypatch.setenv("MOLT_TEST_BACKEND_INPUT", "other-ambient-poison")
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
        == first
    )
    assert len(state.builds) == 1
    coordinate = (
        root
        / "target/dev-fast"
        / ("molt-wasm-facts.host_tool" + (".exe" if os.name == "nt" else ""))
    )
    receipt_path = backend_binary._backend_fingerprint_path(
        root, coordinate, "dev-fast"
    )
    initial = backend_binary._read_runtime_fingerprint(receipt_path)
    observation = CargoEnvironmentInputs.from_dict(initial["cargo_environment"])
    observation.verify(ProcessEnvironment(environment))
    state.expected_probe_value = "changed-explicit-input"
    changed = {**environment, "MOLT_TEST_BACKEND_INPUT": state.expected_probe_value}
    with pytest.raises(ValueError, match="MOLT_TEST_BACKEND_INPUT"):
        observation.verify(ProcessEnvironment(changed))
    # Source/output bytes stay fixed; only the observed ambient input changes.
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=changed
        )
        == first
    )
    assert len(state.builds) == 2
    current = backend_binary._read_runtime_fingerprint(receipt_path)
    CargoEnvironmentInputs.from_dict(current["cargo_environment"]).verify(
        ProcessEnvironment(changed)
    )


def test_failed_facts_probe_is_terminal_and_selects_no_receipt(source_tool):
    root, environment, state = source_tool
    state.probe_error = "independent facts failure"
    with pytest.raises(ValueError, match="independent facts failure"):
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
    assert state.builds == [HostToolProduct.WASM_FACTS]
    assert len(state.probes) == 1
    coordinate = (
        root
        / "target/dev-fast"
        / ("molt-wasm-facts.host_tool" + (".exe" if os.name == "nt" else ""))
    )
    assert not backend_binary._backend_fingerprint_path(
        root, coordinate, "dev-fast"
    ).exists()


def test_facts_probe_timeout_retains_diagnostics_and_releases_probe(source_tool):
    root, environment, state = source_tool
    state.probe_timeout = True
    with pytest.raises(ValueError) as rejected:
        wasm_facts_tool.ensure_wasm_facts_tool(
            root, cargo_profile="dev-fast", env=environment
        )
    assert "WASM facts tool capability probe did not finish within 7.0s" in str(
        rejected.value
    )
    assert "partial facts stdout" in str(rejected.value)
    assert "partial facts stderr" in str(rejected.value)
    assert len(state.builds) == len(state.probes) == 1
    assert not Path(state.probes[0][2]).exists()


@pytest.mark.parametrize("timeout", ["invalid", "0", "-2"])
def test_facts_timeout_diagnostic_precedes_build(source_tool, timeout):
    root, environment, state = source_tool
    with pytest.raises(ValueError, match="MOLT_CARGO_TIMEOUT"):
        wasm_facts_tool.ensure_wasm_facts_tool(
            root,
            cargo_profile="dev-fast",
            env={**environment, "MOLT_CARGO_TIMEOUT": timeout},
        )
    assert state.builds == state.probes == []


def test_installed_facts_is_verified_without_cargo_or_path(tmp_path, monkeypatch):
    from molt import compiler_distribution

    binary = tmp_path / "installed-facts"
    binary.write_bytes(b"installed facts")
    expected = hashlib.sha256(binary.read_bytes()).hexdigest()
    verified = []

    def verify_facts():
        verified.append(True)
        if hashlib.sha256(binary.read_bytes()).hexdigest() != expected:
            raise ValueError("installed facts content changed")

    installed = SimpleNamespace(
        source_sha="a" * 40,
        files=[],
        record={"features": ["native-backend"], "profile": "release"},
        launcher={},
        runtime={},
        wasm_facts={"sha256": expected},
        binary=tmp_path / "installed-compiler",
        wasm_facts_binary=binary,
        verify_sources=lambda: None,
        verify_executing_package=lambda: None,
        verify_binary=lambda features, profile: None,
        verify_wasm_facts=verify_facts,
    )
    monkeypatch.setattr(
        compiler_distribution, "installed_compiler", lambda root: installed
    )
    monkeypatch.setattr(wasm_facts_tool, "installed_compiler", lambda root: installed)
    monkeypatch.setattr(
        backend_binary,
        "backend_build_admission",
        lambda *a, **k: pytest.fail("installed mode invoked Cargo admission"),
    )
    monkeypatch.setattr(
        wasm_facts_tool,
        "project_cargo_target_dir",
        lambda *a, **k: pytest.fail("installed mode chose a source target"),
    )
    assert (
        wasm_facts_tool.ensure_wasm_facts_tool(
            tmp_path, cargo_profile="dev-fast", env={"PATH": ""}
        )
        == binary
    )
    assert verified == [True]
    before = compiler_identity.installed_compiler_admission(tmp_path).fingerprint
    installed.wasm_facts = {"sha256": "b" * 64}
    assert (
        compiler_identity.installed_compiler_admission(tmp_path).fingerprint != before
    )
    binary.write_bytes(b"changed facts")
    with pytest.raises(ValueError, match="installed facts content changed"):
        wasm_facts_tool.ensure_wasm_facts_tool(
            tmp_path, cargo_profile="dev-fast", env={"PATH": ""}
        )


def test_actual_cargo_edges_separate_tool_from_codegen():
    root = Path(__file__).resolve().parents[2]
    backend = cache_fingerprints._backend_source_paths(root, ("wasm-backend",))
    facts = cache_fingerprints._backend_source_paths(
        root, ("host-tool",), product=HostToolProduct.WASM_FACTS
    )
    assert root / "runtime/molt-wasm-facts" not in backend
    assert root / "runtime/molt-wasm-facts" in facts
    assert root / "runtime/molt-artifact-publish" in facts
    assert root / "runtime/molt-host-process" in facts
    for crate in (
        "molt-backend",
        "molt-backend-native",
        "molt-backend-wasm",
        "molt-ir",
        "molt-tir",
        "molt-passes",
        "molt-runtime-resource",
    ):
        assert root / "runtime" / crate not in facts
