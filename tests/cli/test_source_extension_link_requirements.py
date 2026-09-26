from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkCyclicGroup,
    SourceExtensionLinkInput,
    SourceExtensionLinkLoadingPolicy,
    SourceExtensionLinkProvider,
    SourceExtensionLinkProviderKind,
    SourceExtensionLinkRequirements,
    materialize_source_extension_link_requirements,
    parse_source_extension_link_requirements,
    render_source_extension_link_arguments,
    resolve_source_extension_link_arguments,
    source_extension_link_requirements,
    merge_source_extension_link_requirements,
    map_source_extension_link_inputs,
    read_source_extension_link_plan,
    source_extension_link_file,
    validate_source_extension_link_input_files,
)
from molt.cli.source_extension_target import (
    SourceExtensionLinkDialect,
    source_extension_link_dialect,
)
from molt.cli.native_link_plan import whole_archive_link_arguments
from molt.cli.source_extension_link_arguments import source_extension_link_arguments
from molt.cli.source_extensions import _meson_static_library_projection


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def test_local_link_plan_preserves_order_loading_groups_and_repeated_inputs(tmp_path):
    path = tmp_path / "library, with spaces.a"
    path.write_bytes(b"archive")
    lazy = source_extension_link_file(path)
    eager = source_extension_link_file(
        path, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    )
    group = SourceExtensionLinkCyclicGroup((lazy, eager))
    target = "x86_64-unknown-linux-gnu"
    plan = merge_source_extension_link_requirements(
        (
            SourceExtensionLinkRequirements(target, (group,), ("root_b",)),
            SourceExtensionLinkRequirements(target, (lazy,), ("root_a", "root_b")),
        ),
        target_triple=target,
    )
    plan_path = tmp_path / "plan.json"
    plan_path.write_text(
        json.dumps({"link_requirements": plan.manifest_payload()}), encoding="utf-8"
    )
    loaded = read_source_extension_link_plan(plan_path, expected_target_triple=target)
    assert loaded.items == (group, lazy)
    assert loaded.retained_symbols == ("root_a", "root_b")
    assert loaded.inputs == (lazy, eager, lazy)
    validate_source_extension_link_input_files(loaded)
    mapped = map_source_extension_link_inputs(
        loaded,
        lambda item: SourceExtensionLinkInput(
            str(tmp_path / "snapshot.a"),
            item.sha256,
            item.loading,
        ),
    )
    assert [item.loading for item in mapped.inputs] == [
        lazy.loading,
        eager.loading,
        lazy.loading,
    ]
    path.write_bytes(b"changed")
    with pytest.raises(ValueError, match="checksum mismatch"):
        validate_source_extension_link_input_files(loaded)
    with pytest.raises(ValueError, match="must match target_triple"):
        read_source_extension_link_plan(
            plan_path, expected_target_triple="aarch64-unknown-linux-gnu"
        )


def test_local_link_plan_rejects_ambiguous_json_and_relative_inputs(tmp_path):
    path = tmp_path / "plan.json"
    path.write_text(
        '{"link_requirements": {}, "link_requirements": {}}', encoding="utf-8"
    )
    with pytest.raises(ValueError, match="duplicate JSON key"):
        read_source_extension_link_plan(path, expected_target_triple="wasm32-wasip1")
    requirements = SourceExtensionLinkRequirements(
        "wasm32-wasip1", (SourceExtensionLinkInput("relative.a", "0" * 64),)
    )
    path.write_text(
        json.dumps({"link_requirements": requirements.manifest_payload()}),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="local link input must be absolute"):
        read_source_extension_link_plan(path, expected_target_triple="wasm32-wasip1")


@pytest.mark.parametrize(
    "triple,product,dependency",
    [
        (
            "x86_64-pc-windows-msvc",
            (
                "/nologo",
                "/OPT:REF",
                "/DLL",
                "/IMPLIB:extension.lib",
                "/OUT:extension.pyd",
                "/PDB:extension.pdb",
            ),
            ("/DEFAULTLIB:kernel32.lib",),
        ),
        (
            "x86_64-unknown-linux-gnu",
            ("-shared", "-Wl,--gc-sections,-soname,extension.so", "-o", "extension.so"),
            ("-lm",),
        ),
        (
            "aarch64-apple-darwin",
            ("-bundle", "-Wl,-dead_strip", "-o", "extension.so"),
            ("-framework", "Accelerate"),
        ),
        (
            "x86_64-pc-windows-gnu",
            ("-shared", "-Wl,--gc-sections", "--output=extension.dll"),
            ("-lkernel32",),
        ),
        (
            "wasm32-wasip1",
            ("--no-entry", "--gc-sections", "-o", "extension.wasm"),
            ("-lm",),
        ),
    ],
)
def test_meson_image_policy_is_recorded_but_never_becomes_link_dependency(
    tmp_path: Path, triple: str, product: tuple[str, ...], dependency: tuple[str, ...]
) -> None:
    primary = {
        "id": "extension",
        "type": "shared module",
        "filename": ["extension.so"],
        "target_sources": [
            {"linker": ["linker"], "parameters": [*product, *dependency]}
        ],
    }
    projection = _meson_static_library_projection(
        primary_target=primary,
        payload=[
            primary,
            {"id": "decoy", "type": "static library", "filename": ["extension.lib"]},
        ],
        build_root=tmp_path,
    )
    assert projection.targets == ()
    assert tuple(
        argument
        for item in projection.ordered_items
        for argument in item.span.arguments
    ) == tuple(
        argument
        for span in source_extension_link_arguments((*product, *dependency))
        for argument in span.arguments
    )
    dialect = source_extension_link_dialect(triple)
    for item in projection.ordered_items:
        item.span.validate_dialect(dialect)
    assert source_extension_link_requirements(
        tuple(
            argument
            for item in projection.ordered_items
            if item.disposition == "external"
            for argument in item.span.arguments
        ),
        target_triple=triple,
    ) == (source_extension_link_requirements(dependency, target_triple=triple))
    # Explicit final-link configuration cannot seize output custody, even when
    # the same option is meaningful in captured upstream producer metadata.
    with pytest.raises(ValueError, match="output/image policy"):
        source_extension_link_requirements(product, target_triple=triple)


@pytest.mark.parametrize(
    "arguments",
    [
        ("/OPT:garbage",),
        ("/OPT:NOREF",),
        ("/OPT:NOICF",),
        ("/OPT:REF,ICF",),
        ("--no-gc-sections",),
        ("/NODEFAULTLIB:kernel32.lib",),
        ("/MACHINE:ARM64",),
        ("/DEF:exports.def",),
        ("/LIBPATH:unsealed",),
        ("@unsealed.rsp",),
        ("-Wl,--version-script,unsealed.map",),
        ("-Wl,--export=secret",),
    ],
)
def test_meson_projection_does_not_discard_unmodeled_semantic_or_resource_flags(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    primary = {
        "id": "extension",
        "type": "shared module",
        "filename": ["extension.so"],
        "link_args": list(arguments),
    }
    projection = _meson_static_library_projection(
        primary_target=primary, payload=[primary], build_root=tmp_path
    )
    external = tuple(
        argument
        for item in projection.ordered_items
        if item.disposition == "external"
        for argument in item.span.arguments
    )
    assert external
    for triple in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"):
        with pytest.raises(ValueError):
            source_extension_link_requirements(external, target_triple=triple)


@pytest.mark.parametrize(
    "arguments", [("-o",), ("/OUT:",), ("/IMPLIB:",), ("-Wl,-soname",), ("-Xlinker",)]
)
def test_meson_product_options_require_their_complete_operand(arguments) -> None:
    with pytest.raises(ValueError):
        source_extension_link_arguments(arguments)


def test_linker_transport_preserves_literal_comma_paths_through_projection(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "dep, with space.a"
    archive.write_bytes(b"real dependency bytes")
    arguments = ("-Xlinker", "-force_load", "-Xlinker", str(archive))
    primary = {
        "id": "extension",
        "type": "shared module",
        "filename": ["extension.so"],
        "link_args": ["-bundle", *arguments],
    }
    projection = _meson_static_library_projection(
        primary_target=primary, payload=[primary], build_root=tmp_path
    )
    requirements = source_extension_link_requirements(
        tuple(
            argument
            for item in projection.ordered_items
            if item.disposition == "external"
            for argument in item.span.arguments
        ),
        target_triple="aarch64-apple-darwin",
        path_roots=(tmp_path,),
        publish_root=tmp_path / "published",
    )
    assert requirements.inputs[0].sha256 == _sha256(b"real dependency bytes")
    assert (
        requirements.inputs[0].loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    )


def test_consumed_upstream_policy_cannot_cross_target_dialects() -> None:
    for span in source_extension_link_arguments(
        ("/nologo", "/DLL", "/IMPLIB:output.lib")
    ):
        with pytest.raises(ValueError, match="elf-gnu"):
            span.validate_dialect(SourceExtensionLinkDialect.ELF_GNU)


@pytest.mark.parametrize(
    "triple",
    [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "wasm32-wasip1",
    ],
)
def test_archive_argument_roundtrip_preserves_comma_and_space_path(
    tmp_path, triple
) -> None:
    source = tmp_path / "dependency, with space.a"
    source.write_bytes(b"dependency")
    dialect = source_extension_link_dialect(triple)
    arguments = whole_archive_link_arguments(str(source), dialect=dialect)
    requirements = source_extension_link_requirements(
        arguments,
        target_triple=triple,
        path_roots=(tmp_path,),
        publish_root=tmp_path / "published",
    )
    assert len(requirements.inputs) == 1
    item = requirements.inputs[0]
    assert item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    assert item.path.endswith(source.name)
    assert render_source_extension_link_arguments(
        requirements
    ) == whole_archive_link_arguments(item.path, dialect=dialect)


@pytest.mark.parametrize(
    "arguments",
    [
        ("-Xlinker", "-force_load", "-Xlinker", "provider.a"),
        ("-Xlinker", "/WHOLEARCHIVE:provider.lib"),
        ("-Xlinker", "-Xlinker"),
        ("-Xlinker", "--output=foreign"),
    ],
)
def test_driver_envelopes_cannot_cross_link_dialects_or_take_output_authority(
    arguments,
) -> None:
    with pytest.raises(ValueError):
        source_extension_link_requirements(
            arguments, target_triple="x86_64-unknown-linux-gnu"
        )


def test_driver_wrapped_archive_group_keeps_the_typed_group_and_member_policy() -> None:
    requirements = source_extension_link_requirements(
        (
            "-Xlinker",
            "--start-group",
            "first.a",
            "-Xlinker",
            "--whole-archive",
            "second.a",
            "-Xlinker",
            "--no-whole-archive",
            "-Xlinker",
            "--end-group",
        ),
        target_triple="x86_64-pc-windows-gnu",
    )
    group = requirements.items[0]
    assert isinstance(group, SourceExtensionLinkCyclicGroup)
    assert group.members[1].loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    assert (
        source_extension_link_requirements(
            render_source_extension_link_arguments(requirements),
            target_triple=requirements.target_triple,
        )
        == requirements
    )


def test_link_requirements_publish_typed_checksummed_inputs_and_render_late(
    tmp_path: Path,
) -> None:
    build_root = tmp_path / "build"
    publish_root = tmp_path / "wheel" / "demo"
    build_root.mkdir()
    dependency = build_root / "libdependency.a"
    dependency.write_bytes(b"dependency")

    requirements = source_extension_link_requirements(
        (
            "-Wl,--as-needed",
            "-lm",
            "-Wl,--no-as-needed",
            "-Wl,--whole-archive",
            str(dependency),
            "-Wl,--no-whole-archive",
        ),
        target_triple="x86_64-unknown-linux-gnu",
        path_roots=(build_root,),
        publish_root=publish_root,
    )

    relative = f"__molt_link__/{_sha256(b'dependency')}/libdependency.a"
    assert requirements.items == (
        SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY,
            "m",
            SourceExtensionLinkLoadingPolicy.AS_NEEDED,
        ),
        SourceExtensionLinkInput(
            relative,
            _sha256(b"dependency"),
            SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
        ),
    )
    assert render_source_extension_link_arguments(requirements) == (
        "-Wl,--as-needed",
        "-lm",
        "-Wl,--no-as-needed",
        "-Xlinker",
        "--whole-archive",
        relative,
        "-Xlinker",
        "--no-whole-archive",
    )
    assert (publish_root / relative).read_bytes() == b"dependency"
    assert "arguments" not in requirements.manifest_payload()


def test_elf_dependency_policy_is_idempotent_and_persistent() -> None:
    requirements = source_extension_link_requirements(
        (
            "-Wl,--no-as-needed",
            "-Wl,--no-as-needed",
            "-lm",
            "-Wl,--as-needed",
            "-Wl,--as-needed",
            "-ldl",
        ),
        target_triple="x86_64-unknown-linux-gnu",
    )
    assert requirements.items == (
        SourceExtensionLinkProvider(SourceExtensionLinkProviderKind.LIBRARY, "m"),
        SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY,
            "dl",
            SourceExtensionLinkLoadingPolicy.AS_NEEDED,
        ),
    )
    assert (
        source_extension_link_requirements(
            render_source_extension_link_arguments(requirements),
            target_triple=requirements.target_triple,
        )
        == requirements
    )


@pytest.mark.parametrize(
    "argument",
    (
        "-o",
        "-oowned.wasm",
        "--output=owned.wasm",
        "/OUT:owned.exe",
        "-shared",
        "@response.rsp",
        "-Wl,@response.rsp",
        "-Lunsealed",
        "-Wl,--library-path=unsealed",
        "-dynamiclib",
        "/DEFAULTLIB:../unsealed.lib",
        "-Wl,-Tunsealed.ld",
        "-Wl,--version-script=unsealed.map",
        "/wholearchive:C:\\outside.lib",
        "-Xlinker",
        "--sysroot=/outside",
        "-Wl,--sysroot,/outside",
        "-Wl,-Map,secondary.map",
        "-fuse-ld=outside-linker",
        "-Wl,--allow-undefined",
        "--allow-undefined",
        "--no-entry",
    ),
)
def test_link_requirements_reject_final_link_mode_and_path_authority(
    argument: str,
) -> None:
    with pytest.raises(ValueError):
        source_extension_link_requirements(
            (argument,),
            target_triple="wasm32-wasip1",
        )


@pytest.mark.parametrize(
    "arguments",
    [
        ("-Wl,--start-group", "liba.a"),
        ("-Wl,--end-group",),
        (
            "-Wl,--start-group",
            "-Wl,--start-group",
            "liba.a",
            "-Wl,--end-group",
            "-Wl,--end-group",
        ),
        ("-Wl,--whole-archive", "liba.a"),
        ("-Wl,--no-whole-archive",),
        (
            "-Wl,--whole-archive",
            "-Wl,--whole-archive",
            "liba.a",
            "-Wl,--no-whole-archive",
        ),
        (
            "-Wl,--start-group",
            "-Wl,--whole-archive",
            "liba.a",
            "-Wl,--end-group",
            "-Wl,--no-whole-archive",
        ),
    ],
)
def test_gnu_group_and_whole_archive_grammar_is_balanced(
    arguments: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError):
        source_extension_link_requirements(
            arguments,
            target_triple="x86_64-unknown-linux-gnu",
        )


def test_cyclic_group_is_structural_and_preserves_member_policies() -> None:
    requirements = source_extension_link_requirements(
        (
            "-Wl,--start-group",
            "libfirst.a",
            "-Wl,--whole-archive",
            "libsecond.a",
            "-Wl,--no-whole-archive",
            "-Wl,--end-group",
        ),
        target_triple="wasm32-wasip1",
    )

    assert requirements.items == (
        SourceExtensionLinkCyclicGroup(
            (
                SourceExtensionLinkProvider(
                    SourceExtensionLinkProviderKind.ARCHIVE,
                    "libfirst.a",
                ),
                SourceExtensionLinkProvider(
                    SourceExtensionLinkProviderKind.ARCHIVE,
                    "libsecond.a",
                    SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
                ),
            )
        ),
    )
    assert render_source_extension_link_arguments(requirements) == (
        "libfirst.a",
        "--whole-archive",
        "libsecond.a",
        "--no-whole-archive",
    )


def test_bare_system_library_names_are_typed_providers() -> None:
    requirements = source_extension_link_requirements(
        ("python313.lib", "/DEFAULTLIB:ucrt.lib"),
        target_triple="x86_64-pc-windows-msvc",
    )

    assert requirements.items == (
        SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY,
            "python313.lib",
        ),
        SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY,
            "ucrt.lib",
        ),
    )
    assert render_source_extension_link_arguments(requirements) == (
        "python313.lib",
        "ucrt.lib",
    )


@pytest.mark.parametrize(
    ("target_triple", "argument"),
    [
        ("x86_64-pc-windows-msvc", "-Wl,--as-needed"),
        ("x86_64-pc-windows-msvc", "-lm"),
        ("x86_64-unknown-linux-gnu", "/DEFAULTLIB:ucrt.lib"),
        ("x86_64-unknown-linux-gnu", "-Wl,-framework,Accelerate"),
        ("wasm32-wasip1", "-Wl,-force_load,libprovider.a"),
    ],
)
def test_link_requirements_reject_cross_dialect_arguments(
    target_triple: str, argument: str
) -> None:
    with pytest.raises(ValueError):
        source_extension_link_requirements(
            (argument,),
            target_triple=target_triple,
        )


@pytest.mark.parametrize(
    ("target_triple", "source_argument", "rendered"),
    [
        (
            "x86_64-unknown-linux-gnu",
            "-Wl,--undefined=PyInit_demo",
            "-Wl,--undefined=PyInit_demo",
        ),
        (
            "aarch64-apple-darwin",
            "-Wl,-u,_PyInit_demo",
            "-Wl,-u,_PyInit_demo",
        ),
        (
            "x86_64-pc-windows-gnullvm",
            "-Wl,-u,PyInit_demo",
            "-Wl,--undefined=PyInit_demo",
        ),
        (
            "x86_64-pc-windows-msvc",
            "/INCLUDE:PyInit_demo",
            "-Wl,/INCLUDE:PyInit_demo",
        ),
        (
            "wasm32-wasip1",
            "--undefined=PyInit_demo",
            "--undefined=PyInit_demo",
        ),
    ],
)
def test_retained_symbols_are_typed_then_rendered_for_selected_dialect(
    target_triple: str,
    source_argument: str,
    rendered: str,
) -> None:
    requirements = source_extension_link_requirements(
        (source_argument,),
        target_triple=target_triple,
    )
    assert requirements.retained_symbols == (
        source_argument.rsplit(":", 1)[-1]
        if "/INCLUDE:" in source_argument
        else source_argument.rsplit(",", 1)[-1].rsplit("=", 1)[-1],
    )
    assert render_source_extension_link_arguments(requirements) == (rendered,)


def test_windows_gnullvm_uses_coff_gnu_dialect() -> None:
    assert (
        source_extension_link_dialect("x86_64-pc-windows-gnullvm")
        is SourceExtensionLinkDialect.COFF_GNU
    )


def test_manifest_parser_requires_typed_exact_canonical_schema() -> None:
    payload = SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        items=(
            SourceExtensionLinkProvider(
                SourceExtensionLinkProviderKind.LIBRARY,
                "m",
            ),
        ),
        retained_symbols=("PyInit_demo",),
    ).manifest_payload()
    parsed, errors = parse_source_extension_link_requirements(
        {"link_requirements": payload},
        expected_target_triple="wasm32-wasip1",
    )
    assert errors == []
    assert parsed is not None
    assert parsed.manifest_payload() == payload

    payload["arguments"] = []
    parsed, errors = parse_source_extension_link_requirements(
        {"link_requirements": payload},
        expected_target_triple="wasm32-wasip1",
    )
    assert parsed is None
    assert any("keys must be exactly" in error for error in errors)


def test_manifest_parser_rejects_target_drift_and_uppercase_digest() -> None:
    digest = "A" * 64
    parsed, errors = parse_source_extension_link_requirements(
        {
            "link_requirements": {
                "target_triple": "x86_64-unknown-linux-gnu",
                "items": [
                    {
                        "kind": "input",
                        "path": "__molt_link__/input/libunsealed.a",
                        "sha256": digest,
                        "loading": "default",
                    }
                ],
                "retained_symbols": [],
            }
        },
        expected_target_triple="wasm32-wasip1",
    )

    assert parsed is None
    assert any("must match target_triple" in error for error in errors)
    assert any("lowercase SHA-256" in error for error in errors)


def test_manifest_parser_requires_explicit_empty_link_requirements() -> None:
    parsed, errors = parse_source_extension_link_requirements(
        {},
        expected_target_triple="wasm32-wasip1",
    )
    assert parsed is None
    assert errors == ["link_requirements must be an explicit object"]


def test_manifest_parser_rejects_package_escape() -> None:
    parsed, errors = parse_source_extension_link_requirements(
        {
            "link_requirements": {
                "target_triple": "wasm32-wasip1",
                "items": [
                    {
                        "kind": "input",
                        "path": "../libescape.a",
                        "sha256": "0" * 64,
                        "loading": "default",
                    }
                ],
                "retained_symbols": [],
            }
        },
        expected_target_triple="wasm32-wasip1",
    )
    assert parsed is None
    assert any("package-relative" in error for error in errors)


def test_link_requirement_publication_rejects_source_root_escape(
    tmp_path: Path,
) -> None:
    source_root = tmp_path / "source"
    source_root.mkdir()
    outside = tmp_path / "outside.a"
    outside.write_bytes(b"outside")

    with pytest.raises(ValueError, match="escapes declared source roots"):
        source_extension_link_requirements(
            (str(outside),),
            target_triple="wasm32-wasip1",
            path_roots=(source_root,),
            publish_root=tmp_path / "publish",
        )


@pytest.mark.parametrize(
    ("target_triple", "loading_argument", "expected_prefix"),
    [
        (
            "aarch64-apple-darwin",
            "-Wl,-force_load,{path}",
            ("-Xlinker", "-force_load", "-Xlinker"),
        ),
        (
            "x86_64-pc-windows-msvc",
            "-Wl,/WHOLEARCHIVE:{path}",
            ("-Xlinker",),
        ),
    ],
)
def test_target_loading_syntax_becomes_one_input_policy(
    tmp_path: Path,
    target_triple: str,
    loading_argument: str,
    expected_prefix: tuple[str, ...],
) -> None:
    source = tmp_path / (
        "dependency.lib" if "windows" in target_triple else "dependency.a"
    )
    source.write_bytes(b"dependency")
    publish = tmp_path / "publish"
    requirements = source_extension_link_requirements(
        (loading_argument.format(path=source),),
        target_triple=target_triple,
        path_roots=(tmp_path,),
        publish_root=publish,
    )
    assert (
        requirements.inputs[0].loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    )
    assert (
        render_source_extension_link_arguments(requirements)[: len(expected_prefix)]
        == expected_prefix
    )


def test_resolve_and_materialize_verify_bytes_and_preserve_structure(
    tmp_path: Path,
) -> None:
    package_root = tmp_path / "source" / "demo"
    manifest_dir = package_root / "pkg"
    archive = package_root / "__molt_link__" / "input" / "libdependency.a"
    manifest_dir.mkdir(parents=True)
    archive.parent.mkdir(parents=True)
    archive.write_bytes(b"dependency")
    relative = archive.relative_to(package_root).as_posix()
    requirements = SourceExtensionLinkRequirements(
        target_triple="aarch64-apple-darwin",
        items=(
            SourceExtensionLinkProvider(
                SourceExtensionLinkProviderKind.THREAD_RUNTIME,
                "pthread",
            ),
            SourceExtensionLinkInput(
                relative,
                _sha256(b"dependency"),
                SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
            ),
        ),
    )

    resolved, errors = resolve_source_extension_link_arguments(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
    )
    assert errors == []
    assert resolved == (
        "-pthread",
        "-Xlinker",
        "-force_load",
        "-Xlinker",
        str(archive.resolve()),
    )

    publish_root = tmp_path / "published" / "demo"
    materialized, errors = materialize_source_extension_link_requirements(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
        publish_root=publish_root,
    )
    assert errors == []
    assert materialized is not None
    published_input = materialized.inputs[0]
    assert published_input.path == (
        f"__molt_link__/{_sha256(b'dependency')}/libdependency.a"
    )
    assert published_input.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
    assert (publish_root / published_input.path).read_bytes() == b"dependency"

    archive.write_bytes(b"tampered")
    resolved, errors = resolve_source_extension_link_arguments(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
    )
    assert resolved is None
    assert len(errors) == 1
    assert "checksum mismatch" in errors[0]


@pytest.mark.parametrize("suffix", [".a", ".lib"])
def test_explicit_same_basename_inputs_are_never_implicitly_folded(
    tmp_path: Path,
    suffix: str,
) -> None:
    roots = (tmp_path / "local", tmp_path / "external")
    for root in roots:
        root.mkdir()
    paths = tuple(root / ("libsame" + suffix) for root in roots)
    paths[0].write_bytes(b"local-input")
    paths[1].write_bytes(b"external-input")
    requirements = source_extension_link_requirements(
        tuple(str(path) for path in paths),
        target_triple="x86_64-unknown-linux-gnu",
        path_roots=roots,
        publish_root=tmp_path / "publish",
    )
    assert len(requirements.items) == 2
    inputs = tuple(
        item
        for item in requirements.items
        if isinstance(item, SourceExtensionLinkInput)
    )
    assert len(inputs) == 2
    assert tuple(item.sha256 for item in inputs) == (
        _sha256(b"local-input"),
        _sha256(b"external-input"),
    )
