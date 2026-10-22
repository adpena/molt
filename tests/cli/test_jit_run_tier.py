"""The development JIT tier's CLI contracts (HF-371, docs/design/dev_jit_tier.md).

The JIT tier must start a program exactly as an AOT executable does, publish
its launch where the run wrapper looks for it, refuse what it cannot run with
a diagnostic that names the construct, and link its runtime image from the AOT
archive without dead-stripping any runtime symbol.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli import native_link_command
from molt.cli.build_output_layout import _resolve_build_output_layout
from molt.cli.jit_pipeline import _unsupported_jit_construct
from molt.cli.native_launch_plan import NativeLaunchPlan
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.run_tier import JIT_START_RECEIPT, jit_program_started
from molt.cli.run_tier import (
    DEFAULT_RUN_TIER,
    JIT_IMAGE_SUFFIX,
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


def test_jit_emit_publishes_its_image_beside_the_executable_name(
    tmp_path: Path,
) -> None:
    aot = _layout(tmp_path, target=TargetKind.NATIVE, emit=None)
    jit = _layout(tmp_path, target=TargetKind.NATIVE, emit="jit")
    assert aot.output_binary == tmp_path / "bin" / "app_molt"
    assert jit.emit_mode == "jit"
    assert jit.output_binary is None
    assert jit.output_artifact == tmp_path / "bin" / f"app_molt{JIT_IMAGE_SUFFIX}"


@pytest.mark.parametrize("target", ["luau", "rust", "mlir", "wasm"])
def test_jit_emit_requires_the_native_target(tmp_path: Path, target: str) -> None:
    with pytest.raises(ValueError, match="emit"):
        _layout(tmp_path, target=admit_target(target), emit="jit")


_VERIFIED_HOST = resolve_native_target_spec("aarch64-apple-darwin")


def test_jit_names_every_construct_it_cannot_run() -> None:
    assert (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
            host=_VERIFIED_HOST,
        )
        is None
    )
    cross = resolve_native_target_spec("x86_64-unknown-linux-gnu")
    if not cross.is_host:
        reason = _unsupported_jit_construct(
            target=cross,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
            host=_VERIFIED_HOST,
        )
        assert reason is not None and "x86_64-unknown-linux-gnu" in reason
    assert "llvm" in (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="llvm",
            has_external_native_artifacts=False,
            host=_VERIFIED_HOST,
        )
        or ""
    )
    assert "external native package artifacts" in (
        _unsupported_jit_construct(
            target=TargetKind.NATIVE,
            codegen_backend="cranelift",
            has_external_native_artifacts=True,
            host=_VERIFIED_HOST,
        )
        or ""
    )
    assert "wasm" in (
        _unsupported_jit_construct(
            target=TargetKind.WASM,
            codegen_backend="cranelift",
            has_external_native_artifacts=False,
            host=_VERIFIED_HOST,
        )
        or ""
    )


@pytest.mark.parametrize(
    "triple",
    [
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
    ],
)
def test_jit_refuses_every_host_outside_the_verified_cell(triple: str) -> None:
    # Only aarch64 macOS ran the JIT-AOT twin gate; another host must fail
    # before it links and loads an unverified runtime image.
    reason = _unsupported_jit_construct(
        target=TargetKind.NATIVE,
        codegen_backend="cranelift",
        has_external_native_artifacts=False,
        host=resolve_native_target_spec(triple),
    )
    assert reason is not None and triple in reason and "HF-371" in reason


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


def test_only_the_exact_start_receipt_proves_the_program_started(
    tmp_path: Path,
) -> None:
    receipt = tmp_path / "started.json"
    assert not jit_program_started(receipt)
    receipt.write_text('{"schema":"molt.jit-start.v0"}\n', encoding="utf-8")
    assert not jit_program_started(receipt)
    receipt.write_text(JIT_START_RECEIPT, encoding="utf-8")
    assert jit_program_started(receipt)


_POSIX_ONLY = pytest.mark.skipif(
    os.name == "nt", reason="the JIT image is a POSIX shell program"
)


def _fake_runner(tmp_path: Path) -> Path:
    """A runner that records the argv it was started with."""
    runner = tmp_path / "runner's dir" / "molt-jit"
    runner.parent.mkdir()
    runner.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$FAKE_RUNNER_ARGV"\n', encoding="utf-8"
    )
    runner.chmod(0o755)
    return runner


_HEADER = {
    "runtime_image": "/runtime/libmolt_runtime.dylib",
    "backend_environment": {"MOLT_BACKEND": "cranelift"},
    "compiler_diagnostics": "on_failure",
    "launch": {
        "environment": {"MOLT_EXECUTION_TARGET": "native"},
        "runtime_module_roots": [],
        "compiled_source_paths": ["/src/app.py"],
    },
}


@_POSIX_ONLY
def test_a_jit_image_holds_its_header_and_program_in_one_file(tmp_path: Path) -> None:
    import json

    import msgspec

    from molt.cli.jit_image import JIT_IMAGE_MARKER, publish_jit_image

    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    ir = {"functions": [], "profile": None}
    publish_jit_image(image, runner=_fake_runner(tmp_path), header=_HEADER, ir=ir)
    shebang, launch, marker, rest = image.read_bytes().split(b"\n", 3)
    assert shebang == b"#!/bin/sh" and launch.startswith(b"exec ")
    name, header_len, program_len = marker.decode().split(" ")
    assert name == JIT_IMAGE_MARKER
    assert len(rest) == int(header_len) + int(program_len)
    assert json.loads(rest[: int(header_len)]) == _HEADER
    assert msgspec.msgpack.decode(rest[int(header_len) :]) == ir
    assert os.access(image, os.X_OK)


@_POSIX_ONLY
def test_executing_a_jit_image_re_runs_its_program_under_its_runner(
    tmp_path: Path,
) -> None:
    # A program's sys.executable is its argv[0], the image. Spawn-mode
    # multiprocessing and subprocess.run([sys.executable, ...]) execute it;
    # the image must start the same program again, with the new arguments.
    from molt.cli.jit_image import publish_jit_image
    from tests.process_guard_common import run_guarded_test_process

    runner = _fake_runner(tmp_path)
    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    publish_jit_image(image, runner=runner, header=_HEADER, ir={"functions": []})
    recorded = tmp_path / "argv.txt"
    arguments = ["two words", "it's", "$HOME", ""]
    run_guarded_test_process(
        [str(image), *arguments],
        env={**os.environ, "FAKE_RUNNER_ARGV": str(recorded)},
        check=True,
    )
    received = recorded.read_text(encoding="utf-8").split("\n")[:-1]
    # The runner launches the image, and the program sees the image as its
    # argv[0] followed by exactly the arguments the image was started with.
    assert received[:2] == ["--launch", str(image)]
    assert received[received.index("--") + 1 :] == [str(image), *arguments]
