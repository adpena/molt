from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from molt import source_extension_link_inputs as link_inputs_contract
from molt.cli import source_extension_link_inputs as link_inputs_resolver
from molt.cli import wasm_link_inputs
from tools.proof_queue_pkg import execution_environment, toolchain_capture
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    runtime_wasi_c_abi_plan,
)


def test_bound_archive_is_consumed_without_rediscovery(tmp_path, monkeypatch):
    plan = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))
    archive = plan.path("compiler_rt")
    selected = {"PATH": str(tmp_path)}

    captured = link_inputs_resolver.resolve_source_extension_link_inputs(
        "wasm32-wasip1", wasi_c_abi=plan, environment=selected
    )
    payload = captured.metadata()
    bound = {link_inputs_contract.SOURCE_EXTENSION_LINK_INPUTS_ENV: json.dumps(payload)}
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda *a, **kw: pytest.fail("bound input rediscovered"),
    )
    assert (
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1", wasi_c_abi=plan, environment=bound
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
    ) == [archive.parent]
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(
        cas, {"fixture": {"link_inputs": payload}}
    )
    archive.write_bytes(b"!<arch>\nmutated!")
    assert (
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1", wasi_c_abi=plan, environment=bound
        )
        == captured
    )
    verified = toolchain_capture.verify_capture(reference, workers=1, cas_root=cas)
    assert not verified["stable"]
    assert [row["path"] for row in verified["mismatches"]] == [str(archive)]


@pytest.mark.parametrize(
    "target",
    ["x86_64-pc-windows-msvc", "aarch64-apple-darwin", "wasm32-unknown-unknown"],
)
def test_non_wasi_does_not_discover_sdk_archive(target, monkeypatch):
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda *a, **kw: pytest.fail("unexpected SDK input"),
    )
    assert (
        link_inputs_resolver.resolve_source_extension_link_inputs(
            target, wasi_c_abi=None, environment={}
        ).compiler_rt
        is None
    )


@pytest.mark.parametrize(
    "raw", ["", "null", "{}", '{"schema":"a","schema":"b"}', '{"schema":NaN}']
)
def test_malformed_bound_input_fails_without_discovery(raw, monkeypatch):
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda *a, **kw: pytest.fail("invalid binding fell back"),
    )
    with pytest.raises(ValueError):
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1",
            wasi_c_abi=None,
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


@pytest.mark.parametrize(
    "kind", ("old-schema", "old-field", "foreign-archive", "size", "digest")
)
def test_captured_c_runtime_binding_rejects_obsolete_or_different_plan(tmp_path, kind):
    plan = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))
    captured = link_inputs_resolver.resolve_source_extension_link_inputs(
        "wasm32-wasip1",
        wasi_c_abi=plan,
        environment={},
    )
    payload = captured.metadata()
    if kind == "old-schema":
        payload["schema"] = "molt.source-extension-link-inputs.v1"
    elif kind == "old-field":
        payload["compiler_builtins"] = payload.pop("compiler_rt")
    elif kind == "foreign-archive":
        copy = tmp_path / "foreign.a"
        copy.write_bytes(plan.path("compiler_rt").read_bytes())
        payload["compiler_rt"]["path"] = str(copy)
    elif kind == "size":
        payload["compiler_rt"]["size"] += 1
    else:
        payload["compiler_rt"]["sha256"] = "0" * 64
    with pytest.raises(ValueError):
        link_inputs_resolver.resolve_source_extension_link_inputs(
            "wasm32-wasip1",
            wasi_c_abi=plan,
            environment={
                link_inputs_contract.SOURCE_EXTENSION_LINK_INPUTS_ENV: json.dumps(
                    payload
                )
            },
        )


def test_managed_link_input_projection_does_not_adopt_manual_sdk_edits(tmp_path):
    plan = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))
    plan.path("compiler_rt").write_bytes(b"!<arch>\nnew generation")
    selected = link_inputs_resolver.resolve_source_extension_link_inputs(
        "wasm32-wasip1",
        wasi_c_abi=plan,
        environment={},
    )
    assert selected.sha256 == next(
        row[3] for row in plan.files if row[0] == "compiler_rt"
    )
    from molt.llvm_toolchain import load_wasi_sdk_installation, LlvmToolchainConfigError

    with pytest.raises(LlvmToolchainConfigError, match="identity|changed|drift"):
        load_wasi_sdk_installation(
            Path(__file__).resolve().parents[2], plan.sdk.parent, verify_tree=True
        )
