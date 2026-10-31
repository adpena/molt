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
    assert plan.image_header_payload() == {
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
    os.name == "nt", reason="the JIT image is a POSIX interpreter-line program"
)


# A binary that prints the argv it was started with: the kernel starts an
# interpreter-line image as `<runner> --image <image> [args...]`. A script
# cannot stand in, because macOS refuses a script as an interpreter.
_ECHO_RUNNER = Path("/bin/echo")


_HEADER = {
    "runtime_image": "/runtime/libmolt_runtime.dylib",
    "backend_environment": {"MOLT_BACKEND": "cranelift"},
    "compiler_diagnostics": "on_failure",
    "tir_cache_root": "/cache/molt",
    "launch": {
        "environment": {"MOLT_EXECUTION_TARGET": "native"},
        "runtime_module_roots": [],
        "compiled_source_paths": ["/src/app.py"],
    },
}


def _program(ir):
    from molt.cli.backend_ir_transport import BackendIrProgram
    from molt.cli.cache_keys import FunctionDigests

    return BackendIrProgram(ir, FunctionDigests())


@_POSIX_ONLY
def test_a_jit_image_holds_its_header_and_program_in_one_file(tmp_path: Path) -> None:
    import json

    import msgspec

    from molt.cli.jit_image import JIT_IMAGE_MARKER, publish_jit_image

    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    function = {"name": "molt_main", "ops": [{"kind": "ret_void"}]}
    ir = {"functions": [function], "profile": None}
    publish_jit_image(image, runner=_ECHO_RUNNER, header=_HEADER, program=_program(ir))
    interpreter, marker, rest = image.read_bytes().split(b"\n", 2)
    assert interpreter == b"#!/bin/echo --image"
    name, header_len, program_len = marker.decode().split(" ")
    assert name == JIT_IMAGE_MARKER
    assert len(rest) == int(header_len) + int(program_len)
    assert json.loads(rest[: int(header_len)]) == _HEADER
    # The program is a content-addressed lease that ships every body.
    lease = msgspec.msgpack.decode(rest[int(header_len) :])
    assert lease["profile"] is None
    assert [msgspec.msgpack.decode(body) for body in lease["function_bodies"]] == [
        function
    ]
    assert lease["function_digests"] == b"".join(_program(ir).digests)
    assert os.access(image, os.X_OK)


@_POSIX_ONLY
@pytest.mark.parametrize("umask,mode", [(0o077, 0o700), (0o022, 0o755), (0o002, 0o775)])
def test_a_jit_image_is_executable_through_the_umask(
    tmp_path: Path, umask: int, mode: int
) -> None:
    # As the linker creates an AOT executable: 0o777 less the umask.
    from molt.cli.jit_image import publish_jit_image

    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    previous = os.umask(umask)
    try:
        publish_jit_image(
            image,
            runner=_ECHO_RUNNER,
            header=_HEADER,
            program=_program({"functions": []}),
        )
    finally:
        os.umask(previous)
    assert image.stat().st_mode & 0o777 == mode


@_POSIX_ONLY
def test_executing_a_jit_image_starts_its_runner_with_no_shell(
    tmp_path: Path,
) -> None:
    # A program's sys.executable is its argv[0], the image. Spawn-mode
    # multiprocessing and subprocess.run([sys.executable, ...]) execute it.
    # The kernel starts the runner named on the image's first line, with no
    # shell between, so no argument is expanded or reparsed.
    from molt.cli.jit_image import publish_jit_image
    from molt.cli.run_tier import jit_runner_argv
    from tests.process_guard_common import run_guarded_test_process

    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    publish_jit_image(
        image, runner=_ECHO_RUNNER, header=_HEADER, program=_program({"functions": []})
    )
    arguments = ["it's", "$HOME", "--start-receipt", "*"]
    completed = run_guarded_test_process(
        [str(image), *arguments], capture_output=True, text=True, check=True
    )
    # The kernel's command is the one molt run gives the runner, less the
    # start receipt: one spelling of the runner command.
    expected = jit_runner_argv(str(_ECHO_RUNNER), str(image), arguments)[1:]
    assert completed.stdout == " ".join(expected) + "\n"
    assert expected[:2] == ["--image", str(image)]


@_POSIX_ONLY
@pytest.mark.parametrize(
    "runner,reason",
    [
        (Path("relative/molt-jit"), "not absolute"),
        (Path("/runner dir/molt-jit"), "' '"),
        (Path("/runner\tdir/molt-jit"), "'\\t'"),
        # macOS's interpreter-line parser stops at '#' (HF-371 review v4).
        (Path("/runner#dir/molt-jit"), "'#'"),
        (Path("/" + "r" * 600 + "/molt-jit"), "interpreter line"),
    ],
)
def test_a_runner_path_the_kernel_cannot_start_is_refused(
    tmp_path: Path, runner: Path, reason: str
) -> None:
    from molt.cli.jit_image import publish_jit_image

    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    with pytest.raises(ValueError) as refused:
        publish_jit_image(
            image, runner=runner, header=_HEADER, program=_program({"functions": []})
        )
    assert reason in str(refused.value)
    assert not image.exists()


@_POSIX_ONLY
def test_a_jit_image_stops_when_its_runner_generation_is_gone(
    tmp_path: Path,
) -> None:
    # The image names one runner generation of this checkout and does not
    # renew it. After idle eviction or target removal the kernel cannot start
    # its interpreter: a shell reports a bad interpreter and exits 126 (bash
    # names a prefix of the runner path), and execve itself fails with ENOENT
    # (docs/cli-reference.md, --emit jit).
    from molt.cli.jit_image import publish_jit_image
    from tests.process_guard_common import run_guarded_test_process

    runner = tmp_path / "evicted" / "molt-jit"
    image = tmp_path / f"app_molt{JIT_IMAGE_SUFFIX}"
    publish_jit_image(
        image, runner=runner, header=_HEADER, program=_program({"functions": []})
    )
    completed = run_guarded_test_process(
        ["/bin/sh", "-c", '"$0"', str(image)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode in (126, 127)
    assert "bad interpreter" in completed.stderr or "not found" in completed.stderr
    with pytest.raises(FileNotFoundError):
        run_guarded_test_process([str(image)], check=False)


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
def test_runtime_images_are_product_store_entries(monkeypatch, tmp_path: Path) -> None:
    # Each runtime generation links a 37 MiB image. An image is its own product
    # store entry, never a write into the published runtime entry it links
    # from (HF-390 review v2, M1). Images no build or cached launch used for
    # the idle window leave at the next publication, and a cached launch of an
    # evicted image is a miss.
    import json
    import subprocess
    import time

    from molt.cli import jit_runtime_image, product_store

    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang"], None, None),
    )
    links: list[list[str]] = []

    def link(command, **kwargs):
        links.append(command)
        Path(command[command.index("-o") + 1]).write_bytes(b"image")
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(jit_runtime_image, "_run_completed_command", link)
    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    runtime = tmp_path / "runtime"
    runtime.mkdir()
    store_dir = tmp_path / ".molt-products"
    store = product_store.ProductStore(store_dir / "v1" / "jit-runtime-image")

    def renew(path: Path) -> bool:
        return product_store.record_stored_file_use(path, store_dir)

    def ensure(payload: bytes) -> Path:
        binding = _runtime_image_binding(runtime, payload)
        before = sorted(runtime.rglob("*"))
        image = jit_runtime_image.ensure_jit_runtime_image(
            store=store,
            binding=binding,
            target=resolve_native_target_spec(None),
            sysroot_path=None,
            profile="dev",
            link_timeout=None,
        )
        # Nothing is written into the runtime the image links from.
        assert sorted(runtime.rglob("*")) == before
        return image

    a = ensure(b"a")
    assert a.parent.parent == store.root and a.read_bytes() == b"image"
    assert ensure(b"a") == a and len(links) == 1
    assert renew(a)

    ledger_path = store.root / ".product-store.json"
    ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    ledger["last_used"][a.parent.name] = (
        time.time() - product_store.PRODUCT_STORE_IDLE_SECONDS - 60
    )
    ledger_path.write_text(json.dumps(ledger), encoding="utf-8")
    b = ensure(b"b")
    assert not a.parent.exists() and b.is_file()
    assert not renew(a)

    # The next builder clears what a dead builder left in the entry, and links.
    c = ensure(b"c")
    c.unlink()
    (c.parent / ".staged").mkdir()
    (c.parent / ".staged" / c.name).write_bytes(b"partial")
    assert not renew(c)
    assert ensure(b"c") == c and c.read_bytes() == b"image"
    assert sorted(path.name for path in c.parent.iterdir()) == [c.name]
    assert len(links) == 4


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


def test_a_refused_runner_stops_the_jit_build_before_any_runtime_work(
    monkeypatch, capsys
) -> None:
    # The runner's probe refuses a host that blocks executable memory. That
    # refusal must come before the runtime build and the 37 MiB image link
    # (HF-371 review v3, finding 7).
    from types import SimpleNamespace

    from molt.cli import jit_pipeline

    def runtime_work(*_args, **_kwargs):
        raise AssertionError("runtime work ran before the runner was admitted")

    def refuse(*_args, **_kwargs):
        raise ValueError("JIT runner admission failed: the host blocks W^X")

    monkeypatch.setattr(
        jit_pipeline._backend_ir,
        "_prepare_backend_ir",
        lambda **_kwargs: (SimpleNamespace(required_link_features=(), ir={}), None),
    )
    monkeypatch.setattr(jit_pipeline, "ensure_jit_runner", refuse)
    monkeypatch.setattr(
        jit_pipeline, "_initialize_runtime_artifact_state", runtime_work
    )
    monkeypatch.setattr(jit_pipeline, "ensure_jit_runtime_image", runtime_work)
    bundle = SimpleNamespace(
        output_layout=SimpleNamespace(emit_ir_path=None, target_triple=None),
        native_artifact_plan=None,
        module_graph={},
        prepared_frontend_run_ticket=SimpleNamespace(
            frontend_layer_execution_context=SimpleNamespace(
                module_graph_metadata=SimpleNamespace(
                    logical_source_path_by_module={},
                    namespace_portions_by_module={},
                    module_source_role_by_module={},
                    scan_authority=None,
                )
            )
        ),
        known_modules=(),
        stdlib_allowlist=(),
        integration_state=None,
        module_order=(),
        runtime_import_dispatch_roots=(),
        spawn_enabled=False,
        module_source_catalog=SimpleNamespace(validate=lambda: None),
    )
    status = jit_pipeline._run_jit_pipeline(
        prepared_build_preamble=SimpleNamespace(rust_cpu=None, warnings=[]),
        prepared_build_roots=SimpleNamespace(molt_root=Path("."), sysroot_path=None),
        prepared_build_config=SimpleNamespace(
            pgo_profile_summary=None,
            runtime_feedback_summary=None,
            target_python=None,
            backend_cargo_profile="dev-fast",
            cargo_timeout=None,
        ),
        resolved_build_entry=SimpleNamespace(entry_module="main"),
        prepared_frontend_pipeline_bundle=bundle,
        profile="dev",
        json_output=False,
        target=None,
        deterministic=True,
        trusted=False,
        stdlib_profile=None,
    )
    assert status != 0
    assert "blocks W^X" in capsys.readouterr().err


@_POSIX_ONLY
def test_a_jit_image_names_the_cache_root_the_cli_resolved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The runner's environment and current directory are the program's, so
    # the image names the TIR cache root by the CLI's rules (MOLT_CACHE, else
    # XDG_CACHE_HOME/molt, else ~/.cache/molt), resolved where the build ran
    # (HF-371 review v5).
    from molt.cli.jit_pipeline import jit_image_header

    work = tmp_path / "work"
    work.mkdir()
    monkeypatch.chdir(work)
    for name in ("MOLT_CACHE", "MOLT_EXT_ROOT", "XDG_CACHE_HOME"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("HOME", str(tmp_path / "home"))

    def root() -> str:
        return jit_image_header(
            runtime_image=Path("/runtime/libmolt_runtime.dylib"),
            backend_environment={},
            launch={},
        )["tir_cache_root"]

    assert root() == str(tmp_path / "home" / ".cache" / "molt")
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "xdg"))
    assert root() == str(tmp_path / "xdg" / "molt")
    monkeypatch.setenv("MOLT_CACHE", "relative-cache")
    assert root() == str(work / "relative-cache")


def test_a_cache_hit_renews_every_store_entry_a_jit_start_reads(
    monkeypatch, tmp_path: Path
) -> None:
    """A JIT start reads its runner, its runtime image and the runtime's
    callable-symbol table, each in its own product store entry (HF-390). A
    cache hit renews all three; the runtime entry is the one no other use
    renews, so its eviction must be a miss, not a failed start."""
    import hashlib
    import json

    from molt.cli import wrapper_build
    from molt.cli.backend_generation import backend_generation_path
    from molt.cli.product_store import LAYOUT_VERSION, ProductStore
    from molt.dx import PRODUCT_STORE_DIRNAME

    # The store a build of the compiler checkout uses under this artifact
    # root; the cache reader must resolve the same one (review v4).
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifacts))
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    root = artifacts / "target" / PRODUCT_STORE_DIRNAME / LAYOUT_VERSION
    identity, variant = "a" * 64, "b" * 64
    runtime = ProductStore(root / "native-runtime")
    generation = runtime.entry(identity) / variant / ".molt-native-generations" / "c"
    files = {
        "runner": (
            ProductStore(root / "host-tool"),
            backend_generation_path(
                ProductStore(root / "host-tool").entry(identity) / variant / "molt-jit",
                hashlib.sha256(b"runner").hexdigest(),
            ),
        ),
        "runtime_image": (
            ProductStore(root / "jit-runtime-image"),
            ProductStore(root / "jit-runtime-image").entry(identity)
            / "libmolt_runtime.dylib",
        ),
        "callable_symbols": (
            runtime,
            generation / "dev-fast" / "libmolt_runtime.a.callable_symbols.v3.txt",
        ),
    }
    for _store, path in files.values():
        path.parent.mkdir(parents=True)
        path.write_bytes(path.name.encode())
    image = tmp_path / "app_molt.jit"
    image.write_bytes(b"image")
    monkeypatch.setattr(
        wrapper_build,
        "_wrapper_build_default_consumer_path",
        lambda entry, *, tier: image,
    )
    from molt.dx import product_store_dir
    from molt.source_root import compiler_source_root

    assert product_store_dir(compiler_source_root(), os.environ) == root.parent
    wrapper_build._wrapper_build_cache_manifest_path(image).write_text(
        json.dumps(
            {
                "version": wrapper_build._WRAPPER_BUILD_CACHE_SCHEMA_VERSION,
                "cache_key": "key",
                "consumer_output": str(image),
                "binary_sha256": hashlib.sha256(b"image").hexdigest(),
                "output": str(image),
                "artifacts": {name: str(path) for name, (_s, path) in files.items()},
            }
        ),
        encoding="utf-8",
    )

    def hit():
        return wrapper_build._read_wrapper_build_cache_contract(
            resolved_build_entry=None,
            cache_input=({}, "key"),
            tier=RunTier.JIT,
        )

    assert hit() is not None
    for store, _path in files.values():
        ledger = json.loads((store.root / ".product-store.json").read_text("utf-8"))
        assert identity in ledger["last_used"], store.kind
    # Eviction (idle or `molt cache evict`) removes the entry and its record.
    assert runtime.evict(identity)
    assert hit() is None


@pytest.mark.parametrize("location", ["owned", "external", "blocked"])
def test_jit_publication_binds_selected_scratch_before_the_runner_can_write(
    tmp_path, monkeypatch, location, developer_host_context
):
    from types import SimpleNamespace
    import molt.backend_environment as backend_environment
    from molt.checkout_state import (
        CheckoutOwner,
        read_checkout_owner,
        scratch_state_root,
    )
    from molt.cli import jit_pipeline
    from molt.cli.rust_cpu_policy import RustCpuPolicy

    compiler = tmp_path / "compiler"
    compiler.mkdir()
    artifacts = tmp_path / "artifacts"
    monkeypatch.setattr(backend_environment, "compiler_source_root", lambda: compiler)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifacts))
    monkeypatch.delenv("MOLT_SCRATCH_STORAGE", raising=False)
    monkeypatch.delenv("MOLT_DEBUG_ARTIFACT_DIR", raising=False)
    owned = scratch_state_root(artifacts / "tmp", owner=compiler, create=False)
    selected = owned / "molt-backend"
    if location == "external":
        selected = tmp_path / "caller" / "molt-backend"
        monkeypatch.setenv("MOLT_DEBUG_ARTIFACT_DIR", str(selected))
    if location == "blocked":
        artifacts.mkdir()
        (artifacts / "tmp").write_text("not a scratch directory", encoding="utf-8")
    binding = transport_codegen_binding(tmp_path / "runtime")
    published = []

    def publish(image, *, header, **kwargs):
        effective = header["backend_environment"]
        assert effective["MOLT_DEBUG_ARTIFACT_DIR"] == str(selected)
        assert "MOLT_EXT_ROOT" not in effective
        if location == "owned":
            assert read_checkout_owner(owned) == CheckoutOwner(
                compiler.resolve(), "scratch"
            )
        else:
            assert location == "external"
            assert not artifacts.exists()
        # Model the runner's actual mkdir/write boundary after publication.
        selected.mkdir(parents=True)
        (selected / "object.o").write_bytes(b"runner scratch")
        published.append(image)

    monkeypatch.setattr(jit_pipeline, "publish_jit_image", publish)
    monkeypatch.setattr(
        jit_pipeline, "ensure_jit_runner", lambda *a, **k: tmp_path / "runner"
    )
    monkeypatch.setattr(
        jit_pipeline, "ensure_jit_runtime_image", lambda **k: tmp_path / "runtime.dylib"
    )
    monkeypatch.setattr(
        jit_pipeline, "jit_runtime_image_store", lambda _: tmp_path / "images"
    )
    monkeypatch.setattr(
        jit_pipeline,
        "_initialize_runtime_artifact_state",
        lambda **k: SimpleNamespace(native_runtime_codegen_binding=binding),
    )
    monkeypatch.setattr(
        jit_pipeline,
        "_stage_runtime_callable_symbols_for_native_codegen",
        lambda *a, **k: ("digest", None),
    )
    monkeypatch.setattr(
        jit_pipeline._backend_ir,
        "_prepare_backend_ir",
        lambda **k: (
            SimpleNamespace(required_link_features=(), ir={}, function_digest_seeds=()),
            None,
        ),
    )
    monkeypatch.setattr(
        jit_pipeline._runtime_features,
        "runtime_stdlib_profile_for_required_features",
        lambda *a, **k: "micro",
    )
    monkeypatch.setattr(
        jit_pipeline.NativeLaunchPlan,
        "for_program",
        lambda **k: SimpleNamespace(image_header_payload=lambda: {}),
    )
    monkeypatch.setattr(jit_pipeline, "_build_common_build_json_data", lambda **k: {})
    monkeypatch.setattr(jit_pipeline, "_emit_build_result_json", lambda **k: 0)
    bundle = SimpleNamespace(
        output_layout=SimpleNamespace(
            emit_ir_path=None,
            target_triple=None,
            output_artifact=tmp_path / "app.moltjit",
            emit_mode="jit",
        ),
        native_artifact_plan=None,
        module_graph={},
        prepared_frontend_run_ticket=SimpleNamespace(
            frontend_layer_execution_context=SimpleNamespace(
                module_graph_metadata=SimpleNamespace(
                    logical_source_path_by_module={},
                    namespace_portions_by_module={},
                    module_source_role_by_module={},
                    scan_authority=None,
                )
            )
        ),
        known_modules=(),
        stdlib_allowlist=(),
        integration_state=None,
        module_order=(),
        runtime_import_dispatch_roots=(),
        spawn_enabled=False,
        module_source_catalog=SimpleNamespace(
            validate=lambda: None, source_paths=lambda _: ()
        ),
    )
    result = jit_pipeline._run_jit_pipeline(
        prepared_build_preamble=SimpleNamespace(
            rust_cpu=RustCpuPolicy.BASELINE, warnings=[]
        ),
        prepared_build_roots=SimpleNamespace(molt_root=compiler, sysroot_path=None),
        prepared_build_config=SimpleNamespace(
            pgo_profile_summary=None,
            runtime_feedback_summary=None,
            target_python=None,
            backend_cargo_profile="dev-fast",
            runtime_cargo_profile="dev-fast",
            cargo_timeout=None,
            link_timeout=None,
            codegen=backend_environment.CodegenSelection(),
            resolved_capability_policy=None,
            capabilities_source=None,
        ),
        resolved_build_entry=SimpleNamespace(
            entry_module="main", source_path=tmp_path / "app.py"
        ),
        prepared_frontend_pipeline_bundle=bundle,
        profile="dev",
        json_output=True,
        target=admit_target("native"),
        deterministic=True,
        trusted=False,
        stdlib_profile=None,
    )
    if location == "blocked":
        assert result != 0 and not published
        assert (artifacts / "tmp").read_text(
            encoding="utf-8"
        ) == "not a scratch directory"
    else:
        assert result == 0 and published == [tmp_path / "app.moltjit"]
        assert (selected / "object.o").read_bytes() == b"runner scratch"
