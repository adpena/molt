from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli import (
    native_link_command,
    native_link_tool_identity,
    source_extension_target,
)
from molt.cli.native_link_plan import (
    LinkDialect,
    NativeArtifactKind,
    NativeLinkPlan,
    native_artifact_link_arguments,
    native_link_capabilities,
    native_link_policy,
    native_link_policy_flags,
    resolve_link_dialect,
    resolve_native_target_spec,
)
from tests.cli.native_link_test_support import RUNTIME_BUILD_IDENTITY
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    SourceExtensionLinkLoadingPolicy,
    source_extension_link_file,
)


@pytest.mark.parametrize(
    ("triple", "dialect", "role"),
    [
        ("x86_64-pc-windows-msvc", LinkDialect.COFF_MSVC, "lld-link"),
        ("x86_64-pc-windows-gnu", LinkDialect.COFF_GNU, "ld.lld"),
        ("aarch64-pc-windows-gnullvm", LinkDialect.COFF_GNU, "ld.lld"),
        ("s390x-unknown-linux-gnu", LinkDialect.ELF_GNU, "ld.lld"),
        ("aarch64-apple-darwin", LinkDialect.MACHO, "ld64.lld"),
        ("wasm32-wasip1", LinkDialect.WASM, "wasm-ld"),
    ],
)
def test_one_target_dialect_selects_native_and_extension_linker_roles(
    triple, dialect, role
) -> None:
    assert source_extension_target.SourceExtensionLinkDialect is LinkDialect
    assert source_extension_target.source_extension_link_dialect is resolve_link_dialect
    assert resolve_link_dialect(triple) is dialect
    assert dialect.llvm_linker_role == role


@pytest.mark.parametrize("environment", ["gnu", "gnullvm"])
def test_gnu_coff_archive_and_identity_policy_never_use_msvc_arguments(
    environment,
) -> None:
    target = resolve_native_target_spec(f"x86_64-pc-windows-{environment}")
    capabilities = native_link_capabilities(target=target, linker_hint="lld")
    assert native_artifact_link_arguments(
        Path("compiler, input.a"), kind=NativeArtifactKind.ARCHIVE, target=target
    ) == (
        "-Xlinker",
        "--whole-archive",
        "compiler, input.a",
        "-Xlinker",
        "--no-whole-archive",
    )
    assert native_link_policy_flags(target=target, capabilities=capabilities) == (
        "-Wl,--no-insert-timestamp",
        "-Wl,--gc-sections",
        "-Wl,--icf=none",
    )
    with pytest.raises(RuntimeError, match="MSVC driver"):
        native_link_policy_flags(
            target=target, capabilities=capabilities, msvc_driver=True
        )


def test_gnu_coff_linker_discovery_and_replay_custody_use_same_role(
    tmp_path, monkeypatch
) -> None:
    target = resolve_native_target_spec("x86_64-pc-windows-gnu")
    driver, linker = tmp_path / "clang.exe", tmp_path / "ld.lld.exe"
    driver.write_bytes(b"driver")
    linker.write_bytes(b"linker")
    seen: list[str] = []

    def candidates(role, **kwargs):
        seen.append(role)
        return (linker,)

    monkeypatch.setenv("MOLT_DEV_LINKER", "lld")
    monkeypatch.setattr(native_link_command, "llvm_linker_candidates", candidates)
    assert (
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target_triple=target.triple,
            driver_command=(str(driver),),
        )
        == "lld"
    )
    plan = NativeLinkPlan(
        target=target,
        capabilities=native_link_capabilities(target=target, linker_hint="lld"),
        policy=native_link_policy(
            target=target, profile="dev", keep_symbols=False, bolt_requested=False
        ),
        command=(str(driver), "-fuse-ld=lld"),
        linker_hint="lld",
        normalized_target=target.triple,
    )
    monkeypatch.setattr(native_link_tool_identity, "llvm_linker_candidates", candidates)
    facts = native_link_tool_identity.native_link_cache_tool_facts(plan)
    assert seen == ["ld.lld", "ld.lld"]
    assert next(fact for fact in facts if fact["role"] == "linker")["path"] == str(
        linker
    )


def test_final_gnu_coff_link_has_no_msvc_archive_policy_or_definition_options(
    tmp_path, monkeypatch
) -> None:
    triple = "x86_64-pc-windows-gnu"
    extension = tmp_path / "extension, with spaces.a"
    extension.write_bytes(b"archive")
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang", "-fuse-ld=lld"], "lld", triple),
    )
    monkeypatch.setattr(
        native_link_command,
        "_collect_cargo_native_link_deps",
        lambda *args, **kwargs: [],
    )
    monkeypatch.setattr(
        native_link_command,
        "_append_darwin_runtime_frameworks",
        lambda *args, **kwargs: None,
    )
    plan = native_link_command._build_native_link_plan(
        output_obj=tmp_path / "app.a",
        stub_path=tmp_path / "main.c",
        runtime_lib=tmp_path / "runtime.a",
        output_binary=tmp_path / "app.exe",
        target_triple=triple,
        sysroot_path=None,
        profile="dev",
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        external_link_requirements=(
            SourceExtensionLinkRequirements(
                triple,
                (
                    source_extension_link_file(
                        extension, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                    ),
                ),
            ),
        ),
    )
    assert plan.command.count("--whole-archive") == 2
    assert str(extension.resolve()) in plan.command
    assert "-Wl,--no-insert-timestamp" in plan.command
    assert "-Wl,--icf=none" in plan.command
    assert str(tmp_path / ".molt_exports.def") in plan.command
    assert not (tmp_path / ".molt_exports.def").exists()
    assert plan.sidecars[0].content.startswith(b"EXPORTS\n")
    assert not any(
        any(option in arg for option in ("/OPT:", "/DEF:", "/WHOLEARCHIVE:", "/Brepro"))
        for arg in plan.command
    )
