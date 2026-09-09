"""Synthetic header identities; no host compiler, loader or support-cell claim."""

from __future__ import annotations

from io import BytesIO
from pathlib import Path
import random
import struct
from types import SimpleNamespace

import pytest

from molt.cli.native_link_plan import (
    resolve_native_target_spec,
    validate_native_object_artifact,
)
from molt.cli.source_extension_toolchain import _source_extension_meson_host_machine
from molt.native_artifact_header import (
    LINKED_IMAGE_KINDS,
    LOADED_IMAGE_KINDS,
    OBJECT_KINDS,
    MachOHeader,
    NativeArtifactError,
    NativeReader,
    decode_native_artifact,
    native_artifact_from_bytes,
    native_artifact_from_file,
)
from molt.native_target_shape import NativeObjectFormat, native_artifact_shape
from tests.native_artifact_fixtures import (
    coff_header,
    elf_header,
    fat_macho,
    macho_header,
    pe_header,
)


@pytest.mark.parametrize(
    "arch,machine,bits,endian",
    [
        ("x86_64", 62, 64, "<"),
        ("i686", 3, 32, "<"),
        ("aarch64", 183, 64, "<"),
        ("aarch64_be", 183, 64, ">"),
        ("armv7", 40, 32, "<"),
        ("armebv7", 40, 32, ">"),
        ("riscv32", 243, 32, "<"),
        ("riscv64gc", 243, 64, "<"),
        ("s390x", 22, 64, ">"),
        ("powerpc", 20, 32, ">"),
        ("powerpc64", 21, 64, ">"),
        ("powerpc64le", 21, 64, "<"),
        ("sparc", 2, 32, ">"),
        ("sparc64", 43, 64, ">"),
        ("mips", 8, 32, ">"),
        ("mipsel", 8, 32, "<"),
        ("mips64", 8, 64, ">"),
        ("mips64el", 8, 64, "<"),
        ("loongarch64", 258, 64, "<"),
    ],
)
def test_known_elf_shapes_preserve_crossarch_endianness(arch, machine, bits, endian):
    artifact = native_artifact_from_bytes(
        bytes(elf_header(machine=machine, bits=bits, endian=endian))
    )
    shape = native_artifact_shape(arch, object_format=NativeObjectFormat.ELF)
    header = artifact.admit(
        object_format=NativeObjectFormat.ELF,
        kinds=LINKED_IMAGE_KINDS,
        shape=shape,
        exact_target=True,
    )
    assert (header.machine, header.bits, header.endian) == (machine, bits, endian)
    wrong = native_artifact_shape(
        "aarch64" if machine != 183 else "x86_64", object_format=NativeObjectFormat.ELF
    )
    with pytest.raises(NativeArtifactError, match="runtime architecture"):
        artifact.admit(
            object_format=NativeObjectFormat.ELF,
            kinds=LINKED_IMAGE_KINDS,
            shape=wrong,
            exact_target=True,
        )


@pytest.mark.parametrize("bigobj", [False, True])
@pytest.mark.parametrize(
    "arch,machine",
    [
        ("x86_64", 0x8664),
        ("aarch64", 0xAA64),
        ("arm64ec", 0xA641),
        ("arm64x", 0xA64E),
        ("i686", 0x14C),
    ],
)
def test_coff_objects_use_same_machine_shape_authority(arch, machine, bigobj):
    artifact = native_artifact_from_bytes(
        bytes(coff_header(machine=machine, bigobj=bigobj))
    )
    artifact.admit(
        object_format=NativeObjectFormat.COFF,
        kinds=OBJECT_KINDS,
        shape=native_artifact_shape(arch, object_format=NativeObjectFormat.COFF),
        exact_target=True,
    )


@pytest.mark.parametrize(
    "triple,payload",
    [
        ("x86_64-unknown-linux-gnu", elf_header(kind=1)),
        ("s390x-unknown-linux-gnu", elf_header(kind=1, machine=22, endian=">")),
        ("aarch64-apple-darwin", macho_header(cpu=0x0100000C, kind=1)),
        ("aarch64_32-apple-darwin", macho_header(cpu=0x0200000C, kind=1)),
        ("arm64ec-pc-windows-msvc", coff_header(machine=0xA641, bigobj=True)),
    ],
)
def test_object_consumer_admits_real_target_shape(tmp_path: Path, triple, payload):
    path = tmp_path / "artifact"
    path.write_bytes(payload)
    validate_native_object_artifact(path, resolve_native_target_spec(triple))


@pytest.mark.parametrize("endian", ["<", ">"])
@pytest.mark.parametrize("fat64", [False, True])
def test_universal_count_offset_and_all_slice_headers(endian, fat64):
    payload = fat_macho(
        (macho_header(), macho_header(cpu=0x0100000C)), endian=endian, fat64=fat64
    )
    artifact = native_artifact_from_bytes(bytes(payload))
    assert len(artifact.headers) == 2
    for arch in ("x86_64", "aarch64"):
        shape = native_artifact_shape(arch, object_format=NativeObjectFormat.MACHO)
        selected = artifact.admit(
            object_format=NativeObjectFormat.MACHO,
            kinds=LOADED_IMAGE_KINDS,
            shape=shape,
        )
        assert selected.matches(shape, exact_target=False)
        with pytest.raises(NativeArtifactError):
            artifact.admit(
                object_format=NativeObjectFormat.MACHO,
                kinds=LINKED_IMAGE_KINDS,
                shape=shape,
                exact_target=True,
            )
    with pytest.raises(NativeArtifactError, match="explicit runtime architecture"):
        artifact.admit(object_format=NativeObjectFormat.MACHO, kinds=LOADED_IMAGE_KINDS)


def test_header_width_is_not_pointer_abi_and_exact_subtypes_are_not_family_matches():
    shape = native_artifact_shape("arm64_32", object_format=NativeObjectFormat.MACHO)
    assert (shape.header_bits, shape.pointer_bits) == (64, 32)
    native_artifact_from_bytes(bytes(macho_header(cpu=0x0200000C))).admit(
        object_format=NativeObjectFormat.MACHO,
        kinds=LINKED_IMAGE_KINDS,
        shape=shape,
        exact_target=True,
    )
    for cpu, specialized_subtype, generic in (
        (0x01000007, 8, "x86_64"),
        (0x0100000C, 2, "aarch64"),
    ):
        shape = native_artifact_shape(generic, object_format=NativeObjectFormat.MACHO)
        specialized = native_artifact_from_bytes(
            bytes(macho_header(cpu=cpu, subtype=specialized_subtype))
        )
        specialized.admit(
            object_format=NativeObjectFormat.MACHO,
            kinds=LOADED_IMAGE_KINDS,
            shape=shape,
        )
        with pytest.raises(NativeArtifactError, match="subtype"):
            specialized.admit(
                object_format=NativeObjectFormat.MACHO,
                kinds=LINKED_IMAGE_KINDS,
                shape=shape,
                exact_target=True,
            )


def test_universal_subtype_selection_never_chooses_first_family_slice():
    artifact = native_artifact_from_bytes(
        bytes(fat_macho((macho_header(), macho_header(subtype=8))))
    )
    shape = native_artifact_shape("x86_64", object_format=NativeObjectFormat.MACHO)
    with pytest.raises(NativeArtifactError, match="matching slices=2"):
        artifact.admit(
            object_format=NativeObjectFormat.MACHO,
            kinds=LOADED_IMAGE_KINDS,
            shape=shape,
        )


@pytest.mark.parametrize("specialized_first", [False, True])
def test_universal_runtime_uses_dyld_selected_slice_identity(
    specialized_first: bool,
) -> None:
    generic = macho_header(cpu=0x0100000C, subtype=0)
    arm64e = macho_header(cpu=0x0100000C, subtype=0x80000002)
    slices = (arm64e, generic) if specialized_first else (generic, arm64e)
    artifact = native_artifact_from_bytes(bytes(fat_macho(slices)))
    shape = native_artifact_shape("aarch64", object_format=NativeObjectFormat.MACHO)
    selected = artifact.admit(
        object_format=NativeObjectFormat.MACHO,
        kinds=LOADED_IMAGE_KINDS,
        shape=shape,
        loaded_macho_identity=(0x0100000C, 0x80000002),
    )
    assert isinstance(selected.metadata, MachOHeader)
    assert selected.metadata.subtype == 0x80000002


@pytest.mark.parametrize(
    "offset,fmt,value,reason",
    [
        (4, ">I", 0xFFFFFFFF, "bounded admission"),
        (16, ">I", 8, "slice extent"),
        (16, ">I", 0x101, "alignment"),
        (20, ">I", 0xFFFF, "truncated"),
        (24, ">I", 32, "alignment"),
        (0x200 + 4, "<I", 0x01000007, "identity disagreement"),
        (0x200 + 8, "<I", 2, "identity disagreement"),
        (36, ">I", 0x100, "slice extent"),
    ],
)
def test_universal_rejects_malformed_and_unselected_slice_metadata(
    offset, fmt, value, reason
):
    payload = fat_macho((macho_header(), macho_header(cpu=0x0100000C)))
    struct.pack_into(fmt, payload, offset, value)
    with pytest.raises(NativeArtifactError, match=reason):
        native_artifact_from_bytes(bytes(payload))


def test_universal_reserved_fields_duplicates_and_nested_containers_are_rejected():
    payload = fat_macho((macho_header(),), fat64=True)
    struct.pack_into(">I", payload, 8 + 28, 1)
    with pytest.raises(NativeArtifactError, match="slice extent"):
        native_artifact_from_bytes(bytes(payload))
    with pytest.raises(NativeArtifactError, match="duplicated"):
        native_artifact_from_bytes(bytes(fat_macho((macho_header(), macho_header()))))
    payload = fat_macho((macho_header(),))
    payload[0x100:0x104] = b"\xca\xfe\xba\xbe"
    with pytest.raises(NativeArtifactError, match="not a thin"):
        native_artifact_from_bytes(bytes(payload))


@pytest.mark.parametrize("bits,endian", [(32, "<"), (32, ">"), (64, "<"), (64, ">")])
def test_elf_extended_counts_use_bounded_section_zero(bits, endian):
    fixed = 64 if bits == 64 else 52
    section_size = 64 if bits == 64 else 40
    payload = elf_header(bits=bits, endian=endian, image_size=fixed + section_size)
    struct.pack_into(
        endian + ("Q" if bits == 64 else "I"), payload, 40 if bits == 64 else 32, fixed
    )
    struct.pack_into(endian + "H", payload, 58 if bits == 64 else 46, section_size)
    struct.pack_into(
        endian + ("Q" if bits == 64 else "I"),
        payload,
        fixed + (32 if bits == 64 else 20),
        1,
    )
    artifact = native_artifact_from_bytes(bytes(payload))
    assert artifact.headers[0].metadata.section_count == 1


def test_bounded_reader_never_reads_whole_large_file_or_allocates_declared_count():
    fixed = bytes(elf_header())
    requests = []

    def read(offset, size):
        requests.append((offset, size))
        return fixed[offset : offset + size]

    decode_native_artifact(NativeReader(1 << 40, read))
    assert sum(size for _, size in requests) == 68
    malicious = b"\xca\xfe\xba\xbe\xff\xff\xff\xff"
    with pytest.raises(NativeArtifactError, match="bounded admission"):
        decode_native_artifact(
            NativeReader(
                1 << 40, lambda offset, size: malicious[offset : offset + size]
            )
        )


def test_bytes_and_file_readers_share_result_and_bound_reads(tmp_path: Path):
    payload = bytes(pe_header())
    path = tmp_path / "image"
    path.write_bytes(payload)
    requests = []
    with path.open("rb") as stream:

        class Observed:
            def fileno(self):
                return stream.fileno()

            def seek(self, offset):
                return stream.seek(offset)

            def read(self, size):
                requests.append(size)
                assert 0 <= size <= 112
                return stream.read(size)

        assert native_artifact_from_file(Observed()) == native_artifact_from_bytes(
            payload
        )
    assert max(requests) <= 112
    with pytest.raises((OSError, ValueError)):
        native_artifact_from_file(BytesIO(payload))


def test_short_or_random_headers_raise_only_domain_errors():
    rng = random.Random(90117)
    for prefix in (
        b"MZ",
        b"\x7fELF",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbf",
        b"\0\0\xff\xff",
    ):
        for length in range(97):
            payload = (prefix + rng.randbytes(length))[:length]
            try:
                native_artifact_from_bytes(payload)
            except NativeArtifactError:
                pass


@pytest.mark.parametrize(
    "arch,abi,bits,endian,family",
    [
        ("x86_64", "gnux32", 32, "little", "x86_64"),
        ("s390x", "gnu", 64, "big", "s390x"),
        ("aarch64_be", "gnu", 64, "big", "aarch64"),
        ("powerpc64le", "gnu", 64, "little", "ppc64"),
        ("sparc64", "gnu", 64, "big", "sparc64"),
        ("i686", "gnu", 32, "little", "x86"),
    ],
)
def test_meson_endianness_projects_same_explicit_shape(arch, abi, bits, endian, family):
    target = resolve_native_target_spec(f"{arch}-unknown-linux-{abi}")
    shape = native_artifact_shape(
        target.arch, target_triple=target.triple, object_format=target.object_format
    )
    assert shape.header_bits == bits
    result = _source_extension_meson_host_machine(
        SimpleNamespace(is_wasm=False, native_target=target)
    )
    assert result["endian"] == endian
    assert result["cpu_family"] == family


def test_unknown_shape_is_an_explicit_encoding_gate_not_host_fallback():
    target = resolve_native_target_spec("futurecpu-unknown-linux-gnu")
    with pytest.raises(RuntimeError, match="identity shape"):
        native_artifact_shape(target.arch, object_format=target.object_format)
    with pytest.raises(RuntimeError, match="identity shape"):
        _source_extension_meson_host_machine(
            SimpleNamespace(is_wasm=False, native_target=target)
        )


def test_mips_n32_encoded_abi_flag_is_not_o32_header_identity():
    payload = elf_header(machine=8, bits=32, endian=">")
    n32 = native_artifact_shape(
        "mips64",
        target_triple="mips64-unknown-linux-gnuabin32",
        object_format=NativeObjectFormat.ELF,
    )
    artifact = native_artifact_from_bytes(bytes(payload))
    with pytest.raises(NativeArtifactError, match="flags"):
        artifact.admit(
            object_format=NativeObjectFormat.ELF,
            kinds=LINKED_IMAGE_KINDS,
            shape=n32,
            exact_target=True,
        )
    struct.pack_into(">I", payload, 36, 0x20)
    artifact = native_artifact_from_bytes(bytes(payload))
    artifact.admit(
        object_format=NativeObjectFormat.ELF,
        kinds=LINKED_IMAGE_KINDS,
        shape=n32,
        exact_target=True,
    )
    with pytest.raises(NativeArtifactError, match="flags"):
        artifact.admit(
            object_format=NativeObjectFormat.ELF,
            kinds=LINKED_IMAGE_KINDS,
            shape=native_artifact_shape("mips", object_format=NativeObjectFormat.ELF),
            exact_target=True,
        )


@pytest.mark.parametrize("endian", ["<", ">"])
@pytest.mark.parametrize("fat64", [False, True])
def test_singleton_universal_container_is_not_an_exact_thin_release(endian, fat64):
    artifact = native_artifact_from_bytes(
        bytes(fat_macho((macho_header(),), endian=endian, fat64=fat64))
    )
    shape = native_artifact_shape("x86_64", object_format=NativeObjectFormat.MACHO)
    artifact.admit(
        object_format=NativeObjectFormat.MACHO, kinds=LOADED_IMAGE_KINDS, shape=shape
    )
    with pytest.raises(NativeArtifactError, match="thin artifact"):
        artifact.admit(
            object_format=NativeObjectFormat.MACHO,
            kinds=LINKED_IMAGE_KINDS,
            shape=shape,
            exact_target=True,
        )


@pytest.mark.parametrize("cpu,bits", [(0x01000007, 32), (0x0200000C, 32), (7, 64)])
def test_macho_cpu_abi_cannot_disagree_with_header_width_without_a_target(cpu, bits):
    with pytest.raises(NativeArtifactError, match="header width disagree"):
        native_artifact_from_bytes(bytes(macho_header(cpu=cpu, bits=bits)))
