from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkInput,
    SourceExtensionLinkRequirements,
    materialize_source_extension_link_requirements,
    parse_source_extension_link_requirements,
    resolve_source_extension_link_arguments,
    source_extension_link_requirements,
)


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def test_link_requirements_fold_and_publish_checksummed_static_inputs(
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
            "-Wl,--whole-archive",
            str(dependency),
            "-Wl,--no-whole-archive",
        ),
        target_triple="x86_64-pc-windows-msvc",
        folded_static_archives=(folded.name,),
        path_roots=(build_root,),
        publish_root=publish_root,
    )

    assert requirements.arguments == (
        "-Wl,--as-needed",
        "-Wl,--whole-archive",
        f"__molt_link__/{_sha256(b'dependency')}/libdependency.a",
        "-Wl,--no-whole-archive",
    )
    assert requirements.inputs == (
        SourceExtensionLinkInput(
            argument_index=2,
            path=f"__molt_link__/{_sha256(b'dependency')}/libdependency.a",
            sha256=_sha256(b"dependency"),
        ),
    )
    assert (publish_root / requirements.inputs[0].path).read_bytes() == b"dependency"


@pytest.mark.parametrize(
    "argument",
    (
        "-o",
        "--output=owned.wasm",
        "/OUT:owned.exe",
        "-shared",
        "@response.rsp",
        "-Lunsealed",
        "-Wl,--version-script=unsealed.map",
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


def test_manifest_parser_rejects_target_drift_and_unchecksummed_static_path() -> None:
    parsed, errors = parse_source_extension_link_requirements(
        {
            "link_requirements": {
                "target_triple": "x86_64-unknown-linux-gnu",
                "arguments": ["subdir/libunsealed.a"],
                "inputs": [],
            }
        },
        expected_target_triple="wasm32-wasip1",
    )

    assert parsed is None
    assert any("must match target_triple" in error for error in errors)
    assert any("unchecksummed static path operand" in error for error in errors)


def test_bare_system_library_names_are_not_misclassified_as_path_inputs() -> None:
    requirements = source_extension_link_requirements(
        ("python313.lib", "libm.a", "/DEFAULTLIB:ucrt.lib"),
        target_triple="x86_64-pc-windows-msvc",
    )

    assert requirements.arguments == (
        "python313.lib",
        "libm.a",
        "/DEFAULTLIB:ucrt.lib",
    )
    assert requirements.inputs == ()
    parsed, errors = parse_source_extension_link_requirements(
        {"link_requirements": requirements.manifest_payload()},
        expected_target_triple="x86_64-pc-windows-msvc",
    )
    assert errors == []
    assert parsed == requirements


def test_manifest_parser_rejects_package_escape() -> None:
    parsed, errors = parse_source_extension_link_requirements(
        {
            "link_requirements": {
                "target_triple": "wasm32-wasip1",
                "arguments": ["../libescape.a"],
                "inputs": [
                    {
                        "argument_index": 0,
                        "path": "../libescape.a",
                        "sha256": "0" * 64,
                        "prefix": "",
                    }
                ],
            }
        },
        expected_target_triple="wasm32-wasip1",
    )

    assert parsed is None
    assert any("must be package-relative" in error for error in errors)


def test_resolve_and_materialize_verify_bytes_and_rewrite_only_owned_operands(
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
        target_triple="wasm32-wasip1",
        arguments=("--allow-undefined", f"-Wl,-force_load,{relative}"),
        inputs=(
            SourceExtensionLinkInput(
                argument_index=1,
                path=relative,
                sha256=_sha256(b"dependency"),
                prefix="-Wl,-force_load,",
            ),
        ),
    )

    resolved, errors = resolve_source_extension_link_arguments(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
    )
    assert errors == []
    assert resolved == ("--allow-undefined", f"-Wl,-force_load,{archive.resolve()}")

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
    assert (publish_root / published_input.path).read_bytes() == b"dependency"
    assert materialized.arguments[1] == f"-Wl,-force_load,{published_input.path}"

    archive.write_bytes(b"tampered")
    resolved, errors = resolve_source_extension_link_arguments(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
    )
    assert resolved is None
    assert len(errors) == 1
    assert "checksum mismatch" in errors[0]
