"""The development JIT tier's CLI contracts (HF-371, docs/design/dev_jit_tier.md).

The JIT tier must start a program exactly as an AOT executable does, publish
its launch where the run wrapper looks for it, refuse what it cannot run with
a diagnostic that names the construct, and link its runtime image from the AOT
archive without dead-stripping any runtime symbol.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli import native_link_command
from molt.cli.build_output_layout import _resolve_build_output_layout
from molt.cli.jit_pipeline import _unsupported_jit_construct, jit_program_path
from molt.cli.native_launch_plan import NativeLaunchPlan
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.run_tier import (
    DEFAULT_RUN_TIER,
    JIT_LAUNCH_SUFFIX,
    JIT_PROGRAM_SUFFIX,
    RunTier,
)
from molt.compilation_target import TargetKind, admit_target
from molt.native_target import resolve_native_target_spec
from tests.cli.native_link_test_support import transport_codegen_binding


def _layout(tmp_path: Path, *, target, emit: str | None, output: str | None = None):
    project_root = tmp_path / "project"
    project_root.mkdir(exist_ok=True)
    return _resolve_build_output_layout(
        target=target,
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=emit,
        output=output,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "dist",
        output_base="app",
        out_dir_path=None,
        project_root=project_root,
    )


def test_each_tier_runs_one_build_artifact() -> None:
    assert DEFAULT_RUN_TIER is RunTier.AOT
    assert {tier: tier.build_emit for tier in RunTier} == {
        RunTier.AOT: "bin",
        RunTier.JIT: "jit",
    }


@pytest.mark.parametrize(
    "policy",
    [
        CapabilityManifest().resolve(),
        CapabilityManifest(allow=["fs.read", "env.read"]).resolve(),
    ],
)
def test_jit_launch_environment_is_the_aot_stub_environment(policy) -> None:
    sources = ("/src/app.py", "/src/lib.py")
    plan = NativeLaunchPlan.for_program(
        resolved_capability_policy=policy, compiled_source_paths=sources
    )
    stub = _render_native_main_stub(
        resolved_capability_policy=policy, compiled_source_paths=sources
    )
    # The stub installs each literal key/value pair with setenv on POSIX.
    installed = dict(re.findall(r'setenv\("([^"]+)", "([^"]*)", 1\)', stub))
    assert installed == dict(plan.environment)
    assert plan.environment["MOLT_EXECUTION_TARGET"] == "native"
    assert plan.manifest_payload() == {
        "environment": dict(plan.environment),
        "runtime_module_roots": [],
        "compiled_source_paths": list(sources),
    }


def test_launch_plan_resolves_runtime_module_roots_once(tmp_path: Path) -> None:
    root = tmp_path / "packages"
    plan = NativeLaunchPlan.for_program(
        resolved_capability_policy=CapabilityManifest().resolve(),
        runtime_module_roots=(root, root),
    )
    assert plan.runtime_module_roots == (str(root.resolve()),)


def test_jit_emit_publishes_its_launch_beside_the_executable_name(
    tmp_path: Path,
) -> None:
    aot = _layout(tmp_path, target=TargetKind.NATIVE, emit=None)
    jit = _layout(tmp_path, target=TargetKind.NATIVE, emit="jit")
    assert aot.output_binary == tmp_path / "bin" / "app_molt"
    assert jit.emit_mode == "jit"
    assert jit.output_binary is None
    assert jit.output_artifact == tmp_path / "bin" / f"app_molt{JIT_LAUNCH_SUFFIX}"
    assert jit_program_path(jit.output_artifact) == (
        tmp_path / "bin" / f"app_molt{JIT_PROGRAM_SUFFIX}"
    )


@pytest.mark.parametrize("target", ["luau", "rust", "mlir", "wasm"])
def test_jit_emit_requires_the_native_target(tmp_path: Path, target: str) -> None:
    with pytest.raises(ValueError, match="emit"):
        _layout(tmp_path, target=admit_target(target), emit="jit")


def test_jit_names_every_construct_it_cannot_run() -> None:
    assert (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
        )
        is None
    )
    cross = resolve_native_target_spec("x86_64-unknown-linux-gnu")
    if not cross.is_host:
        reason = _unsupported_jit_construct(
            target=cross,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
        )
        assert reason is not None and "x86_64-unknown-linux-gnu" in reason
    assert "llvm" in (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="llvm",
            has_external_native_artifacts=False,
        )
        or ""
    )
    assert "external native package artifacts" in (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="cranelift",
            has_external_native_artifacts=True,
        )
        or ""
    )
    assert "wasm" in (
        _unsupported_jit_construct(
            target=TargetKind.WASM,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
        )
        or ""
    )


def _runtime_image_plan(monkeypatch, tmp_path: Path, *, target_triple: str | None):
    target = resolve_native_target_spec(target_triple)
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang"], None, None),
    )
    binding = transport_codegen_binding(tmp_path / "runtime")
    output = tmp_path / "image" / "libmolt_runtime.dylib"
    return (
        native_link_command._build_native_runtime_image_link_plan(
            runtime_lib=binding.runtime_lib,
            output_image=output,
            target=target,
            sysroot_path=None,
            profile="dev",
            runtime_codegen_binding=binding,
        ),
        binding,
        output,
    )


@pytest.mark.skipif(
    resolve_native_target_spec(None).object_format.value != "macho",
    reason="the Mach-O runtime image is the verified JIT cell",
)
def test_macho_runtime_image_keeps_every_archive_member(
    monkeypatch, tmp_path: Path
) -> None:
    plan, binding, output = _runtime_image_plan(
        monkeypatch, tmp_path, target_triple=None
    )
    command = list(plan.command)
    assert "-dynamiclib" in command
    assert f"-Wl,-force_load,{binding.runtime_lib}" in command
    # The program, not the runtime, defines the isolate initializer.
    assert "-Wl,-U,_molt_isolate_bootstrap" in command
    assert command[command.index("-o") + 1] == str(output)
    # An AOT link dead-strips unreferenced runtime functions; the image must
    # keep them, because generated code resolves imports in it by name.
    assert not any("dead_strip" in argument for argument in command)
    assert "-Wl,-exported_symbols_list" not in " ".join(command)


def test_runtime_image_refuses_a_cross_target(monkeypatch, tmp_path: Path) -> None:
    cross = "x86_64-unknown-linux-gnu"
    if resolve_native_target_spec(cross).is_host:
        cross = "aarch64-apple-darwin"
    with pytest.raises(RuntimeError, match="host target"):
        _runtime_image_plan(monkeypatch, tmp_path, target_triple=cross)
