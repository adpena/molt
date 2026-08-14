from __future__ import annotations

import hashlib
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
)
from molt.cli.source_extension_target import (
    SourceExtensionLinkDialect,
    source_extension_link_dialect,
)


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def test_link_requirements_publish_typed_checksummed_inputs_and_render_late(
    tmp_path: Path,
) -> None:
    build_root = tmp_path / "build"
    publish_root = tmp_path / "wheel" / "demo"
    build_root.mkdir()
    folded = build_root / "demo.molt.a"
    dependency = build_root / "libdependency.a"
    folded.write_bytes(b"extension")
    dependency.write_bytes(b"dependency")

    requirements = source_extension_link_requirements(
        (
            str(folded),
            "-Wl,--as-needed",
            "-lm",
            "-Wl,--no-as-needed",
            "-Wl,--whole-archive",
            str(dependency),
            "-Wl,--no-whole-archive",
        ),
        target_triple="x86_64-unknown-linux-gnu",
        folded_static_archives=(folded.name,),
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
        "-Wl,--whole-archive",
        relative,
        "-Wl,--no-whole-archive",
    )
    assert (publish_root / relative).read_bytes() == b"dependency"
    assert "arguments" not in requirements.manifest_payload()


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
        "-Wl,--start-group",
        "libfirst.a",
        "-Wl,--whole-archive",
        "libsecond.a",
        "-Wl,--no-whole-archive",
        "-Wl,--end-group",
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
            "-Wl,-force_load,",
        ),
        (
            "x86_64-pc-windows-msvc",
            "-Wl,/WHOLEARCHIVE:{path}",
            "-Wl,/WHOLEARCHIVE:",
        ),
    ],
)
def test_target_loading_syntax_becomes_one_input_policy(
    tmp_path: Path,
    target_triple: str,
    loading_argument: str,
    expected_prefix: str,
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
    assert render_source_extension_link_arguments(requirements)[0].startswith(
        expected_prefix
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
        f"-Wl,-force_load,{archive.resolve()}",
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
