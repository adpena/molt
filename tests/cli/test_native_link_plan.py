from __future__ import annotations

from molt import native_target

from molt.native_target import resolve_native_target_spec

from types import SimpleNamespace

from dataclasses import FrozenInstanceError, replace
from pathlib import Path
import os
import subprocess
import sys
import platform

import pytest
from tools.command_execution import CommandExecutor

import molt.cli as cli
from molt.cli import build_results, link_pipeline, native_link_command, native_link_plan
from molt.cli import atomic_io
from molt.cli import link_fingerprints
from molt import file_publication
from molt.native_target import NativeArtifactKind
from molt.native_target_shape import NativeObjectFormat
from tests.cli.native_link_test_support import (
    RUNTIME_BUILD_IDENTITY,
    write_test_darwin_sdk,
)
from tests.native_artifact_fixtures import elf_header, signed_macho_image
from tests.llvm_sdk_test_support import verified_llvm_tools
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkInput,
    SourceExtensionLinkRequirements,
    SourceExtensionLinkLoadingPolicy,
    source_extension_link_file,
)
from tests.process_guard_common import install_module_view
from molt.cli import build_command as cli_build_command
from molt.cli.build_request import BuildRequest


_COMMANDS = CommandExecutor.for_file(__file__)


def _managed_tool(directory: Path, name: str) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if sys.platform == "win32" else ""
    path = directory / f"{name}{suffix}"
    path.write_bytes(b"tool")
    path.chmod(0o755)
    return path


def _plan(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    host_platform: str,
    host_arch: str = "x86_64",
    profile: str = "release",
    linker: str | None = None,
    bolt_requested: bool = False,
    cc: str = "clang",
    external_target: str | None = None,
    external_inputs: tuple[SourceExtensionLinkInput, ...] = (),
    output_kind: NativeArtifactKind = NativeArtifactKind.ARCHIVE,
    stdlib_path: Path | None = None,
):
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    for path in (output_obj, stub_path, runtime_lib):
        path.write_bytes(b"x")

    if host_platform == "darwin":
        sdk = write_test_darwin_sdk(tmp_path / "SDK")
        monkeypatch.setenv("SDKROOT", str(sdk))
        monkeypatch.delenv("DEVELOPER_DIR", raising=False)
    fake_driver = tmp_path / "clang.exe"
    fake_driver.write_bytes(b"clang")
    monkeypatch.setenv("CC", cc.replace("clang", str(fake_driver), 1))
    monkeypatch.delenv("MOLT_KEEP_SYMBOLS", raising=False)
    monkeypatch.setattr(
        native_link_command,
        "_resolve_native_linker_hint",
        lambda **_kwargs: linker,
    )
    monkeypatch.setattr(
        native_link_command,
        "_collect_cargo_native_link_deps",
        lambda _runtime_lib, **_kwargs: SimpleNamespace(flags=(), verify=lambda: None),
    )
    monkeypatch.setattr(
        native_link_command,
        "_append_darwin_runtime_frameworks",
        lambda _command, *, target: None,
    )
    return native_link_command._build_native_link_plan(
        output_obj=output_obj,
        output_kind=output_kind,
        stdlib_obj_path=stdlib_path,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=tmp_path / "app",
        target=resolve_native_target_spec(
            None, host_platform=host_platform, host_arch=host_arch
        ),
        sysroot_path=None,
        profile=profile,
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        bolt_requested=bolt_requested,
        host_platform=host_platform,
        external_link_requirements=(
            ()
            if external_target is None
            else (
                SourceExtensionLinkRequirements(external_target, items=external_inputs),
            )
        ),
    )


@pytest.mark.parametrize(
    ("host_arch", "explicit_arch", "expected_arch"),
    [
        ("arm64", None, "arm64"),
        ("x86_64", None, "x86_64"),
        ("arm64", "aarch64", "aarch64"),
        ("x86_64", "x86_64", "x86_64"),
    ],
)
def test_darwin_link_plans_use_admitted_target_without_arch_probe(
    monkeypatch, tmp_path, host_arch, explicit_arch, expected_arch
):
    from molt.cli import native_toolchain
    from tests.cli.native_link_test_support import transport_codegen_binding

    def unexpected_process(*args, **kwargs):
        pytest.fail("link planning must not launch an archive architecture probe")

    monkeypatch.setattr(native_toolchain, "_run_completed_command", unexpected_process)
    # The selected target, not the machine running this planning test, owns
    # architecture selection for both application and runtime-image links.
    monkeypatch.setattr(
        platform, "machine", lambda: "x86_64" if host_arch == "arm64" else "arm64"
    )
    monkeypatch.delenv("CFLAGS", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.setenv("MOLT_MACOSX_DEPLOYMENT_TARGET", "11.0")
    if explicit_arch is None:
        monkeypatch.delenv("MOLT_ARCH", raising=False)
    else:
        monkeypatch.setenv("MOLT_ARCH", explicit_arch)
    app = _plan(monkeypatch, tmp_path, host_platform="darwin", host_arch=host_arch)
    binding = transport_codegen_binding(
        tmp_path / "runtime", target_triple=app.target.triple
    )
    runtime = native_link_command._build_native_runtime_image_link_plan(
        runtime_lib=binding.runtime_lib,
        output_image=tmp_path / "runtime.dylib",
        target=app.target,
        sysroot_path=None,
        profile="release",
        runtime_codegen_binding=binding,
    )
    for plan in (app, runtime):
        assert plan.command.count("-arch") == 1
        assert plan.command[plan.command.index("-arch") + 1] == expected_arch
        assert "-target" not in plan.command
        assert plan.normalized_target is None
        assert "-mmacosx-version-min=11.0" in plan.command
    assert "-dynamiclib" in runtime.command
    assert "-dynamiclib" not in app.command


@pytest.mark.parametrize(
    ("host_arch", "override"), [("arm64", "x86_64"), ("x86_64", "arm64")]
)
def test_darwin_arch_override_still_refuses_conflicting_admitted_target(
    monkeypatch, tmp_path, host_arch, override
):
    monkeypatch.setenv("MOLT_ARCH", override)
    monkeypatch.delenv("CFLAGS", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.setenv("MOLT_MACOSX_DEPLOYMENT_TARGET", "11.0")
    with pytest.raises(RuntimeError, match="compiler command -arch .* conflicts"):
        _plan(monkeypatch, tmp_path, host_platform="darwin", host_arch=host_arch)


def test_native_link_sidecars_are_private_for_overlapping_plans(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plain = _plan(monkeypatch, tmp_path, host_platform="linux")
    extension = tmp_path / "extension.a"
    extension.write_bytes(b"archive")
    external = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        # The simulated host is x86_64 Linux (_plan default), not this machine.
        external_target=native_target._host_target_triple(
            host_platform="linux", host_arch="x86_64"
        ),
        external_inputs=(source_extension_link_file(extension),),
    )
    assert plain.sidecars[0].planned_path == external.sidecars[0].planned_path
    assert plain.sidecar_facts() != external.sidecar_facts()
    assert not plain.sidecars[0].planned_path.exists()

    def version_script(command: list[str]) -> Path:
        token = next(
            item for item in command if item.startswith("-Wl,--version-script=")
        )
        return Path(token.split("=", 1)[1])

    with link_pipeline._native_link_execution_command(
        plain,
        planned_output=tmp_path / "app",
        execution_output=tmp_path / "plain-candidate",
    ) as plain_command:
        plain_script = version_script(plain_command)
        assert plain_script.read_bytes() == plain.sidecars[0].content
        with link_pipeline._native_link_execution_command(
            external,
            planned_output=tmp_path / "app",
            execution_output=tmp_path / "external-candidate",
        ) as external_command:
            external_script = version_script(external_command)
            assert external_script != plain_script
            assert external_script.read_bytes() == external.sidecars[0].content
            assert plain_script.read_bytes() == plain.sidecars[0].content
        assert not external_script.exists()
        assert plain_script.read_bytes() == plain.sidecars[0].content
    assert not plain_script.exists()
    assert not plain.sidecars[0].planned_path.exists()


@pytest.mark.parametrize("collision_index", [0, 1])
def test_native_link_sidecar_collision_reaps_only_acquired_stages(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, collision_index: int
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="linux")
    first = plan.sidecars[0]
    second = replace(
        first,
        planned_path=first.planned_path.with_name("second.map"),
        command_index=len(plan.command),
    )
    plan = replace(
        plan,
        command=(*plan.command, f"{second.operand_prefix}{second.planned_path}"),
        sidecars=(first, second),
    )
    stages = {
        sidecar.planned_path: file_publication.staged_file_path(
            sidecar.planned_path,
            purpose="native-link",
            suffix=sidecar.planned_path.suffix,
        )
        for sidecar in plan.sidecars
    }
    collision = stages[plan.sidecars[collision_index].planned_path]
    collision.write_bytes(b"another owner's stage")
    before = collision.stat()
    monkeypatch.setattr(
        file_publication, "staged_file_path", lambda path, **kwargs: stages[path]
    )

    with pytest.raises(FileExistsError):
        with native_link_plan.native_link_execution_command(
            plan,
            planned_output=tmp_path / "app",
            execution_output=tmp_path / "candidate",
        ):
            pytest.fail("a colliding private sidecar was admitted")

    assert collision.read_bytes() == b"another owner's stage"
    assert os.path.samestat(before, collision.stat())
    assert not stages[plan.sidecars[1 - collision_index].planned_path].exists()
    assert all(not sidecar.planned_path.exists() for sidecar in plan.sidecars)


def test_native_link_sidecar_retargets_only_its_typed_operand(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="linux")
    sidecar = plan.sidecars[0]
    unrelated = f"-Wl,--user-note={sidecar.planned_path}.unrelated"
    plan = replace(plan, command=(*plan.command, unrelated))
    with native_link_plan.native_link_execution_command(
        plan,
        planned_output=tmp_path / "app",
        execution_output=tmp_path / "candidate",
    ) as command:
        assert command[-1] == unrelated
        assert command[sidecar.command_index] != plan.command[sidecar.command_index]
        assert sidecar.planned_path.exists() is False
    tampered = replace(
        plan,
        sidecars=(replace(sidecar, command_index=len(plan.command) - 1),),
    )
    with pytest.raises(RuntimeError, match="sidecar operand mismatch"):
        with native_link_plan.native_link_execution_command(
            tampered,
            planned_output=tmp_path / "app",
            execution_output=tmp_path / "candidate",
        ):
            pass


@pytest.mark.parametrize("host_platform", ["linux", "darwin", "win32"])
def test_compiler_archives_are_wholly_loaded_without_changing_runtime_policy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, host_platform: str
) -> None:
    stdlib = tmp_path / "stdlib, with spaces.unrelated"
    stdlib.write_bytes(b"archive")
    plan = _plan(monkeypatch, tmp_path, host_platform=host_platform, stdlib_path=stdlib)
    for path in (tmp_path / "output.o", stdlib):
        arguments = native_link_plan.native_artifact_link_arguments(
            path, kind=NativeArtifactKind.ARCHIVE, target=plan.target
        )
        assert any(
            plan.command[index : index + len(arguments)] == arguments
            for index in range(len(plan.command))
        )
    runtime = str(tmp_path / "libmolt_runtime.a")
    assert not any(runtime in arg and "WHOLEARCHIVE" in arg for arg in plan.command)
    if host_platform == "linux":
        assert plan.command.index("--no-whole-archive") < plan.command.index(runtime)


@pytest.mark.parametrize("host_platform", ["linux", "darwin", "win32"])
def test_object_link_input_is_never_selected_by_filename_suffix(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, host_platform: str
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform=host_platform,
        output_kind=NativeArtifactKind.OBJECT,
    )
    assert str(tmp_path / "output.o") in plan.command
    assert not any(
        "whole-archive" in arg or "WHOLEARCHIVE" in arg for arg in plan.command
    )
    assert "-force_load" not in plan.command


def test_missing_shared_stdlib_fails_before_link(monkeypatch, tmp_path) -> None:
    with pytest.raises(RuntimeError, match="Shared stdlib artifact is unavailable"):
        _plan(
            monkeypatch,
            tmp_path,
            host_platform="linux",
            stdlib_path=tmp_path / "missing.a",
        )


@pytest.mark.slow
def test_real_elf_extension_link_preserves_eager_members_lazy_dependencies_and_runtime_edges(
    monkeypatch,
    tmp_path,
) -> None:
    tools = verified_llvm_tools()
    target = tools.native_target("elf")
    cc, ar, nm, linker = tools.clang, tools.ar, tools.nm, tools.linker("ld.lld")

    def run(command):
        result = _COMMANDS.run(
            command,
            env=dict(tools.environment),
            capture_output=True,
            text=True,
            timeout=30,
            encoding="utf-8",
        )
        assert result.returncode == 0, result.stderr
        return result.stdout

    def archive(name, sources):
        objects = []
        for index, content in enumerate(sources):
            source = tmp_path / f"{name}-{index}.c"
            source.write_text(content, encoding="utf-8")
            obj = source.with_suffix(".o")
            run([cc, f"--target={target}", "-c", str(source), "-o", str(obj)])
            objects.append(str(obj))
        path = tmp_path / f"lib{name}.a"
        run([ar, "rcsD", str(path), *objects])
        return path

    primary = archive(
        "extension",
        (
            "extern int dependency(void); int PyInit_demo(void) { return dependency(); }",
            "extern void eager_hook(void); __attribute__((constructor)) "
            "void eager_ctor(void) { eager_hook(); }",
        ),
    )
    dependency = archive(
        "dependency",
        (
            "extern int runtime_value(void); int dependency(void) { return runtime_value(); }",
            "extern int must_not_link(void); int dormant(void) { return must_not_link(); }",
        ),
    )
    runtime = archive(
        "runtime",
        (
            "int runtime_value(void) { return 42; } void eager_hook(void) {} "
            "int Py_None, Py_NotImplementedSentinel, Py_EllipsisObject;",
        ),
    )
    app = archive("app", ())
    # This freestanding object-format proof has no host sysroot/CRT dependency.
    # Empty language/math archives satisfy the production driver's search flags.
    archive("stdc++", ())
    archive("m", ())
    stub = tmp_path / "entry.c"
    stub.write_text(
        "extern int PyInit_demo(void); int main(void) { return PyInit_demo(); }",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (
            [
                cc,
                f"--target={target}",
                f"-fuse-ld={linker}",
                "-nostdlib",
                f"-L{tmp_path}",
                "-Wl,-e,main",
            ],
            "lld",
            target,
        ),
    )
    monkeypatch.setattr(
        native_link_command,
        "_collect_cargo_native_link_deps",
        lambda *args, **kwargs: SimpleNamespace(flags=(), verify=lambda: None),
    )
    plan = native_link_command._build_native_link_plan(
        output_obj=app,
        stub_path=stub,
        runtime_lib=runtime,
        output_binary=tmp_path / "app.elf",
        target=resolve_native_target_spec(target),
        sysroot_path=None,
        profile="dev",
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        external_link_requirements=(
            SourceExtensionLinkRequirements(
                target,
                (
                    source_extension_link_file(
                        primary, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                    ),
                    source_extension_link_file(dependency),
                ),
            ),
        ),
    )
    output = tmp_path / "app.elf"
    candidate = file_publication.staged_file_path(
        output, purpose="native-link", suffix=output.suffix
    )
    from molt.cli.link_selection_admission import native_link_selection
    from molt.link_outputs import link_selection_path

    selection_output = link_selection_path(output)
    selection_candidate = file_publication.staged_file_path(
        selection_output, purpose="native-link"
    )
    with native_link_selection(plan, candidate) as selection:
        assert selection is not None
        with native_link_plan.native_link_execution_command(
            plan,
            planned_output=output,
            execution_output=candidate,
            selection_arguments=selection.arguments,
        ) as command:
            trace = run(command)
        selection.admit(stdout=trace, stderr="", output=selection_candidate)
    symbols = {
        line.split()[0]
        for line in run(
            [nm, "--format=posix", "--defined-only", str(candidate)]
        ).splitlines()
        if line.strip()
    }
    assert {"eager_ctor", "dependency", "runtime_value"} <= symbols
    assert "dormant" not in symbols
    fingerprint = link_fingerprints._link_fingerprint(
        project_root=tmp_path,
        inputs=[app, primary, dependency, runtime, stub],
        link_cmd=list(plan.command),
        tool_facts=plan.sidecar_facts(),
    )
    assert fingerprint is not None
    receipt_path = tmp_path / "app.elf.fingerprint"
    monkeypatch.delenv("MOLT_SKIP_BINARY_VALIDITY_CHECK", raising=False)
    monkeypatch.delenv("MOLT_BUILD_SMOKE_EXEC", raising=False)
    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple=target,
            strip=False,
            link_selection=(selection_candidate, selection_output),
            receipt=link_fingerprints.FinalLinkReceiptRequest.from_fingerprint(
                receipt_path, fingerprint
            ),
        )
        is None
    )
    assert not candidate.exists()
    assert link_fingerprints._link_outputs_match(
        outputs={"binary": output, "selection": selection_output},
        fingerprint=fingerprint,
        receipt_path=receipt_path,
    )
    selection_output.write_bytes(b"tampered selection")
    assert not link_fingerprints._link_outputs_match(
        outputs={"binary": output, "selection": selection_output},
        fingerprint=fingerprint,
        receipt_path=receipt_path,
    )


def test_link_plan_is_immutable_and_preserves_elf_function_identity(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        linker="lld",
    )

    assert plan.target.object_format is NativeObjectFormat.ELF
    assert plan.policy.preserve_function_identity
    assert "-Wl,--icf=none" in plan.command
    assert "-Wl,--strip-all" in plan.command
    assert "-Wl,/Brepro" not in plan.command
    assert plan.command.count(str(tmp_path / "libmolt_runtime.a")) == 1
    assert plan.policy.symbol_strip is native_link_plan.NativeSymbolStrip.AT_LINK
    with pytest.raises(FrozenInstanceError):
        plan.linker_hint = None  # type: ignore[misc]


@pytest.mark.parametrize(
    "foreign_target", ["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl"]
)
def test_host_native_link_rejects_foreign_architecture_or_abi(
    monkeypatch, tmp_path, foreign_target
):
    with pytest.raises(RuntimeError, match="cross target triples"):
        _plan(
            monkeypatch, tmp_path, host_platform="linux", external_target=foreign_target
        )


@pytest.mark.parametrize(
    "flags",
    [
        "--target=aarch64-unknown-linux-gnu",
        "-target x86_64-unknown-linux-musl",
        "-m32",
        "-mx32",
    ],
)
def test_native_link_rejects_cflags_target_drift(monkeypatch, tmp_path, flags):
    monkeypatch.setenv("CFLAGS", flags)
    with pytest.raises(RuntimeError, match="target custody"):
        _plan(monkeypatch, tmp_path, host_platform="linux")


def test_native_link_rejects_cc_target_drift(monkeypatch, tmp_path):
    with pytest.raises(RuntimeError, match="target custody"):
        _plan(
            monkeypatch,
            tmp_path,
            host_platform="linux",
            cc="clang --target=aarch64-unknown-linux-gnu",
        )


def test_native_link_preserves_matching_target_flags(monkeypatch, tmp_path):
    monkeypatch.setenv("CFLAGS", "-m64")
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        cc="clang --target=x86_64-unknown-linux-gnu",
    )
    assert "--target=x86_64-unknown-linux-gnu" in plan.command
    assert "-m64" in plan.command


@pytest.mark.parametrize(
    ("host_platform", "target", "user_flag", "identity_flag"),
    [
        ("linux", "x86_64-unknown-linux-gnu", "-Wl,--icf=all", "-Wl,--icf=none"),
        ("darwin", "x86_64-apple-darwin", "-Wl,-deduplicate", "-Wl,-no_deduplicate"),
        ("win32", "x86_64-pc-windows-msvc", "-Wl,/OPT:ICF", "-Wl,/OPT:NOICF"),
    ],
)
def test_extension_link_applies_identity_policy_after_user_link_arguments(
    monkeypatch, tmp_path, host_platform, target, user_flag, identity_flag
):
    extension = tmp_path / "extension.a"
    extension.write_bytes(b"extension archive")
    monkeypatch.setenv("CFLAGS", user_flag)
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform=host_platform,
        linker="lld",
        external_target=target,
        external_inputs=(source_extension_link_file(extension),),
    )
    assert plan.target.arch == "x86_64"
    assert (
        plan.command.index(user_flag)
        < plan.command.index(str(extension))
        < plan.command.index(str(tmp_path / "libmolt_runtime.a"))
        < plan.command.index(identity_flag)
    )


def test_macho_plan_preserves_identity_without_suppressing_warnings(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="darwin")

    assert "-Wl,-no_deduplicate" in plan.command
    assert "-Wl,-w" not in plan.command
    # A release image is stripped by ld64 itself, so its signature stands.
    assert plan.policy.symbol_strip is native_link_plan.NativeSymbolStrip.AT_LINK
    assert "-Wl,-x" in plan.command
    assert "-Wl,-S" in plan.command
    assert "-Wl,/Brepro" not in plan.command
    # ld64 archive extraction is order-sensitive; the deliberate second pass
    # is confined to Mach-O rather than leaking into COFF.
    assert plan.command.count(str(tmp_path / "libmolt_runtime.a")) == 2


def test_explicit_driver_linker_selection_gets_matching_capability_policy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        cc="clang -fuse-ld=/opt/llvm/bin/ld.lld",
    )

    assert plan.linker_hint == "lld"
    assert "-Wl,--icf=none" in plan.command


def test_fast_linker_auto_detection_does_not_leak_across_target_formats(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_DEV_LINKER", raising=False)
    monkeypatch.setattr(
        native_link_command,
        "_resolve_available_fast_linker",
        lambda *_args, **_kwargs: "mold",
    )
    assert (
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target=resolve_native_target_spec(None, host_platform="darwin"),
            host_platform="darwin",
        )
        is None
    )


def test_windows_native_link_selects_available_lld_explicitly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_DEV_LINKER", raising=False)
    monkeypatch.setattr(
        native_link_command,
        "_resolve_available_fast_linker",
        lambda *_args, **_kwargs: "lld",
    )
    assert (
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target=resolve_native_target_spec(None, host_platform="win32"),
            host_platform="win32",
        )
        == "lld"
    )
    assert (
        native_link_command._resolve_native_linker_hint(
            profile="release",
            target=resolve_native_target_spec(None, host_platform="win32"),
            host_platform="win32",
        )
        == "lld"
    )


@pytest.mark.parametrize(
    ("host_platform", "expected_role"),
    (("linux", "ld.lld"), ("darwin", "ld64.lld"), ("win32", "lld-link")),
)
def test_explicit_lld_selection_requests_the_target_specific_role(
    host_platform: str,
    expected_role: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: list[str] = []
    monkeypatch.setenv("MOLT_DEV_LINKER", "lld")

    def linker_candidates(role: str, **_kwargs: object) -> tuple[Path, ...]:
        seen.append(role)
        return (Path("/llvm/bin") / role,)

    monkeypatch.setattr(
        native_link_command, "llvm_linker_candidates", linker_candidates
    )

    assert (
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target=resolve_native_target_spec(None, host_platform=host_platform),
            host_platform=host_platform,
        )
        == "lld"
    )
    assert seen == [expected_role]


def test_explicit_generic_lld_path_cannot_satisfy_elf_role(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    generic = _managed_tool(tmp_path / "llvm" / "bin", "lld")

    with pytest.raises(RuntimeError, match="generic lld driver"):
        _plan(
            monkeypatch,
            tmp_path,
            host_platform="linux",
            cc=f"clang -fuse-ld={generic}",
        )


def test_native_driver_and_linker_prefer_one_managed_llvm_family(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    managed_bin = tmp_path / "target" / "toolchains" / "llvm-99" / "bin"
    clang = _managed_tool(managed_bin, "clang")
    _managed_tool(managed_bin, "lld-link")
    sdk_bin = tmp_path / "target" / "toolchains" / "wasi-sdk" / "bin"
    _managed_tool(sdk_bin, "clang")
    _managed_tool(sdk_bin, "lld-link")
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "target"))
    monkeypatch.delenv("CC", raising=False)
    monkeypatch.delenv("MOLT_DEV_LINKER", raising=False)

    command, linker_hint, _target = (
        native_link_command._build_native_link_driver_command(
            target=resolve_native_target_spec(
                None, host_platform="win32", host_arch="AMD64"
            ),
            sysroot_path=None,
            profile="dev",
            host_platform="win32",
        )
    )

    assert Path(command[0]) == clang.resolve()
    assert command.count("-fuse-ld=lld") == 1
    assert linker_hint == "lld"


def test_explicit_cc_overrides_managed_driver(tmp_path: Path, monkeypatch) -> None:
    managed_bin = tmp_path / "target" / "toolchains" / "llvm-99" / "bin"
    _managed_tool(managed_bin, "clang")
    explicit = _managed_tool(tmp_path / "explicit" / "bin", "clang")
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "target"))
    monkeypatch.setenv("CC", str(explicit))
    monkeypatch.setenv("MOLT_DEV_LINKER", "off")

    command, linker_hint, _target = (
        native_link_command._build_native_link_driver_command(
            target=resolve_native_target_spec(None),
            sysroot_path=None,
            profile="dev",
        )
    )

    # The explicit CC is the driver. On a macOS host the driver also receives
    # -arch, the deployment target and the SDK, whichever driver it is.
    assert command[0] == str(explicit.resolve())
    assert not any(str(managed_bin) in token for token in command)
    if sys.platform != "darwin":
        assert command == [str(explicit.resolve())]
    assert linker_hint is None


def test_explicit_mold_non_elf_selection_fails_before_link(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_DEV_LINKER", "mold")

    with pytest.raises(RuntimeError, match="Linux ELF"):
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target=resolve_native_target_spec(None, host_platform="darwin"),
            host_platform="darwin",
        )


def test_coff_plan_explicitly_disables_icf(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="win32", host_arch="AMD64")

    assert "-Wl,/OPT:REF" in plan.command
    assert "-Wl,/OPT:NOICF" in plan.command
    assert "-Wl,/Brepro" in plan.command
    assert plan.command.count(str(tmp_path / "libmolt_runtime.a")) == 1
    assert plan.policy.symbol_strip is native_link_plan.NativeSymbolStrip.NONE


def test_bolt_release_plan_emits_relocations_and_defers_stripping(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        linker="mold",
        bolt_requested=True,
    )

    assert "-Wl,--emit-relocs" in plan.command
    assert "-Wl,--strip-all" not in plan.command
    assert plan.policy.symbol_strip is native_link_plan.NativeSymbolStrip.NONE
    assert plan.policy.bolt_requested


@pytest.mark.parametrize(
    ("host_platform", "host_arch", "profile", "message"),
    [
        ("win32", "AMD64", "release", "Linux ELF"),
        ("darwin", "arm64", "release", "Linux ELF"),
        ("linux", "riscv64", "release", "x86_64 and aarch64"),
        ("linux", "x86_64", "dev", "release build profile"),
    ],
)
def test_bolt_unsupported_cells_fail_during_link_planning(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    host_platform: str,
    host_arch: str,
    profile: str,
    message: str,
) -> None:
    with pytest.raises(RuntimeError, match=message):
        _plan(
            monkeypatch,
            tmp_path,
            host_platform=host_platform,
            host_arch=host_arch,
            profile=profile,
            bolt_requested=True,
        )


def test_cli_threads_bolt_intent_into_the_single_build_pipeline(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    received: list[BuildRequest] = []

    def fake_build(request: BuildRequest) -> int:
        received.append(request)
        return 0

    monkeypatch.setattr(cli_build_command, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "molt",
            "build",
            "--bolt",
            "--bolt-training-cmd",
            "{binary} --train",
            str(entry),
        ],
    )

    assert cli.main() == 0
    [request] = received
    assert request.bolt is True
    assert request.bolt_training_cmd == "{binary} --train"


def test_planned_release_strip_failure_is_loud(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    install_module_view(monkeypatch, "sys", sys, build_results, platform="linux")
    install_module_view(
        monkeypatch, "platform", platform, build_results, machine=lambda: "x86_64"
    )
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("/usr/bin/strip"),),
    )
    monkeypatch.setattr(
        build_results,
        "_run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            ["strip"], 1, "", "unsupported relocation"
        ),
    )

    assert build_results._post_link_strip(binary, None) == (
        "post-link strip failed: unsupported relocation"
    )


def test_cross_target_strip_requires_target_capable_llvm_strip(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    install_module_view(monkeypatch, "sys", sys, build_results, platform="win32")
    install_module_view(
        monkeypatch, "platform", platform, build_results, machine=lambda: "AMD64"
    )
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("C:/Windows/System32/strip.exe"),),
    )

    assert build_results._post_link_strip(binary, "aarch64-unknown-linux-gnu") == (
        "post-link target-capable llvm-strip is unavailable for linux/aarch64"
    )


def test_cross_target_strip_uses_llvm_strip_and_target_format_flags(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    llvm_strip = tmp_path / "llvm-strip.exe"
    llvm_strip.write_bytes(b"tool")
    received: list[list[str]] = []
    install_module_view(monkeypatch, "sys", sys, build_results, platform="win32")
    install_module_view(
        monkeypatch, "platform", platform, build_results, machine=lambda: "AMD64"
    )
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("C:/Windows/System32/strip.exe"), llvm_strip),
    )

    def fake_run(command, **_kwargs):
        received.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(build_results, "_run_completed_command", fake_run)

    assert build_results._post_link_strip(binary, "aarch64-unknown-linux-gnu") is None
    assert received == [[str(llvm_strip), "--strip-all", str(binary)]]


@pytest.mark.parametrize("placement", ["linker-stage", "sibling", "elsewhere"])
def test_native_candidate_in_the_output_directory_is_published_without_a_copy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, placement: str
) -> None:
    from molt import file_publication

    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    if placement == "linker-stage":
        candidate = file_publication.staged_file_path(output, purpose="native-link")
    elif placement == "sibling":
        # BOLT writes `<binary>.bolt` beside its input.
        candidate = output.parent / "app.bolt"
    else:
        candidate = tmp_path / "elsewhere" / "app"
        candidate.parent.mkdir()
    program = bytes(elf_header(kind=2))
    candidate.write_bytes(program)
    linked_inode = candidate.stat().st_ino
    signed: list[Path] = []
    monkeypatch.setattr(
        atomic_io, "_codesign_atomic_copy_temp", lambda path, _dst: signed.append(path)
    )

    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple="x86_64-unknown-linux-gnu",
            strip=False,
        )
        is None
    )
    assert output.read_bytes() == program
    assert not candidate.exists() or placement == "elsewhere"
    # A stage in the output's directory is renamed into place: the published
    # binary is the inode the linker wrote. A candidate elsewhere is copied.
    assert (output.stat().st_ino == linked_inode) is (placement != "elsewhere")
    # Only a Mach-O image carries a code signature, whatever the host.
    assert signed == []


@pytest.mark.parametrize(
    ("placement", "identifier", "strip", "re_signed"),
    [
        # The actual linker signature names the published binary: keep it.
        ("linker-stage", "app", False, False),
        # Some Apple ld versions, ld64.lld and zig use the private stage name.
        ("linker-stage", "stage", False, True),
        # x86_64 ld64 without -adhoc_codesign leaves the image unsigned.
        ("linker-stage", None, False, True),
        # Strip rewrote the image after the link.
        ("linker-stage", "app", True, True),
        # Another tool wrote this image; its signature is not the linker's.
        ("sibling", "app", False, True),
    ],
)
def test_macho_stage_keeps_only_a_linker_signature_naming_the_published_binary(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    placement: str,
    identifier: str | None,
    strip: bool,
    re_signed: bool,
) -> None:
    from molt import file_publication

    output = tmp_path / "app"
    candidate = (
        file_publication.staged_file_path(output, purpose="native-link")
        if placement == "linker-stage"
        else tmp_path / "app.bolt"
    )
    if identifier == "stage":
        identifier = candidate.name
    program = bytes(signed_macho_image(identifier))
    candidate.write_bytes(program)
    signed: list[Path] = []

    def sign(stage: Path, destination: Path) -> None:
        assert destination == output and stage.parent == output.parent
        signed.append(stage)

    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", sign)
    monkeypatch.setattr(build_results, "_post_link_strip", lambda *_: None)

    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple="aarch64-apple-darwin",
            strip=strip,
        )
        is None
    )
    assert output.read_bytes() == program
    assert not candidate.exists()
    assert (len(signed) == 1) is re_signed


def test_native_candidate_is_finalized_before_atomic_publication(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    candidate = tmp_path / ".app.link-candidate"
    output = tmp_path / "app"
    candidate.write_bytes(b"linked")
    output.write_bytes(b"previous")
    events: list[str] = []

    def fake_strip(path: Path, _target: str | None) -> None:
        assert path == candidate
        events.append("strip")
        path.write_bytes(b"stripped")
        return None

    def fake_validate(path: Path, _target: str | None) -> None:
        assert path.read_bytes() == b"stripped"
        events.append("validate")

    def fake_sign(path: Path, destination: Path) -> None:
        assert destination == output
        assert path.read_bytes() == b"stripped"
        assert output.read_bytes() == b"previous"
        events.append("sign")

    monkeypatch.setattr(build_results, "_post_link_strip", fake_strip)
    monkeypatch.setattr(build_results, "_assert_native_binary_valid", fake_validate)
    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", fake_sign)
    phase_times: dict[str, int] = {}

    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple="aarch64-apple-darwin",
            strip=True,
            phase_times=phase_times,
        )
        is None
    )
    assert events == ["strip", "sign", "validate"]
    assert output.read_bytes() == b"stripped"
    assert not candidate.exists()
    assert set(phase_times) == {
        "strip_wall_ns",
        "validate_wall_ns",
        "publish_wall_ns",
        "cleanup_wall_ns",
    }
    assert all(value >= 0 for value in phase_times.values())


def test_native_candidate_failure_preserves_previous_published_artifact(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    candidate = tmp_path / ".app.link-candidate"
    output = tmp_path / "app"
    candidate.write_bytes(b"linked")
    output.write_bytes(b"previous")
    monkeypatch.setattr(
        build_results,
        "_post_link_strip",
        lambda *_args, **_kwargs: "unsupported target relocation",
    )

    phase_times: dict[str, int] = {}
    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple="aarch64-unknown-linux-gnu",
            strip=True,
            phase_times=phase_times,
        )
        == "unsupported target relocation"
    )
    assert set(phase_times) == {"strip_wall_ns", "cleanup_wall_ns"}
    assert output.read_bytes() == b"previous"
    assert not candidate.exists()


@pytest.mark.parametrize(
    ("host_platform", "cc", "expected"),
    [
        (
            "win32",
            ("clang-cl",),
            ("/link", "/Brepro", "/OPT:REF", "/OPT:NOICF"),
        ),
        (
            "linux",
            ("clang", "-fuse-ld=lld"),
            ("-Wl,--gc-sections", "-Wl,--icf=none"),
        ),
        (
            "darwin",
            ("clang",),
            ("-Wl,-dead_strip", "-Wl,-no_deduplicate"),
        ),
        # zig's own linker runs no identical-code folding and rejects ld64's
        # and LLD's flags for disabling it.
        ("darwin", ("zig", "cc"), ("-Wl,-dead_strip",)),
        ("linux", ("zig", "cc"), ("-Wl,--gc-sections",)),
    ],
)
def test_native_identity_flags_are_driver_ready(
    host_platform: str,
    cc: tuple[str, ...],
    expected: tuple[str, ...],
) -> None:
    target = native_target.resolve_native_target_spec(
        None, host_platform=host_platform, host_arch="x86_64"
    )
    capabilities = native_link_plan.native_link_capabilities(
        target=target,
        linker_hint=native_link_plan.native_linker_name_from_driver_command(cc),
    )
    assert (
        native_link_plan.native_link_policy_flags(
            target=target, capabilities=capabilities, msvc_driver=cc[0] == "clang-cl"
        )
        == expected
    )


_AT_LINK = native_link_plan.NativeSymbolStrip.AT_LINK
_AFTER_LINK = native_link_plan.NativeSymbolStrip.AFTER_LINK
_NO_STRIP = native_link_plan.NativeSymbolStrip.NONE


@pytest.mark.parametrize(
    ("host_platform", "cc", "profile", "keep_symbols", "bolt", "expected"),
    [
        ("darwin", ("clang",), "release", False, False, _AT_LINK),
        ("darwin", ("clang", "-fuse-ld=lld"), "release", False, False, _AT_LINK),
        # zig's Mach-O linker keeps the local thunks it synthesizes under -x.
        ("darwin", ("zig", "cc"), "release", False, False, _AFTER_LINK),
        ("linux", ("clang",), "release", False, False, _AT_LINK),
        ("linux", ("clang", "-fuse-ld=mold"), "release", False, False, _AT_LINK),
        ("linux", ("zig", "cc"), "release", False, False, _AT_LINK),
        ("linux", ("clang",), "release", False, True, _NO_STRIP),
        ("darwin", ("clang",), "dev", False, False, _NO_STRIP),
        ("darwin", ("clang",), "release", True, False, _NO_STRIP),
        ("win32", ("clang-cl",), "release", False, False, _NO_STRIP),
    ],
)
def test_release_symbol_strip_stage_follows_the_linker_capability(
    host_platform: str,
    cc: tuple[str, ...],
    profile: str,
    keep_symbols: bool,
    bolt: bool,
    expected: native_link_plan.NativeSymbolStrip,
) -> None:
    target = native_target.resolve_native_target_spec(
        None, host_platform=host_platform, host_arch="x86_64"
    )
    capabilities = native_link_plan.native_link_capabilities(
        target=target,
        linker_hint=native_link_plan.native_linker_name_from_driver_command(cc),
    )
    policy = native_link_plan.native_link_policy(
        target=target,
        capabilities=capabilities,
        profile=profile,
        keep_symbols=keep_symbols,
        bolt_requested=bolt,
    )
    assert policy.symbol_strip is expected


def test_link_time_strip_flags_match_the_post_link_strip_per_format() -> None:
    def spec(host_platform: str):
        return native_target.resolve_native_target_spec(
            None, host_platform=host_platform, host_arch="x86_64"
        )

    # ld64 -x drops what strip -x drops except the debug-map stabs; -S drops those.
    assert native_link_plan.native_link_strip_flags(spec("darwin")) == (
        "-Wl,-x",
        "-Wl,-S",
    )
    assert native_link_plan.native_strip_flags(spec("darwin")) == ("-x",)
    assert native_link_plan.native_link_strip_flags(spec("linux")) == (
        "-Wl,--strip-all",
    )
    assert native_link_plan.native_strip_flags(spec("linux")) == ("--strip-all",)
    with pytest.raises(RuntimeError, match="Link-time stripping has no policy"):
        native_link_plan.native_link_strip_flags(spec("win32"))


@pytest.mark.parametrize(
    ("zig_target", "host_platform", "strips"),
    [
        ("x86_64-linux-gnu", "linux", True),
        ("aarch64-macos", "darwin", False),
    ],
)
def test_zig_linker_strip_capability_matches_its_output(
    tmp_path: Path, zig_target: str, host_platform: str, strips: bool
) -> None:
    import shutil

    from molt.llvm_wasi_tools import llvm_named_tool_candidates
    from tests.cli.process_guard import run_cli_test_process

    zig = shutil.which("zig")
    nm = llvm_named_tool_candidates("llvm-nm")
    if zig is None or not nm:
        pytest.skip("zig and llvm-nm are the oracle for zig's own linker")
    target = native_target.resolve_native_target_spec(
        None, host_platform=host_platform, host_arch="x86_64"
    )
    capabilities = native_link_plan.native_link_capabilities(
        target=target,
        linker_hint=native_link_plan.native_linker_name_from_driver_command(
            (zig, "cc")
        ),
    )
    assert capabilities.strips_local_symbols is strips
    source = tmp_path / "main.c"
    source.write_text(
        "static int helper(int x) { return x * 3; }\n"
        "int visible(int x) { return helper(x) + 1; }\n"
        "int main(int argc, char **argv) { return visible(argc); }\n",
        encoding="utf-8",
    )
    image = tmp_path / "app"
    run_cli_test_process(
        [
            zig,
            "cc",
            "-target",
            zig_target,
            str(source),
            "-o",
            str(image),
            *native_link_plan.native_link_strip_flags(target),
        ],
        check=True,
        timeout=300,
    )
    listing = run_cli_test_process(
        [str(nm[0]), "--defined-only", str(image)], check=False, timeout=30
    ).stdout
    # nm prints "<address> <type> <name>"; a lower-case type is a local symbol.
    local = [
        line
        for line in listing.splitlines()
        if len(line.split()) == 3 and line.split()[1].islower()
    ]
    assert (not local) is strips, local[:5]


@pytest.mark.parametrize(
    "host_platform,host_arch",
    [("linux", "x86_64"), ("win32", "AMD64"), ("darwin", "arm64")],
)
def test_host_spec_keeps_driver_and_runtime_selection_native(
    tmp_path, monkeypatch, host_platform, host_arch
):
    from molt.cli.native_link_manifest import read_native_link_flags
    from tests.cli.native_link_test_support import (
        write_test_native_link_manifest,
        write_test_static_archive,
    )
    from tests.runtime_build_identity_helper import native_runtime_staticlib_identity

    target = resolve_native_target_spec(
        None, host_platform=host_platform, host_arch=host_arch
    )
    driver = _managed_tool(tmp_path / "driver", "clang")
    monkeypatch.setenv("CC", str(driver))
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    monkeypatch.delenv("CFLAGS", raising=False)
    monkeypatch.delenv("MOLT_ARCH", raising=False)
    monkeypatch.setenv("MOLT_DEV_LINKER", "off")
    monkeypatch.setattr(
        native_link_command, "llvm_named_tool_candidates", lambda *a, **k: ()
    )
    monkeypatch.setattr(
        native_link_command, "validate_compiler_target", lambda *a, **k: None
    )
    sdk = write_test_darwin_sdk(tmp_path / "SDK")
    monkeypatch.setenv("SDKROOT", str(sdk))
    monkeypatch.setenv("MOLT_MACOSX_DEPLOYMENT_TARGET", "11.0")
    monkeypatch.delenv("DEVELOPER_DIR", raising=False)
    runtime = tmp_path / "dev-fast" / "runtime.a"
    runtime.parent.mkdir()
    write_test_static_archive(runtime)
    identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=None,
        host_target=target.triple,
        family_seed="host-target-selection",
    )
    write_test_native_link_manifest(
        runtime, build_identity=identity, target_triple=None
    )
    command, _, normalized = native_link_command._build_native_link_driver_command(
        target=target,
        sysroot_path=None,
        profile="dev",
        host_platform=host_platform,
    )
    assert Path(command[0]) == driver.resolve()
    assert "-target" not in command
    assert normalized is None
    if host_platform == "darwin":
        assert "-arch" in command
        assert "-mmacosx-version-min=11.0" in command
        # A managed LLVM clang has no implicit macOS SDK; the driver command
        # names the active SDK so <stdio.h> resolves for the main stub.
        assert command[command.index("-isysroot") + 1] == str(sdk)
    else:
        assert "-isysroot" not in command
    inputs = read_native_link_flags(
        runtime,
        target_triple=target.cargo_target,
        object_format=target.object_format.value,
        runtime_build_identity=identity,
    )
    assert "-lc" in inputs.flags
    with pytest.raises(RuntimeError, match="target mismatch"):
        read_native_link_flags(
            runtime,
            target_triple=target.triple,
            object_format=target.object_format.value,
            runtime_build_identity=identity,
        )


def test_linux_host_triple_projects_musl_abi(monkeypatch):
    import sysconfig

    monkeypatch.setattr(sysconfig, "get_config_var", lambda name: "x86_64-linux-musl")
    target = resolve_native_target_spec(None, host_platform="linux", host_arch="x86_64")
    assert target.triple == "x86_64-unknown-linux-musl"
    assert target.cargo_target is None


@pytest.mark.parametrize(
    "corruption", ["hash-offset", "hash-width", "hash-kind", "absent-hashes"]
)
def test_malformed_linker_signature_cannot_publish_or_invoke_signing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, corruption: str
) -> None:
    import struct

    output = tmp_path / "app"
    output.write_bytes(b"previously published")
    candidate = file_publication.staged_file_path(output, purpose="native-link")
    image = signed_macho_image("app")
    if corruption == "hash-offset":
        struct.pack_into(">I", image, 8212 + 16, 0xFFFFFFFF)
    elif corruption == "hash-width":
        image[8212 + 36] = 0
    elif corruption == "hash-kind":
        image[8212 + 37] = 255
    else:
        del image[8212 + 92 :]
        struct.pack_into(">I", image, 8212 + 4, 92)
        struct.pack_into(">I", image, 8192 + 4, 112)
        struct.pack_into("<I", image, 44, 112)
    candidate.write_bytes(image)

    def no_sign(*_args):
        raise AssertionError("malformed signatures must fail closed before signing")

    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", no_sign)
    result = build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="aarch64-apple-darwin",
        strip=False,
    )
    assert result is not None and "atomic native publication failed" in result
    assert output.read_bytes() == b"previously published"
    assert not candidate.exists()
