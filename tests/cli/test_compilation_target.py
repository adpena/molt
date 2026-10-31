from __future__ import annotations

import pytest

from molt.compilation_target import (
    TargetKind,
    admit_target,
    is_wasm_target,
    target_name,
    target_triple,
)
from molt.cli.build_request import BuildRequest, build_request_from_payload
from molt.native_target import NativeTargetSpec
from molt.cli.rust_cpu_policy import RustCpuPolicy, resolve_rust_cpu_policy


@pytest.mark.parametrize(
    "text,kind,triple,wasm",
    [
        ("native", TargetKind.NATIVE, None, False),
        ("wasm", TargetKind.WASM, "wasm32-wasip1", True),
        (
            "wasm-freestanding",
            TargetKind.WASM_FREESTANDING,
            "wasm32-unknown-unknown",
            True,
        ),
        ("wasm32-wasip1", TargetKind.WASI, "wasm32-wasip1", True),
        (
            "wasm32-unknown-unknown",
            TargetKind.WASM_UNKNOWN,
            "wasm32-unknown-unknown",
            True,
        ),
        ("rust", TargetKind.RUST, None, False),
        ("luau", TargetKind.LUAU, None, False),
        ("mlir", TargetKind.MLIR, None, False),
    ],
)
def test_closed_target_identity(text, kind, triple, wasm):
    target = admit_target(text)
    assert target is kind
    assert target != text
    assert target_name(target) == text
    assert target_triple(target) == triple
    assert is_wasm_target(target) is wasm


@pytest.mark.parametrize(
    "triple,os,arch",
    [
        ("x86_64-unknown-linux-gnu", "linux", "x86_64"),
        ("aarch64-apple-darwin", "macos", "aarch64"),
        ("x86_64-pc-windows-msvc", "windows", "x86_64"),
        ("x86_64-pc-windows-gnu", "windows", "x86_64"),
        ("futurearch-unknown-linux-musl", "linux", "futurearch"),
    ],
)
def test_open_native_triples_reuse_admitted_spec(triple, os, arch):
    target = admit_target(triple)
    assert isinstance(target, NativeTargetSpec)
    assert (target.os, target.arch, target.is_host) == (os, arch, False)
    assert target.cargo_target == triple
    assert target_triple(target) == triple


@pytest.mark.parametrize(
    "value", ["", "llvm", "wasm64-wasip1", "x-notlinux-gnu", False, {}]
)
def test_invalid_target_rejects_at_wire_admission(value):
    with pytest.raises((ValueError, TypeError)):
        build_request_from_payload({"file_path": "main.py", "target": value})


def test_default_request_discovers_no_host_or_transport(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("default request performed target discovery or serialization")

    with monkeypatch.context() as patch:
        patch.setattr("molt.compilation_target.resolve_native_target_spec", forbidden)
        patch.setattr("json.dumps", forbidden)
        patch.setattr("subprocess.Popen", forbidden)
        assert BuildRequest(file_path="main.py").target is TargetKind.NATIVE
        assert (
            build_request_from_payload({"file_path": "main.py"}).target
            is TargetKind.NATIVE
        )


def test_explicit_host_spelling_does_not_enable_native_cpu_policy():
    env = {"MOLT_PERF_PROFILE": "native-arch"}
    assert resolve_rust_cpu_policy(TargetKind.NATIVE, env) == (
        RustCpuPolicy.NATIVE,
        None,
    )
    policy, warning = resolve_rust_cpu_policy(admit_target("aarch64-apple-darwin"), env)
    assert policy is RustCpuPolicy.BASELINE
    assert warning is not None


def test_artifact_projection_reuses_admitted_native_spec(monkeypatch):
    from molt.cli import backend_artifact_contract

    selected = admit_target("x86_64-pc-windows-msvc")

    def forbidden(*args, **kwargs):
        pytest.fail("artifact projection reparsed an admitted native target")

    monkeypatch.setattr(
        backend_artifact_contract, "resolve_native_target_spec", forbidden
    )
    contract = backend_artifact_contract.resolve_backend_artifact_contract(
        target=selected, emit_mode="obj"
    )
    assert contract.native_target is selected
    assert contract.suffix == ".obj"


@pytest.mark.parametrize(
    "text,freestanding,triple",
    [
        ("wasm", False, None),
        ("wasm-freestanding", True, None),
        ("wasm32-wasip1", False, "wasm32-wasip1"),
        ("wasm32-unknown-unknown", True, "wasm32-unknown-unknown"),
    ],
)
def test_wasm_consumers_agree_without_erasing_request_abi(
    tmp_path, text, freestanding, triple
):
    from molt.cli.build_inputs import _target_sys_platform
    from molt.cli.build_output_layout import _resolve_build_output_layout
    from molt.cli.source_extension_target import resolve_source_extension_target_plan

    target = admit_target(text)
    layout = _resolve_build_output_layout(
        target=target,
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path,
        bin_root=tmp_path,
        output_root=tmp_path,
        output_base="example",
        out_dir_path=None,
        project_root=tmp_path,
    )
    assert layout.is_wasm is True
    assert layout.is_wasm_freestanding is freestanding
    assert layout.emit_mode == "wasm"
    assert layout.target_triple == triple
    assert _target_sys_platform(target) == "wasi"
    plan = resolve_source_extension_target_plan(target)
    assert plan.requested is target
    assert plan.target_triple == (
        "wasm32-unknown-unknown" if freestanding else "wasm32-wasip1"
    )


@pytest.mark.parametrize("command", ["build", "internal-backend-build"])
def test_invalid_configured_target_stays_with_command_diagnostic_owner(
    command, tmp_path, capsys
):
    from molt.cli.entrypoint_dispatch import _dispatch_entrypoint_command
    from molt.cli.entrypoint_parser import _build_entrypoint_parser

    def forbidden(request):
        pytest.fail("invalid target reached compilation")

    parser = _build_entrypoint_parser()
    args = parser.parse_args([command, "main.py"] if command == "build" else [command])
    result = _dispatch_entrypoint_command(
        args,
        parser=parser,
        build_fn=forbidden,
        config_root=tmp_path,
        config={},
        build_cfg={"target": "x86_64-notlinux-gnu"},
        run_cfg={},
        compare_cfg={},
        test_cfg={},
        diff_cfg={},
        extension_cfg={},
        publish_cfg={},
        cfg_capabilities=None,
    )
    assert result != 0
    assert "no object-format policy" in capsys.readouterr().err


def test_resolved_host_spec_rejects_before_probes_or_serialization(monkeypatch):
    from molt.native_target import resolve_native_target_spec
    from molt.cli.source_extension_target import resolve_source_extension_target_plan
    from molt.compilation_target import native_target_spec

    resolved_host = resolve_native_target_spec(
        None, host_platform="darwin", host_arch="arm64"
    )
    explicit = admit_target("aarch64-apple-darwin")

    def forbidden(*args, **kwargs):
        pytest.fail("invalid compilation intent performed discovery or serialization")

    with monkeypatch.context() as patch:
        patch.setattr("molt.compilation_target.resolve_native_target_spec", forbidden)
        patch.setattr("json.dumps", forbidden)
        patch.setattr("subprocess.Popen", forbidden)
        for consume in (
            lambda value: BuildRequest(file_path="main.py", target=value),
            target_name,
            target_triple,
            native_target_spec,
            resolve_source_extension_target_plan,
        ):
            with pytest.raises(ValueError, match="TargetKind.NATIVE"):
                consume(resolved_host)
        request = BuildRequest(file_path="main.py", target=explicit)
        assert request.target is explicit
        assert target_triple(request.target) == "aarch64-apple-darwin"


@pytest.mark.parametrize("target", ["", False, None])
def test_invalid_configured_target_does_not_become_host(target):
    from molt.cli.build_request_resolution import (
        parse_build_arguments,
        resolve_build_request,
    )

    with pytest.raises(ValueError, match="target must be a nonempty"):
        resolve_build_request(
            parse_build_arguments([], file_path="main.py"),
            build_cfg={"target": target},
            cfg_capabilities=None,
            env={},
        )


@pytest.mark.parametrize(
    "target", ["wasm-freestanding", "wasm32-unknown-unknown", "rust"]
)
def test_run_rejects_nonexecutable_target_before_workload(
    monkeypatch, tmp_path, capsys, target
):
    from molt.cli import entrypoint, script_commands

    source = tmp_path / "main.py"
    source.write_text("pass\n", encoding="utf-8")

    def forbidden(*args, **kwargs):
        pytest.fail("unsupported execution target reached workload")

    monkeypatch.setattr(script_commands, "run_script", forbidden)
    monkeypatch.setattr(script_commands, "_run_script_cross", forbidden)
    monkeypatch.setattr(
        "sys.argv", ["molt", "run", f"--build-arg=--target={target}", str(source)]
    )
    assert entrypoint.main() != 0
    assert "cannot execute target" in capsys.readouterr().err


@pytest.mark.parametrize(
    "field,value", [("os", "windows"), ("arch", "aarch64"), ("object_format", "coff")]
)
def test_native_target_derived_facts_cannot_be_supplied_or_replaced(field, value):
    from dataclasses import replace

    target = NativeTargetSpec("x86_64-unknown-linux-gnu")
    with pytest.raises(TypeError):
        NativeTargetSpec(target.triple, **{field: value})
    with pytest.raises(ValueError, match="init=False"):
        replace(target, **{field: value})


@pytest.mark.parametrize("host", [None, 0, 1, "false", []])
def test_native_target_host_intent_requires_actual_bool(host):
    with pytest.raises(TypeError, match="is_host must be a bool"):
        NativeTargetSpec("x86_64-unknown-linux-gnu", is_host=host)


@pytest.mark.parametrize(
    "triple", [None, 1, False, [], "wasm32-wasip1", "x-notlinux-gnu"]
)
def test_native_target_direct_construction_admits_triple(triple):
    with pytest.raises((TypeError, RuntimeError)):
        NativeTargetSpec(triple)


@pytest.mark.parametrize(
    "triple,expected_os,expected_arch,expected_format,suffix",
    [
        ("aarch64-apple-darwin", "macos", "aarch64", "macho", ".o"),
        ("x86_64-pc-windows-msvc", "windows", "x86_64", "coff", ".obj"),
        ("futurearch-unknown-linux-musl", "linux", "futurearch", "elf", ".o"),
    ],
)
def test_native_target_replacement_recomputes_facts_and_preserves_wire_identity(
    monkeypatch, triple, expected_os, expected_arch, expected_format, suffix
):
    from dataclasses import replace
    from molt.cli.build_request import build_request_to_payload
    from molt.native_target import NativeArtifactKind

    def forbidden(*args, **kwargs):
        pytest.fail("explicit target construction probed host")

    monkeypatch.setattr("molt.native_target._host_target_triple", forbidden)
    original = NativeTargetSpec("x86_64-unknown-linux-gnu")
    target = replace(original, triple=triple.upper())
    assert (target.triple, target.os, target.arch, target.object_format.value) == (
        triple,
        expected_os,
        expected_arch,
        expected_format,
    )
    request = BuildRequest(file_path="main.py", target=target)
    restored = build_request_from_payload(build_request_to_payload(request))
    assert restored == request
    assert NativeArtifactKind.OBJECT.suffix(target) == suffix
    assert NativeArtifactKind.OBJECT.suffix(restored.target) == suffix
    assert target.cargo_target == triple

    # Once admitted, direct request and artifact consumers reuse stored facts.
    monkeypatch.setattr("molt.native_target._admit_native_triple", forbidden)
    assert BuildRequest(file_path="main.py", target=target).target is target
    assert NativeArtifactKind.OBJECT.suffix(target) == suffix
    assert target_name(target) == triple


def test_linux_host_libc_is_read_once_per_process(monkeypatch):
    # libc_ver reads the interpreter executable in chunks; the C library is a
    # fact of the running process, so repeated host resolution reads it once.
    import sysconfig

    from molt import native_target

    calls = []

    def libc_ver():
        calls.append(1)
        return ("musl", "1.2")

    native_target.host_linux_libc.cache_clear()
    monkeypatch.setattr(native_target.platform, "libc_ver", libc_ver)
    monkeypatch.setattr(sysconfig, "get_config_var", lambda name: None)
    try:
        triples = {
            native_target._host_target_triple(
                host_platform="linux", host_arch="aarch64"
            )
            for _ in range(3)
        }
    finally:
        native_target.host_linux_libc.cache_clear()
    assert triples == {"aarch64-unknown-linux-musl"}
    assert calls == [1]


@pytest.mark.parametrize(
    "text,triple",
    [
        ("wasm", "wasm32-wasip1"),
        ("wasm32-wasip1", "wasm32-wasip1"),
        ("wasm-freestanding", "wasm32-unknown-unknown"),
        ("wasm32-unknown-unknown", "wasm32-unknown-unknown"),
        ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"),
        ("aarch64-apple-darwin", "aarch64-apple-darwin"),
        ("x86_64-pc-windows-msvc", "x86_64-pc-windows-msvc"),
    ],
)
def test_artifact_target_resolves_explicit_intent_without_the_host(
    monkeypatch, text, triple
):
    from molt.compilation_target import artifact_triple, resolve_artifact_target

    def forbidden(*args, **kwargs):
        pytest.fail("explicit target resolution probed the host")

    monkeypatch.setattr("molt.native_target._host_target_triple", forbidden)
    assert artifact_triple(resolve_artifact_target(admit_target(text))) == triple


def test_artifact_target_resolves_the_symbolic_host_once(monkeypatch):
    from molt.compilation_target import artifact_triple, resolve_artifact_target

    calls = []

    def host(**_):
        calls.append(1)
        return "riscv64gc-unknown-linux-gnu"

    monkeypatch.setattr("molt.native_target._host_target_triple", host)
    target = resolve_artifact_target(TargetKind.NATIVE)
    assert isinstance(target, NativeTargetSpec) and target.is_host
    # The resolved value carries the host; projecting it asks nothing again.
    assert artifact_triple(target) == "riscv64gc-unknown-linux-gnu"
    assert artifact_triple(target) == "riscv64gc-unknown-linux-gnu"
    assert calls == [1]


@pytest.mark.parametrize("kind", [TargetKind.RUST, TargetKind.LUAU, TargetKind.MLIR])
def test_source_backends_have_no_artifact_target(kind):
    from molt.compilation_target import resolve_artifact_target

    with pytest.raises(ValueError, match="has no artifact target"):
        resolve_artifact_target(kind)
