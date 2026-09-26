from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
from pathlib import Path
import subprocess
import sys

import pytest

import molt.cli as cli
from molt.cli import build_results, link_pipeline, native_link_command, native_link_plan
from molt.cli import atomic_io
from molt.cli import link_fingerprints
from molt import file_publication
from molt.cli.native_link_plan import NativeArtifactKind, NativeObjectFormat
from tests.cli.native_link_test_support import RUNTIME_BUILD_IDENTITY
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkInput,
    SourceExtensionLinkRequirements,
    SourceExtensionLinkLoadingPolicy,
    source_extension_link_file,
)


def _managed_tool(directory: Path, name: str) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if sys.platform == "win32" else ""
    path = directory / f"{name}{suffix}"
    path.write_bytes(b"tool")
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
        lambda _runtime_lib, **_kwargs: [],
    )
    monkeypatch.setattr(
        native_link_command,
        "_append_darwin_runtime_frameworks",
        lambda _command, *, target_triple: None,
    )
    return native_link_command._build_native_link_plan(
        output_obj=output_obj,
        output_kind=output_kind,
        stdlib_obj_path=stdlib_path,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=tmp_path / "app",
        target_triple=None,
        sysroot_path=None,
        profile=profile,
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        bolt_requested=bolt_requested,
        host_platform=host_platform,
        host_arch=host_arch,
        external_link_requirements=(
            ()
            if external_target is None
            else (
                SourceExtensionLinkRequirements(external_target, items=external_inputs),
            )
        ),
    )


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
        external_target=native_link_plan._host_target_triple(host_platform="linux"),
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
    from molt.cli.llvm_wasi_tools import llvm_tool_candidates, llvm_linker_candidates

    target = "x86_64-unknown-linux-gnu"
    candidates = {kind: llvm_tool_candidates(kind) for kind in ("cc", "ar", "nm")}
    linkers = llvm_linker_candidates("ld.lld")
    if any(not paths for paths in candidates.values()) or not linkers:
        pytest.skip("canonical LLVM clang/ar/nm/ld.lld tool family is unavailable")
    cc, ar, nm = (str(candidates[kind][0]) for kind in ("cc", "ar", "nm"))

    def run(command):
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
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
                f"-fuse-ld={linkers[0]}",
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
        lambda *args, **kwargs: [],
    )
    plan = native_link_command._build_native_link_plan(
        output_obj=app,
        stub_path=stub,
        runtime_lib=runtime,
        output_binary=tmp_path / "app.elf",
        target_triple=target,
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
    assert "-Wl,--strip-all" not in plan.command
    assert "-Wl,/Brepro" not in plan.command
    assert plan.command.count(str(tmp_path / "libmolt_runtime.a")) == 1
    assert plan.policy.strip_after_link
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
    assert "-Wl,-x" not in plan.command
    assert "-Wl,-S" not in plan.command
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
            target_triple=None,
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
            target_triple=None,
            host_platform="win32",
        )
        == "lld"
    )
    assert (
        native_link_command._resolve_native_linker_hint(
            profile="release",
            target_triple=None,
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
            target_triple=None,
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
            output_obj=None,
            target_triple=None,
            sysroot_path=None,
            profile="dev",
            host_platform="win32",
            host_arch="AMD64",
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
            output_obj=None,
            target_triple=None,
            sysroot_path=None,
            profile="dev",
        )
    )

    assert command == [str(explicit.resolve())]
    assert linker_hint is None


def test_explicit_mold_non_elf_selection_fails_before_link(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_DEV_LINKER", "mold")

    with pytest.raises(RuntimeError, match="Linux ELF"):
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target_triple=None,
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
    assert not plan.policy.strip_after_link


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
    assert not plan.policy.strip_after_link
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
    received: dict[str, object] = {}

    def fake_build(*_args: object, **kwargs: object) -> int:
        received.update(kwargs)
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
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
    assert received["bolt"] is True
    assert received["bolt_training_cmd"] == "{binary} --train"


def test_planned_release_strip_failure_is_loud(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    monkeypatch.setattr(build_results.sys, "platform", "linux")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "x86_64")
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
    monkeypatch.setattr(build_results.sys, "platform", "win32")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "AMD64")
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
    monkeypatch.setattr(build_results.sys, "platform", "win32")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "AMD64")
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

    def fake_sign(path: Path) -> None:
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
            target_triple=None,
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
    assert set(phase_times) == {"strip_wall_ns"}
    assert output.read_bytes() == b"previous"
    assert candidate.read_bytes() == b"linked"


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
    ],
)
def test_native_identity_flags_are_driver_ready(
    host_platform: str,
    cc: tuple[str, ...],
    expected: tuple[str, ...],
) -> None:
    target = native_link_plan.resolve_native_target_spec(
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
