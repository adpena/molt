from __future__ import annotations

import inspect
import os
from pathlib import Path

import pytest
import molt.cli as cli
from molt.cli import native_binary, build_results, atomic_io
from molt.cli import native_link_plan
from tests.native_artifact_fixtures import (
    elf_header,
    pe_header,
    macho_header,
    fat_macho,
)

_NATIVE_BINARY_NAMES = (
    "_NativeBinaryInvalid",
    "_assert_native_binary_valid",
    "_darwin_binary_imports_validation_error",
    "_darwin_binary_magic_error",
    "_expected_binary_format_for_target",
    "_smoke_probe_native_binary",
    "_target_is_host_executable",
    "_validate_native_binary_format",
)

_NATIVE_BINARY_DEFINITIONS = (
    "class _NativeBinaryInvalid",
    "def _assert_native_binary_valid(",
    "def _darwin_binary_imports_validation_error(",
    "def _darwin_binary_magic_error(",
    "def _expected_binary_format_for_target(",
    "def _smoke_probe_native_binary(",
    "def _target_is_host_executable(",
    "def _validate_native_binary_format(",
)


def test_cli_native_binary_authority_is_single_home() -> None:
    for name in _NATIVE_BINARY_NAMES:
        assert getattr(cli, name) is getattr(native_binary, name)

    cli_source = inspect.getsource(cli)
    for marker in _NATIVE_BINARY_DEFINITIONS:
        assert marker not in cli_source


@pytest.mark.parametrize(
    "payload,target",
    [
        (macho_header(cpu=0x0100000C), "aarch64-apple-darwin"),
        (elf_header(), "x86_64-unknown-linux-gnu"),
        (pe_header(), "x86_64-pc-windows-msvc"),
        (elf_header(machine=243), "riscv64gc-unknown-linux-gnu"),
        (elf_header(machine=22, endian=">"), "s390x-unknown-linux-gnu"),
        (elf_header(machine=183, endian=">"), "aarch64_be-unknown-linux-gnu"),
        (elf_header(bits=32), "x86_64-unknown-linux-gnux32"),
        (macho_header(cpu=0x0200000C), "aarch64_32-apple-darwin"),
    ],
)
def test_native_binary_validation_admits_complete_target_header(
    tmp_path: Path, payload: bytes, target: str
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(payload)
    native_binary._validate_native_binary_format(binary, target)
    native_binary.validate_native_binary_architecture(binary, target)


def test_native_binary_validation_rejects_wrong_target_object_magic(tmp_path) -> None:
    binary = tmp_path / "not-windows.exe"
    binary.write_bytes(elf_header())
    with pytest.raises(native_binary._NativeBinaryInvalid, match="expected coff"):
        native_binary._validate_native_binary_format(binary, "x86_64-pc-windows-msvc")


@pytest.mark.parametrize("payload", [b"", b"MZ", b"\x7fELF", b"\xcf\xfa\xed\xfe"])
def test_native_binary_validation_rejects_truncated_outputs(tmp_path, payload) -> None:
    binary = tmp_path / "truncated"
    binary.write_bytes(payload)
    with pytest.raises(native_binary._NativeBinaryInvalid, match="truncated"):
        native_binary._validate_native_binary_format(binary, "x86_64-pc-windows-msvc")


def test_native_binary_validation_identifies_32_bit_macho_corruption(tmp_path) -> None:
    binary = tmp_path / "corrupt-macho"
    binary.write_bytes(macho_header(cpu=12, bits=32))
    with pytest.raises(
        native_binary._NativeBinaryInvalid, match="runtime architecture"
    ):
        native_binary._validate_native_binary_format(binary, "aarch64-apple-darwin")


@pytest.mark.parametrize(
    "payload,target",
    [
        (macho_header(cpu=0x0100000C, subtype=2), "aarch64-apple-darwin"),
        (macho_header(subtype=8), "x86_64-apple-darwin"),
        (macho_header(subtype=0x80000003), "x86_64-apple-darwin"),
        (pe_header(machine=0xA641), "aarch64-pc-windows-msvc"),
        (pe_header(dll=True), "x86_64-pc-windows-msvc"),
        (elf_header(kind=1), "x86_64-unknown-linux-gnu"),
        (elf_header(machine=183), "aarch64_be-unknown-linux-gnu"),
        (elf_header(), "x86_64-unknown-linux-gnux32"),
        (
            fat_macho((macho_header(), macho_header(cpu=0x0100000C))),
            "x86_64-apple-darwin",
        ),
    ],
)
def test_release_exact_target_never_accepts_wrong_kind_abi_or_subtype(
    tmp_path, payload, target
) -> None:
    binary = tmp_path / "candidate"
    binary.write_bytes(payload)
    with pytest.raises(native_binary._NativeBinaryInvalid):
        native_binary.validate_native_binary_architecture(binary, target)


def test_expected_binary_format_for_explicit_targets() -> None:
    assert (
        native_binary._expected_binary_format_for_target("aarch64-apple-darwin")
        == "macho"
    )
    assert (
        native_binary._expected_binary_format_for_target("x86_64-unknown-linux-gnu")
        == "elf"
    )
    assert (
        native_binary._expected_binary_format_for_target("x86_64-pc-windows-msvc")
        == "pe"
    )
    assert native_binary._target_is_host_executable("wasm32-wasi") is False
    for target in (
        "x86_64-unknown-notlinux-gnu",
        "x86_64-unknown-linux-windows",
        "aarch64-apple-ios",
    ):
        with pytest.raises(RuntimeError):
            native_binary._expected_binary_format_for_target(target)


def test_smoke_probe_requires_exact_host_shape_and_does_not_assume_rosetta(monkeypatch):
    monkeypatch.setattr(native_link_plan.sys, "platform", "darwin")
    monkeypatch.setattr(native_link_plan.platform, "machine", lambda: "arm64")
    assert native_binary._target_is_host_executable("aarch64-apple-darwin")
    assert not native_binary._target_is_host_executable("x86_64-apple-darwin")
    assert not native_binary._target_is_host_executable("aarch64_32-apple-darwin")


def test_darwin_post_link_hook_uses_same_bounded_header_authority(
    tmp_path, monkeypatch
):
    binary = tmp_path / "image"
    binary.write_bytes(macho_header())
    monkeypatch.setattr(native_link_plan.sys, "platform", "darwin")
    monkeypatch.setattr(native_link_plan.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(Path, "read_bytes", lambda path: pytest.fail("whole-file read"))
    assert native_binary._darwin_binary_magic_error(binary) is None
    binary.write_bytes(macho_header(cpu=0x0100000C))
    assert native_binary._darwin_binary_magic_error(binary) is None
    binary.write_bytes(elf_header())
    assert "expected macho" in native_binary._darwin_binary_magic_error(binary)


@pytest.mark.parametrize("valid", [False, True])
def test_actual_native_finalization_consumer_rejects_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, valid: bool
) -> None:
    candidate = tmp_path / "candidate"
    output = tmp_path / "published"
    payload = bytes(elf_header(kind=2 if valid else 1))
    candidate.write_bytes(payload)
    output.write_bytes(b"previous-generation")
    published = []
    monkeypatch.delenv("MOLT_SKIP_BINARY_VALIDITY_CHECK", raising=False)
    monkeypatch.delenv("MOLT_BUILD_SMOKE_EXEC", raising=False)

    def sign(source: Path) -> None:
        published.append(source)

    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", sign)
    timings: dict[str, int] = {}
    error = build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="x86_64-unknown-linux-gnu",
        strip=False,
        phase_times=timings,
    )
    assert {"strip_wall_ns", "validate_wall_ns", "publish_wall_ns"} <= timings.keys()
    assert all(value >= 0 for value in timings.values())
    if valid:
        assert error is None
        assert len(published) == 1 and published[0] != output
        assert output.read_bytes() == payload
        assert not candidate.exists()
    else:
        assert "native candidate validation failed" in error
        assert len(published) == 1 and published[0] != output
        assert output.read_bytes() == b"previous-generation"
        assert candidate.read_bytes() == payload


@pytest.mark.parametrize("alias", ["same", "relative", "hardlink"])
def test_finalization_rejects_public_candidate_aliases_before_strip(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, alias: str
) -> None:
    output = tmp_path / "published"
    output.write_bytes(b"public generation")
    candidate = output
    if alias == "relative":
        nested = tmp_path / "nested"
        nested.mkdir()
        candidate = nested / ".." / output.name
    elif alias == "hardlink":
        candidate = tmp_path / "candidate"
        os.link(output, candidate)
    monkeypatch.setattr(
        build_results,
        "_post_link_strip",
        lambda *_args: pytest.fail("must not strip a public or aliased candidate"),
    )
    error = build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="x86_64-unknown-linux-gnu",
        strip=True,
    )
    assert error is not None and "private candidate" in error
    assert output.read_bytes() == b"public generation"
