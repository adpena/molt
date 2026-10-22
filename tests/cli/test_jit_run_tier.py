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
from molt.cli.native_launch_plan import NativeLaunchPlan
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.run_tier import JIT_START_RECEIPT, jit_program_started
from molt.cli.run_tier import (
    DEFAULT_RUN_TIER,
    JIT_IMAGE_SUFFIX,
    RunTier,
    jit_tier_refusal,
)
from molt.compilation_target import TargetKind, admit_target
from molt.native_target import resolve_native_target_spec
from tests.cli.native_link_test_support import transport_codegen_binding
from tests.cli.test_installed_compiler import installation  # noqa: F401  (fixture)


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
_SOURCE_CHECKOUT = Path(__file__).resolve().parents[2]


def _refusal(*, target=TargetKind.NATIVE, codegen="cranelift", host=_VERIFIED_HOST):
    return jit_tier_refusal(
        target=target,
        codegen_backend=codegen,
        host=host,
        compiler_root=_SOURCE_CHECKOUT,
    )


def test_jit_names_every_construct_it_cannot_run() -> None:
    assert _refusal() is None
    cross = resolve_native_target_spec("x86_64-unknown-linux-gnu")
    if not cross.is_host:
        assert "x86_64-unknown-linux-gnu" in (_refusal(target=cross) or "")
    assert "llvm" in (_refusal(codegen="llvm") or "")
    assert "wasm" in (_refusal(target=TargetKind.WASM) or "")


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
    reason = _refusal(host=resolve_native_target_spec(triple))
    assert reason is not None and triple in reason and "HF-371" in reason


def test_an_installed_compiler_is_refused_before_any_jit_work(
    installation,  # noqa: F811  (the imported fixture)
) -> None:
    # An installed Molt ships no runner; the refusal must come from the
    # admission, not from a runtime image link or a runner build.
    assert "installed Molt compiler" in (
        jit_tier_refusal(
            target=TargetKind.NATIVE,
            codegen_backend="cranelift",
            host=_VERIFIED_HOST,
            compiler_root=installation,
        )
        or ""
    )


@pytest.mark.parametrize(
    ("build_args", "construct"),
    [
        (["--target", "wasm"], "wasm"),
        (["--target", "llvm"], "llvm"),
        (["--backend", "llvm"], "llvm"),
    ],
)
def test_request_resolution_refuses_a_jit_request_before_the_frontend(
    build_args: list[str], construct: str, tmp_path: Path
) -> None:
    from molt.cli.build_request_resolution import resolve_build_arguments

    source = tmp_path / "app.py"
    source.write_text("print(1)\n", encoding="utf-8")
    with pytest.raises(ValueError, match="The JIT tier does not support") as refused:
        resolve_build_arguments(
            ["--emit", "jit", *build_args],
            file_path=str(source),
            module=None,
            project_root=tmp_path,
            env={},
        )
    # The target comes first; on an unverified host the host comes next.
    host = resolve_native_target_spec(None)
    verified = host.os == "macos" and host.arch == "aarch64"
    expected = construct if verified or construct == "wasm" else host.triple
    assert expected in str(refused.value)
    # The same request without the JIT tier resolves.
    resolve_build_arguments(
        build_args, file_path=str(source), module=None, project_root=tmp_path, env={}
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


def _program_exits(
    tmp_path: Path, status: int, *, starts: bool, receipt: Path
) -> list[str]:
    """A real process that exits with ``status``, after writing the start
    receipt when ``starts``, as molt-jit does."""
    import sys

    script = (
        "import pathlib, sys\n"
        f"if {starts!r}:\n"
        f"    pathlib.Path({str(receipt)!r}).write_text({JIT_START_RECEIPT!r})\n"
        "sys.stderr.write('molt-jit: compiler said no\\n')\n"
        f"sys.exit({status})\n"
    )
    return [sys.executable, "-c", script]


@pytest.mark.parametrize("json_output", [False, True])
def test_molt_run_reports_a_jit_compile_failure_as_a_failed_build(
    tmp_path: Path, capsys, json_output: bool
) -> None:
    import json

    from molt.cli.script_commands import _run_program
    from molt.process_environment import ProcessEnvironment

    receipt = tmp_path / "started"

    def run(command: list[str], start_receipt: Path | None) -> int:
        return _run_program(
            command,
            env=ProcessEnvironment(os.environ),
            cwd=tmp_path,
            json_output=json_output,
            verbose=False,
            timing=False,
            build_duration_s=0.0,
            start_receipt=start_receipt,
        )

    # The runner exits 125 before its program starts: a failed build.
    assert (
        run(_program_exits(tmp_path, 125, starts=False, receipt=receipt), receipt) == 1
    )
    captured = capsys.readouterr()
    if json_output:
        payload = json.loads(captured.out)
        assert payload["status"] == "error"
        assert "could not compile" in payload["errors"][0]
        assert "compiler said no" in payload["data"]["build_stderr"]
    else:
        assert "could not compile" in captured.err
    # A program that started and exits 125 keeps its own status.
    assert (
        run(_program_exits(tmp_path, 125, starts=True, receipt=receipt), receipt) == 125
    )
    receipt.unlink()
    # An AOT executable has no receipt; its status is the program's.
    assert (
        run(_program_exits(tmp_path, 125, starts=False, receipt=receipt), None) == 125
    )


def _runtime_image_binding(root: Path, payload: bytes):
    from tests.cli.native_link_test_support import (
        native_codegen_binding,
        write_test_static_archive,
    )
    from tests.runtime_build_identity_helper import native_runtime_staticlib_identity

    archive = root / f"libmolt_runtime.{payload.decode()}.a"
    write_test_static_archive(archive, payload=payload)
    return native_codegen_binding(
        archive,
        native_runtime_staticlib_identity(
            cargo_profile="dev-fast",
            target_triple=None,
            family_seed="jit-image-retention",
            host_target=resolve_native_target_spec(None).triple,
        ),
    )


@pytest.mark.skipif(
    resolve_native_target_spec(None).object_format.value != "macho",
    reason="the Mach-O runtime image is the verified JIT cell",
)
def test_runtime_images_are_retired_by_last_use(monkeypatch, tmp_path: Path) -> None:
    # Each runtime generation links a 37 MiB image. Images no build or cached
    # launch used for the idle window leave at the next publication; an image
    # a cached launch still runs stays.
    from molt.cli import generation_store, jit_runtime_image

    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang"], None, None),
    )

    def link(command, **kwargs):
        Path(command[command.index("-o") + 1]).write_bytes(b"image")
        return __import__("subprocess").CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(jit_runtime_image, "_run_completed_command", link)
    now = {"t": 1_000.0}
    monkeypatch.setattr(generation_store.time, "time", lambda: now["t"])
    idle = generation_store.GENERATION_STORE_IDLE_SECONDS
    root = tmp_path / "runtime"
    root.mkdir()

    def ensure(payload: bytes) -> Path:
        return jit_runtime_image.ensure_jit_runtime_image(
            binding=_runtime_image_binding(root, payload),
            target=resolve_native_target_spec(None),
            sysroot_path=None,
            profile="dev",
            link_timeout=None,
        )

    a = ensure(b"a")
    now["t"] = 2_000.0
    b = ensure(b"b")
    # A cached launch runs image a well inside its window.
    now["t"] = 50_000.0
    jit_runtime_image.record_jit_runtime_image_use(a)
    now["t"] = 2_000.0 + idle + 1
    c = ensure(b"c")
    assert a.is_file() and b.is_file() and c.is_file()
    now["t"] = 50_000.0 + idle + 1
    d = ensure(b"d")
    # Only a went idle: b and c were each renewed when the next image was
    # selected, and d is selected.
    assert not a.parent.exists()
    assert b.is_file() and c.is_file() and d.is_file()


def test_a_cached_jit_launch_is_keyed_by_the_runner_identity(
    monkeypatch, tmp_path: Path
) -> None:
    # A cache hit runs the cached image under its recorded runner. The key
    # must change when the runner's own sources change, or a hit would run a
    # stale runner; AOT keys must not depend on the runner.
    from dataclasses import replace
    from types import SimpleNamespace

    from molt.cli import wrapper_build
    from molt.cli.build_request_resolution import resolve_build_arguments
    from molt.host_tool_products import HostToolProduct
    from molt.target_python import _parse_target_python_version

    source = tmp_path / "main.py"
    source.write_text("pass\n", encoding="utf-8")
    entry = SimpleNamespace(
        source_path=source,
        entry_module="main",
        target_python=_parse_target_python_version("3.12"),
        external_module_roots=(),
    )
    monkeypatch.setattr(
        wrapper_build,
        "_wrapper_build_dependency_fingerprints",
        lambda **kw: ([], {"image": {}}),
    )
    monkeypatch.setattr(wrapper_build, "_cache_tooling_fingerprint", lambda: "tooling")
    runner = {"identity": "runner-a"}
    monkeypatch.setattr(
        wrapper_build,
        "_cache_fingerprint",
        lambda **kw: (
            runner["identity"]
            if kw.get("product") is HostToolProduct.JIT
            else "backend"
        ),
    )
    aot_request = resolve_build_arguments(
        [], file_path=str(source), module=None, project_root=tmp_path, env={}
    )

    def key(request) -> str:
        cache_input = wrapper_build._wrapper_build_cache_input(
            resolved_build_entry=entry,
            request=request,
            build_args=[],
            env={},
            project_root=tmp_path,
        )
        assert cache_input is not None
        return cache_input[1]

    jit_request = replace(aot_request, emit="jit")
    jit_before, aot_before = key(jit_request), key(aot_request)
    runner["identity"] = "runner-b"
    assert key(jit_request) != jit_before
    assert key(aot_request) == aot_before


def test_the_differential_jit_adapter_uses_the_one_admission(monkeypatch) -> None:
    # The harness must refuse the JIT tier on exactly the hosts the CLI
    # refuses, so it never runs a build that admission will reject.
    from molt import native_target
    from tools.compat import backends

    unverified = native_target.resolve_native_target_spec("x86_64-unknown-linux-gnu")
    resolve = native_target.resolve_native_target_spec
    monkeypatch.setattr(
        native_target,
        "resolve_native_target_spec",
        lambda triple=None: unverified if triple is None else resolve(triple),
    )
    availability = backends.JitAdapter().availability()
    assert not availability.available
    assert "x86_64-unknown-linux-gnu" in (availability.reason or "")


def test_an_output_that_cannot_carry_native_artifacts_fails_before_the_frontend(
    monkeypatch, tmp_path: Path
) -> None:
    # The import plan names every external native artifact before lowering,
    # so a JIT image (or any output that cannot link them) is refused before
    # the frontend spends its time.
    from types import SimpleNamespace

    from molt.cli import build_pipeline
    from molt.cli.external_native import (
        _external_native_artifact_output_custody_error,
    )

    layout = _layout(tmp_path, target=TargetKind.NATIVE, emit="jit")
    artifact = SimpleNamespace(
        module="ext",
        package="ext",
        target_triple=resolve_native_target_spec(None).triple,
        runtime_linkage="static_link",
        artifact_kind="static_archive",
    )
    plan = SimpleNamespace(artifacts=(artifact,))
    # The real custody authority refuses this output.
    reason = _external_native_artifact_output_custody_error(
        native_artifact_plan=plan, output_layout=layout, target=TargetKind.NATIVE
    )
    assert reason is not None and "emit=jit" in reason

    def frontend(**_kwargs):
        raise AssertionError("the frontend ran before native-artifact custody")

    monkeypatch.setattr(build_pipeline, "_run_frontend_pipeline", frontend)
    failures: list[str] = []
    monkeypatch.setattr(
        build_pipeline,
        "_fail",
        lambda message, json_output, command: failures.append(message) or 2,
    )
    bundle = SimpleNamespace(
        prepared_frontend_run_ticket=object(),
        output_layout=layout,
        native_artifact_plan=plan,
        build_diagnostics_payload=lambda: (None, None),
    )
    result = build_pipeline._run_build_pipeline(
        prepared_build_preamble=SimpleNamespace(
            diagnostics_enabled=False, resolved_diagnostics_verbosity="summary"
        ),
        prepared_build_roots=None,
        prepared_build_config=None,
        resolved_build_entry=None,
        prepared_frontend_pipeline_bundle=bundle,
        profile="dev",
        json_output=False,
        target=TargetKind.NATIVE,
        cache_dir=None,
        cache=False,
        cache_report=False,
        deterministic=True,
        trusted=False,
        verbose=False,
        require_linked=False,
    )
    assert result == 2
    assert failures == [reason]
