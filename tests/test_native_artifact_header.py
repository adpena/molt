"""Synthetic header identities; no host compiler, loader or support-cell claim."""

from __future__ import annotations

from io import BytesIO
from pathlib import Path
import random
import struct
from types import SimpleNamespace

import pytest

from molt.cli.native_link_plan import validate_native_object_artifact
from molt.native_target import resolve_native_target_spec
from molt.cli.source_extension_toolchain import _source_extension_meson_host_machine
from molt.native_artifact_header import (
    LINKED_IMAGE_KINDS,
    LOADED_IMAGE_KINDS,
    OBJECT_KINDS,
    MachOHeader,
    NativeArtifactError,
    NativeReader,
    decode_native_artifact,
    macho_code_signature_from_bytes,
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
    signed_macho_image,
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


def test_code_directory_names_the_identity_and_the_signed_extent() -> None:
    signature = macho_code_signature_from_bytes(bytes(signed_macho_image("app")))
    assert signature is not None
    assert (signature.identifier, signature.flags) == ("app", 0x20002)
    assert (signature.code_limit, signature.signature_offset) == (8192, 8192)
    assert (signature.code_slots, signature.page_size) == (2, 4096)
    assert signature.ad_hoc and signature.covers_image
    assert macho_code_signature_from_bytes(bytes(signed_macho_image(None))) is None
    # A directory whose slots stop short of the image does not cover it.
    short = macho_code_signature_from_bytes(
        bytes(signed_macho_image("app", code_slots=1))
    )
    assert short is not None and not short.covers_image
    unsigned_flags = macho_code_signature_from_bytes(
        bytes(signed_macho_image("app", flags=0))
    )
    assert unsigned_flags is not None and not unsigned_flags.ad_hoc


def _corrupt(image: bytearray, offset: int, fmt: str, value: int) -> bytes:
    struct.pack_into(fmt, image, offset, value)
    return bytes(image)


def _unterminated_identifier(image: bytearray) -> bytes:
    directory = 8192 + 20
    (hash_offset,) = struct.unpack_from(">I", image, directory + 16)
    image[directory + 88 : directory + hash_offset] = b"A" * (hash_offset - 88)
    return bytes(image)


@pytest.mark.parametrize(
    "mutate,message",
    [
        # Load command size beyond the declared command bytes.
        (lambda image: _corrupt(image, 36, "<I", 64), "load command size"),
        # Signature extent beyond the end of the file.
        (lambda image: _corrupt(image, 44, "<I", 1 << 20), "truncated"),
        (lambda image: _corrupt(image, 8192, ">I", 0xFADE0C01), "superblob"),
        (lambda image: _corrupt(image, 8192 + 12, ">I", 5), "single code directory"),
        (lambda image: _corrupt(image, 8192 + 20, ">I", 0xFADE0C01), "code directory"),
        # Identifier offset past the end of the code directory.
        (lambda image: _corrupt(image, 8192 + 20 + 20, ">I", 4096), "out of range"),
        # The identifier runs to the end of the code directory.
        (_unterminated_identifier, "unterminated"),
    ],
)
def test_code_directory_decoding_is_bounded(mutate, message: str) -> None:
    with pytest.raises(NativeArtifactError, match=message):
        macho_code_signature_from_bytes(mutate(signed_macho_image("app")))


def test_code_signature_belongs_to_one_thin_macho_image() -> None:
    for payload in (elf_header(), fat_macho((signed_macho_image("app"),))):
        with pytest.raises(NativeArtifactError, match="one thin Mach-O image"):
            macho_code_signature_from_bytes(bytes(payload))


@pytest.mark.parametrize(
    ("offset", "fmt", "value", "message"),
    [
        (16, ">I", 0xFFFFFFFF, "hash array"),
        (16, ">I", 0, "special hashes"),
        (24, ">I", 3, "special hashes"),
        (28, ">I", 0xFFFFFFFF, "hash array"),
        (36, "B", 0, "hash type and size"),
        (36, "B", 20, "hash type and size"),
        (37, "B", 255, "hash type and size"),
        (20, ">I", 12, "identifier is out of range"),
        (20, ">I", 92, "identifier overlaps hashes"),
        (44, ">I", 4, "scatter overlaps its header"),
        (44, ">I", 156, "scatter.*truncated"),
        (44, ">I", 92, "scatter overlaps hashes"),
        (8, ">I", 0x10000, "code directory is invalid"),
        (8, ">I", 0x30000, "code directory is invalid"),
    ],
)
def test_code_directory_rejects_false_coverage_fields(offset, fmt, value, message):
    # Independent literal ABI offsets, not offsets/widths obtained from the decoder.
    image = signed_macho_image("app")
    struct.pack_into(fmt, image, 8212 + offset, value)
    with pytest.raises(NativeArtifactError, match=message):
        macho_code_signature_from_bytes(bytes(image))


@pytest.mark.parametrize(
    ("version", "minimum"),
    [
        (0x20001, 44),
        (0x20100, 48),
        (0x20200, 52),
        (0x20300, 64),
        (0x20400, 88),
        (0x20500, 96),
        (0x20600, 108),
    ],
)
def test_code_directory_requires_its_versioned_fixed_fields(version, minimum):
    image = signed_macho_image("app")
    struct.pack_into(">II", image, 8216, minimum - 1, version)
    with pytest.raises(NativeArtifactError, match="versioned code directory header"):
        macho_code_signature_from_bytes(bytes(image))


def _replace_code_directory(image: bytearray, directory: bytes) -> bytes:
    # One index at 8204; its payload starts at 8212. Adjust all enclosing lengths.
    image[8212:] = directory
    struct.pack_into(">I", image, 8216, len(directory))
    struct.pack_into(">I", image, 8196, 20 + len(directory))
    struct.pack_into("<I", image, 44, 20 + len(directory))
    return bytes(image)


def test_claimed_code_slots_need_real_hash_array_bytes():
    image = signed_macho_image("app")
    # Both page hashes are absent; limit/slot arithmetic still describes 8192 bytes.
    image = _replace_code_directory(image, image[8212 : 8212 + 92])
    with pytest.raises(NativeArtifactError, match="hash array"):
        macho_code_signature_from_bytes(image)


@pytest.mark.parametrize(("kind", "width"), [(1, 20), (2, 32), (3, 20), (4, 48)])
def test_declared_digest_widths_and_special_slots_are_admitted(kind, width):
    image = signed_macho_image("app")
    directory = bytearray(image[8212 : 8212 + 92])
    # Two special hashes precede two ordinary hashes, all inside the directory.
    struct.pack_into(">I", directory, 16, 92 + 2 * width)
    struct.pack_into(">I", directory, 24, 2)
    struct.pack_into("BB", directory, 36, width, kind)
    directory.extend(bytes(4 * width))
    signature = macho_code_signature_from_bytes(
        _replace_code_directory(image, directory)
    )
    assert signature is not None and signature.covers_image


def test_scattered_directory_does_not_claim_contiguous_image_coverage():
    image = signed_macho_image("app")
    directory = bytearray(image[8212:])
    struct.pack_into(">I", directory, 44, len(directory))
    # A valid terminated vector which starts at page 7, not page zero.
    directory.extend(struct.pack(">IIQQ", 2, 7, 0, 0))
    directory.extend(bytes(24))
    signature = macho_code_signature_from_bytes(
        _replace_code_directory(image, directory)
    )
    assert signature is not None and signature.scattered and not signature.covers_image


def test_infinite_page_size_describes_one_hash_slot():
    image = signed_macho_image("app", code_slots=1)
    image[8212 + 39] = 0  # cs_blobs.h: pageSize=0 denotes one unlimited page.
    signature = macho_code_signature_from_bytes(bytes(image))
    assert signature is not None and signature.covers_image


@pytest.mark.parametrize("blob_offset", [12, 8192])
def test_signature_index_cannot_alias_index_or_escape_superblob(blob_offset):
    image = signed_macho_image("app")
    struct.pack_into(">I", image, 8208, blob_offset)
    with pytest.raises(NativeArtifactError, match="overlaps its index|truncated"):
        macho_code_signature_from_bytes(bytes(image))


def test_signature_identifier_scan_does_not_read_the_hash_array():
    from molt import native_artifact_header as headers

    image = signed_macho_image("app")
    directory = bytearray(image[8212:])
    directory.extend(bytes(1024 * 1024))
    payload = _replace_code_directory(image, directory)
    reads: list[tuple[int, int]] = []

    def read_at(offset, size):
        reads.append((offset, size))
        return payload[offset : offset + size]

    signature = headers._thin_macho_code_signature(NativeReader(len(payload), read_at))
    assert signature is not None and signature.identifier == "app"
    assert max(size for _, size in reads) <= 4096
    assert not any(offset < 8368 and offset + size > 8304 for offset, size in reads)


def test_signature_duplicate_index_stops_without_scanning_the_remaining_table():
    from molt import native_artifact_header as headers

    image = signed_macho_image("app")
    directory = bytes(image[8212:])
    count = 1_000_000
    directory_offset = 12 + 8 * count
    blob_length = directory_offset + len(directory)
    header = bytearray(image[:48])
    struct.pack_into("<I", header, 44, blob_length)
    blocks = {
        0: bytes(header),
        8192: struct.pack(">III", 0xFADE0CC0, blob_length, count),
        8204: struct.pack(">II", 0, directory_offset),
        8212: struct.pack(">II", 0, directory_offset),
        8192 + directory_offset: directory,
    }
    reads: list[tuple[int, int]] = []

    def read_at(offset, size):
        reads.append((offset, size))
        for start, data in blocks.items():
            if start <= offset and offset + size <= start + len(data):
                return data[offset - start : offset - start + size]
        raise AssertionError(f"unnecessary read after duplicate index: {offset}/{size}")

    with pytest.raises(NativeArtifactError, match="single code directory"):
        headers._thin_macho_code_signature(NativeReader(8192 + blob_length, read_at))
    assert len(reads) < 20


@pytest.mark.parametrize("terminated", [True, False])
def test_long_identifier_is_read_in_bounded_chunks(terminated):
    from molt import native_artifact_header as headers

    name = "n" * 9000
    image = signed_macho_image(name)
    if not terminated:
        image[8212 + 88 + len(name)] = ord("n")
    reads: list[int] = []

    def read_at(offset, size):
        assert size <= 4096
        reads.append(size)
        return bytes(image[offset : offset + size])

    reader = NativeReader(len(image), read_at)
    if terminated:
        signature = headers._thin_macho_code_signature(reader)
        assert signature is not None and signature.identifier == name
    else:
        with pytest.raises(NativeArtifactError, match="unterminated"):
            headers._thin_macho_code_signature(reader)
    assert len(reads) < 20
