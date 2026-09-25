from __future__ import annotations

import json
import subprocess

import pytest

from molt import source_extension_link_inputs as link_inputs_contract
from molt.cli import source_extension_link_inputs as link_inputs_resolver
from molt.cli import wasm_link_inputs
from tools.proof_queue_pkg import execution_environment, toolchain_capture


def test_bound_archive_is_consumed_without_rediscovery(tmp_path, monkeypatch):
    archive = tmp_path / "libcompiler_builtins-fixture.rlib"
    archive.write_bytes(b"!<arch>\nselected")
    selected = {"PATH": str(tmp_path)}

    def discover(target, *, environment):
        assert target == "wasm32-wasip1" and environment == selected
        return archive

    monkeypatch.setattr(wasm_link_inputs, "wasm_compiler_builtins_archive", discover)
    captured = link_inputs_resolver.resolve_source_extension_link_inputs(
        "wasm32-wasip1", environment=selected
    )
    payload = captured.metadata()
    bound = {link_inputs_contract.SOURCE_EXTENSION_LINK_INPUTS_ENV: json.dumps(payload)}
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda *a, **kw: pytest.fail("bound input rediscovered"),
    )
    assert (
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1", environment=bound
        )
        == captured
    )
    frozen = toolchain_capture.frozen_files(
        {"source-extension": {"link_inputs": payload}}
    )
    assert [(row.path, row.sha256, row.size) for row in frozen] == [
        (str(archive), captured.sha256, captured.size)
    ]
    assert execution_environment._broad_toolchain_roots(
        {"source-extension": {"link_inputs": payload}}
    ) == [tmp_path]
    archive.write_bytes(b"!<arch>\nmutated!")
    with pytest.raises(ValueError, match="content changed"):
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1", environment=bound
        )


@pytest.mark.parametrize(
    "target",
    ["x86_64-pc-windows-msvc", "aarch64-apple-darwin", "wasm32-unknown-unknown"],
)
def test_non_wasi_does_not_discover_rust_archive(target, monkeypatch):
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda *a, **kw: pytest.fail("unexpected Rust input"),
    )
    assert (
        link_inputs_resolver.resolve_source_extension_link_inputs(
            target, environment={}
        ).compiler_builtins
        is None
    )


@pytest.mark.parametrize(
    "raw", ["", "null", "{}", '{"schema":"a","schema":"b"}', '{"schema":NaN}']
)
def test_malformed_bound_input_fails_without_discovery(raw, monkeypatch):
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda *a, **kw: pytest.fail("invalid binding fell back"),
    )
    with pytest.raises(ValueError):
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1",
            environment={link_inputs_contract.SOURCE_EXTENSION_LINK_INPUTS_ENV: raw},
        )


def test_bound_environment_is_published_only_by_owner():
    name = link_inputs_contract.SOURCE_EXTENSION_LINK_INPUTS_ENV
    assert execution_environment.environment_override_policy_error({name: "{}"})
    filtered, contract = execution_environment._deterministic_execution_environment(
        {name: "{}"}, override_names=[]
    )
    assert name not in filtered and name in contract["omitted_names"]


def test_rust_archive_selection_is_unambiguous(tmp_path):
    assert (
        wasm_link_inputs.wasm_compiler_builtins_archive(target_libdir=tmp_path) is None
    )
    archive = tmp_path / "libcompiler_builtins-first.rlib"
    archive.write_bytes(b"first")
    assert (
        wasm_link_inputs.wasm_compiler_builtins_archive(target_libdir=tmp_path)
        == archive
    )
    (tmp_path / "libcompiler_builtins.rlib").write_bytes(b"second")
    with pytest.raises(ValueError, match="ambiguous"):
        wasm_link_inputs.wasm_compiler_builtins_archive(target_libdir=tmp_path)


def test_rust_target_libdir_cache_owns_environment_and_compiler_generation(
    tmp_path, monkeypatch
):
    rustc = tmp_path / "rustc"
    rustc.write_bytes(b"compiler1")
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    source = tmp_path / "compiler-source"
    source.mkdir()
    guest = tmp_path / "guest"
    guest.mkdir()
    monkeypatch.chdir(guest)
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
    calls = []
    monkeypatch.setattr(wasm_link_inputs, "find_executable", lambda *a, **kw: rustc)
    monkeypatch.setattr(
        wasm_link_inputs, "resolve_rustup_proxy", lambda path, **kw: path
    )

    def query(argv, **kwargs):
        calls.append(kwargs["env"])
        assert kwargs["cwd"] == source
        return subprocess.CompletedProcess(
            argv, 0, kwargs["env"]["SELECTED_LIBDIR"] + "\n", ""
        )

    monkeypatch.setattr(wasm_link_inputs, "_run_completed_command", query)
    wasm_link_inputs.clear_rust_target_libdir_cache()
    selected = {"SELECTED_LIBDIR": str(first)}
    assert (
        wasm_link_inputs.rust_target_libdir("wasm32-wasip1", environment=selected)
        == first
    )
    assert (
        wasm_link_inputs.rust_target_libdir("wasm32-wasip1", environment=selected)
        == first
    )
    assert len(calls) == 1
    changed = {"SELECTED_LIBDIR": str(second)}
    assert (
        wasm_link_inputs.rust_target_libdir("wasm32-wasip1", environment=changed)
        == second
    )
    rustc.write_bytes(b"compiler-generation-two")
    assert (
        wasm_link_inputs.rust_target_libdir("wasm32-wasip1", environment=changed)
        == second
    )
    assert calls == [selected, changed, changed]
