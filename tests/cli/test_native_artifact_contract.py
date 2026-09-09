from __future__ import annotations

from pathlib import Path

import pytest

from tests.native_artifact_fixtures import (
    coff_header,
    elf_header,
    macho_header,
    pe_header,
)

from molt.cli import link_pipeline
from molt.cli.native_link_plan import (
    NativeArtifactKind,
    native_artifact_link_arguments,
    resolve_native_target_spec,
    validate_native_object_artifact,
)


@pytest.mark.parametrize(
    ("platform", "object_suffix", "archive_suffix"),
    [("linux", ".o", ".a"), ("darwin", ".o", ".a"), ("win32", ".obj", ".lib")],
)
def test_output_kind_and_suffix_are_one_typed_contract(
    platform: str, object_suffix: str, archive_suffix: str
) -> None:
    target = resolve_native_target_spec(
        None, host_platform=platform, host_arch="x86_64"
    )
    assert NativeArtifactKind.for_emit_mode("obj") is NativeArtifactKind.OBJECT
    assert NativeArtifactKind.for_emit_mode("bin") is NativeArtifactKind.ARCHIVE
    assert NativeArtifactKind.OBJECT.suffix(target) == object_suffix
    assert NativeArtifactKind.ARCHIVE.suffix(target) == archive_suffix
    with pytest.raises(ValueError, match="emit mode"):
        NativeArtifactKind.for_emit_mode("wasm")


@pytest.mark.parametrize(
    ("platform", "prefix", "suffix"),
    [
        ("linux", ("-Xlinker", "--whole-archive"), ("-Xlinker", "--no-whole-archive")),
        ("darwin", ("-Xlinker", "-force_load", "-Xlinker"), ()),
        ("win32", ("-Xlinker",), ()),
    ],
)
def test_archive_arguments_preserve_paths_and_target_dialect(
    platform, prefix, suffix
) -> None:
    path = Path("compiler, inputs") / "unit without extension"
    target = resolve_native_target_spec(
        None, host_platform=platform, host_arch="x86_64"
    )
    argument = f"/WHOLEARCHIVE:{path}" if platform == "win32" else str(path)
    assert native_artifact_link_arguments(
        path, kind=NativeArtifactKind.ARCHIVE, target=target
    ) == (*prefix, argument, *suffix)
    assert native_artifact_link_arguments(
        path, kind=NativeArtifactKind.OBJECT, target=target
    ) == (str(path),)
    with pytest.raises(ValueError, match="artifact kind"):
        native_artifact_link_arguments(path, kind="archive", target=target)  # type: ignore[arg-type]


def _object_header(platform: str, *, executable: bool = False) -> bytes:
    if platform == "linux":
        header = elf_header(kind=2 if executable else 1)
    elif platform == "darwin":
        header = macho_header(kind=2 if executable else 1)
    else:
        header = pe_header() if executable else coff_header()
    return bytes(header)


@pytest.mark.parametrize("platform", ["linux", "darwin", "win32"])
def test_object_container_validation_rejects_images_archives_and_truncation(
    tmp_path, platform
) -> None:
    target = resolve_native_target_spec(
        None, host_platform=platform, host_arch="x86_64"
    )
    path = tmp_path / "output.a"
    path.write_bytes(_object_header(platform))
    validate_native_object_artifact(path, target)
    for invalid in (
        _object_header(platform, executable=True),
        b"!<arch>\n",
        b"!<thin>\n",
        _object_header(platform)[:8],
        b"MZ" + bytes(62),
        *(
            _object_header(other)
            for other in ("linux", "darwin", "win32")
            if other != platform
        ),
    ):
        path.write_bytes(invalid)
        with pytest.raises(RuntimeError, match="not a relocatable"):
            validate_native_object_artifact(path, target)


def test_object_output_rejects_split_stdlib_without_subprocess_or_mutation(
    tmp_path, monkeypatch
) -> None:
    output = tmp_path / "output.obj"
    output.write_bytes(_object_header("win32"))
    monkeypatch.setattr(
        link_pipeline,
        "_run_native_link_command",
        lambda **kwargs: pytest.fail("object emission must not invoke linker"),
    )
    artifact, failure = link_pipeline._prepare_native_object_artifact(
        output_artifact=output,
        stdlib_obj_path=tmp_path / "missing.lib",
        json_output=True,
        target_triple="x86_64-pc-windows-msvc",
    )
    assert artifact is None and failure is not None
    assert output.read_bytes() == _object_header("win32")
    artifact, failure = link_pipeline._prepare_native_object_artifact(
        output_artifact=output,
        stdlib_obj_path=None,
        json_output=True,
        target_triple="x86_64-pc-windows-msvc",
    )
    assert artifact == output and failure is None
