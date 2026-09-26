from __future__ import annotations

import pytest
from dataclasses import replace
from pathlib import Path
from molt.cli.source_extension_language import SourceExtensionLanguage
from molt.cli.source_extensions import (
    _SourceExtensionBuildPlan,
    _SourceExtensionCompileUnit,
    _source_extension_replay_compile_args,
    _validate_source_extension_build_plan_target,
)

from molt.cli.extension_manifest import _extension_binary_suffix
from molt.cli.native_link_plan import _host_target_triple, resolve_native_target_spec
from molt.cli.source_extension_link_requirements import (
    parse_source_extension_link_requirements,
)
from molt.cli.source_extension_target import (
    SourceExtensionLinkDialect,
    resolve_source_extension_target_plan,
    source_extension_artifact_kind,
    source_extension_artifact_suffix,
    source_extension_link_dialect,
    source_extension_recorded_target_plan,
    source_extension_target_is_wasm,
)


@pytest.mark.parametrize(
    "host_platform,arch,triple,dialect",
    [
        ("win32", "AMD64", "x86_64-pc-windows-msvc", "coff-msvc"),
        ("win32", "ARM64", "aarch64-pc-windows-msvc", "coff-msvc"),
        ("darwin", "arm64", "aarch64-apple-darwin", "macho"),
        ("darwin", "x86-64", "x86_64-apple-darwin", "macho"),
        ("linux", "x64", "x86_64-unknown-linux-gnu", "elf-gnu"),
        ("linux", "riscv64", "riscv64-unknown-linux-gnu", "elf-gnu"),
    ],
)
def test_native_host_facts_have_one_projection(host_platform, arch, triple, dialect):
    facts = {"host_platform": host_platform, "host_arch": arch}
    plan = resolve_source_extension_target_plan(" NATIVE ", **facts)
    native = resolve_native_target_spec(None, **facts)
    assert plan.target_triple == _host_target_triple(**facts) == triple
    assert plan.native_target == native
    assert plan.compiler_target_triple is None
    assert plan.requested == "native"
    assert source_extension_link_dialect(triple).value == dialect
    assert source_extension_artifact_suffix(triple) == ".molt.a"
    assert _extension_binary_suffix(triple) == (
        ".pyd" if host_platform == "win32" else ".so"
    )


@pytest.mark.parametrize(
    "requested,triple",
    [
        ("WASM", "wasm32-wasip1"),
        ("wasm-freestanding", "wasm32-unknown-unknown"),
        ("wasm32-wasip1", "wasm32-wasip1"),
        ("wasm32-unknown-unknown", "wasm32-unknown-unknown"),
    ],
)
def test_wasm_target_does_not_consult_native_host(requested, triple):
    plan = resolve_source_extension_target_plan(
        requested, host_platform="unsupported", host_arch=""
    )
    assert plan.target_triple == plan.compiler_target_triple == triple
    assert plan.native_target is None
    assert plan.is_wasm
    assert plan.artifact_kind == "wasm_relocatable_object"
    assert plan.artifact_suffix == ".molt.wasm"
    assert source_extension_link_dialect(triple) is SourceExtensionLinkDialect.WASM


@pytest.mark.parametrize(
    "target",
    [
        "",
        " ",
        "wasm32-typo",
        "wasm64-unknown-unknown",
        "wasm32-wasip2",
        "wasm32-pc-windows-msvc",
        "x86_64-notlinux-gnu",
        "x86_64-apple-ios",
        "x86_64-unknown-linux-android",
        "x86_64-windows-linux-gnu",
        "x86_64-unknown-linux-msvc",
        "x86_64-windowsish-msvc",
        "x86_64--linux-gnu",
        "x86_64 unknown linux gnu",
    ],
)
def test_target_rejection_is_shared_by_all_artifact_consumers(target):
    for consumer in (
        resolve_source_extension_target_plan,
        source_extension_target_is_wasm,
        source_extension_artifact_kind,
        source_extension_artifact_suffix,
        source_extension_link_dialect,
    ):
        with pytest.raises(ValueError):
            consumer(target)
    requirements, errors = parse_source_extension_link_requirements(
        {
            "link_requirements": {
                "target_triple": target,
                "items": [],
                "retained_symbols": [],
            }
        },
        expected_target_triple=target,
    )
    assert requirements is None
    assert errors


@pytest.mark.parametrize(
    "host_platform,host_arch",
    [("other", "x86_64"), ("win32", ""), ("linux", "unknown")],
)
def test_native_target_requires_usable_host_facts(host_platform, host_arch):
    with pytest.raises(ValueError):
        resolve_source_extension_target_plan(
            "native", host_platform=host_platform, host_arch=host_arch
        )


@pytest.mark.parametrize(
    "target,dialect",
    [
        ("x86_64-pc-windows-gnu", "coff-gnu"),
        ("aarch64-pc-windows-gnullvm", "coff-gnu"),
        ("x86_64-pc-windows-msvc", "coff-msvc"),
        ("s390x-unknown-linux-gnu", "elf-gnu"),
        ("loongarch64-unknown-linux-gnu", "elf-gnu"),
        ("aarch64-apple-darwin", "macho"),
    ],
)
def test_explicit_target_policy_is_host_independent(target, dialect):
    plan = resolve_source_extension_target_plan(
        target.upper(), host_platform="other", host_arch=""
    )
    assert plan.target_triple == plan.compiler_target_triple == target
    assert source_extension_link_dialect(target).value == dialect
    assert plan.native_target is not None
    assert plan.native_target.triple == target


def test_none_is_not_an_inner_target_authority():
    with pytest.raises(ValueError, match="explicit"):
        resolve_source_extension_target_plan(None)  # type: ignore[arg-type]


@pytest.mark.parametrize(
    "triple",
    ["x86_64-pc-windows-msvc", "aarch64-apple-darwin", "s390x-unknown-linux-gnu"],
)
def test_recorded_native_target_uses_artifact_not_inspector(triple, monkeypatch):
    def no_host(**_kwargs):
        raise AssertionError("recorded target must not query inspector host")

    monkeypatch.setattr("molt.cli.source_extension_target._host_target_triple", no_host)
    plan = source_extension_recorded_target_plan("native", target_triple=triple)
    assert plan.target_triple == triple
    assert plan.requested == "native"
    assert plan.compiler_target_triple is None
    assert plan.native_target is not None


@pytest.mark.parametrize(
    "requested,triple",
    [
        ("native", "wasm32-wasip1"),
        ("wasm", "wasm32-unknown-unknown"),
        ("wasm-freestanding", "wasm32-wasip1"),
        ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"),
        ("NATIVE", "x86_64-pc-windows-msvc"),
        ("native", "native"),
    ],
)
def test_recorded_target_rejects_alias_and_artifact_drift(requested, triple):
    with pytest.raises(ValueError):
        source_extension_recorded_target_plan(requested, target_triple=triple)


@pytest.fixture
def source_plan():
    source = Path("input.c")
    unit = _SourceExtensionCompileUnit(
        source_path=source,
        owner_target_id="unit",
        producer_object_path=Path("build/unit.p/input.o"),
        generated=False,
        language=SourceExtensionLanguage.C,
        compiler=("clang",),
        include_dirs=(),
        compile_args=(),
    )
    return _SourceExtensionBuildPlan(
        kind="meson-intro-targets",
        plan_path=Path("plan.json"),
        plan_sha256="a" * 64,
        compile_commands_path=None,
        compile_commands_sha256=None,
        target_id="unit",
        target_name="unit",
        target_selector="unit",
        target_type="shared module",
        source_root=Path("src"),
        build_root=Path("build"),
        sources=(source,),
        generated_sources=(),
        skipped_generated_sources=(),
        non_compiled_inputs=(),
        compile_units=(unit,),
        include_dirs=(),
        compile_args=(),
        link_args=(),
        digest="b" * 64,
    )


@pytest.mark.parametrize(
    "args,target,valid",
    [
        (("-DWASI=1", "-I/wasm32/include"), "wasm32-wasip1", False),
        (("--target=wasm32-unknown-unknown",), "wasm32-wasip1", False),
        (("--target=wasm32-wasip1",), "wasm32-wasip1", True),
        (("--target=aarch64-unknown-linux-gnu",), "x86_64-unknown-linux-gnu", False),
        ((), "x86_64-unknown-linux-gnu", True),
    ],
)
def test_source_plan_requires_exact_target_not_incidental_text(
    source_plan, args, target, valid
):
    unit = replace(source_plan.compile_units[0], compile_args=args)
    plan = replace(source_plan, compile_units=(unit,))
    errors = _validate_source_extension_build_plan_target(plan, target_triple=target)
    assert bool(errors) is not valid


def test_source_plan_accepts_zig_driver_target_spelling(source_plan):
    unit = replace(
        source_plan.compile_units[0],
        compiler=("zig", "cc"),
        compile_args=("-target", "wasm32-wasi"),
    )
    plan = replace(source_plan, compile_units=(unit,))
    assert not _validate_source_extension_build_plan_target(
        plan, target_triple="wasm32-wasip1"
    )


def test_replay_validates_target_before_removing_duplicate_selectors():
    assert _source_extension_replay_compile_args(
        ("--target=wasm32-wasip1", "-O2"), compiler_target="wasm32-wasip1"
    ) == ["-O2"]
    with pytest.raises(ValueError, match="conflicts"):
        _source_extension_replay_compile_args(
            ("--target=wasm32-unknown-unknown", "-O2"), compiler_target="wasm32-wasip1"
        )
