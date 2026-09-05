"""Synthetic binary/loader proofs; no host compiler or live runtime capture."""

from __future__ import annotations

import struct
from pathlib import Path

import pytest

from molt import python_native_dependency_custody as native
from molt.python_file_node_custody import _FileNodePool
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_native_locations import _native_contract_valid


def _pe_image(
    name: bytes = b"KERNEL32.dll", *, delay: bool = False, va: bool = False
) -> bytearray:
    image = bytearray(0x600)
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, 0x80)
    image[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", image, 0x84, 0x8664, 1)
    struct.pack_into("<H", image, 0x94, 240)
    optional = 0x98
    struct.pack_into("<H", image, optional, 0x20B)
    struct.pack_into("<Q", image, optional + 24, 0x400000)
    struct.pack_into("<I", image, optional + 108, 16)
    struct.pack_into("<IIII", image, optional + 240 + 8, 0x400, 0x1000, 0x300, 0x200)
    if delay:
        struct.pack_into("<II", image, optional + 112 + 13 * 8, 0x1040, 64)
        struct.pack_into(
            "<II", image, 0x240, 0 if va else 1, 0x401080 if va else 0x1080
        )
    else:
        struct.pack_into("<II", image, optional + 112 + 8, 0x1000, 40)
        struct.pack_into("<IIIII", image, 0x200, 0, 0, 0, 0x1080, 0)
    image[0x280 : 0x280 + len(name) + 1] = name + b"\0"
    return image


def _elf_image(name: bytes = b"libc.so") -> bytearray:
    image = bytearray(0x400)
    image[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", image, 18, 62)
    struct.pack_into("<Q", image, 32, 64)
    struct.pack_into("<HH", image, 54, 56, 2)
    struct.pack_into("<IIQQQQQQ", image, 64, 1, 0, 0, 0, 0, len(image), len(image), 1)
    struct.pack_into("<IIQQQQQQ", image, 120, 2, 0, 0x200, 0x200, 0, 64, 64, 8)
    struct.pack_into("<qQ", image, 0x200, 1, 1)
    struct.pack_into("<qQ", image, 0x210, 5, 0x300)
    strings = b"\0" + name + b"\0"
    struct.pack_into("<qQ", image, 0x220, 10, len(strings))
    image[0x300 : 0x300 + len(strings)] = strings
    return image


def _macho_image(
    *, cpu: int = 0x01000007, name: bytes = b"/usr/lib/libSystem.B.dylib"
) -> bytearray:
    command_size = (24 + len(name) + 1 + 7) & ~7
    image = bytearray(32 + command_size)
    image[:4] = b"\xcf\xfa\xed\xfe"
    struct.pack_into("<I", image, 4, cpu)
    struct.pack_into("<II", image, 16, 1, command_size)
    struct.pack_into("<III", image, 32, 0x80000018, command_size, 24)
    image[56 : 56 + len(name)] = name
    return image


def _fat_macho(*, endian: str = ">", fat64: bool = False) -> bytearray:
    image = bytearray(0x300)
    struct.pack_into(endian + "II", image, 0, 0xCAFEBABF if fat64 else 0xCAFEBABE, 2)
    row_format = endian + ("IIQQII" if fat64 else "IIIII")
    for index, (cpu, name) in enumerate(
        ((0x01000007, b"@rpath/x86.dylib"), (0x0100000C, b"@rpath/arm.dylib"))
    ):
        thin = _macho_image(cpu=cpu, name=name)
        start = 0x100 * (index + 1)
        values = (cpu, 0, start, len(thin), 8) + ((0,) if fat64 else ())
        struct.pack_into(
            row_format, image, 8 + index * struct.calcsize(row_format), *values
        )
        image[start : start + len(thin)] = thin
    return image


@pytest.mark.parametrize("delay,va", [(False, False), (True, False), (True, True)])
def test_pe_import_and_delay_import_address_modes(delay: bool, va: bool) -> None:
    image = bytes(_pe_image(delay=delay, va=va))
    assert native._native_dependency_names(image, "windows", architecture="x86_64") == (
        "kernel32.dll",
    )


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<H", 0x94, 1, "truncated PE header"),
        ("<I", 0x98 + 108, 17, "directories are truncated"),
        ("<I", 0x98 + 112 + 12, 20, "import table is unterminated"),
        ("<I", 0x98 + 112 + 12, 0, "directory is incomplete"),
        ("<I", 0x200 + 12, 0x1320, "invalid RVA"),
        ("<I", 0x98 + 240 + 16, 0x500, "raw data is truncated"),
    ],
)
def test_pe_rejects_malformed_extents(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _pe_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._pe_dependency_names(bytes(image))


def test_pe_name_cannot_use_zero_fill_or_terminate_outside_raw_section() -> None:
    image = _pe_image()
    image[0x280:0x500] = b"x" * (0x500 - 0x280)
    with pytest.raises(PythonEnvironmentIdentityError, match="name is unterminated"):
        native._pe_dependency_names(bytes(image))


@pytest.mark.parametrize("attributes", [2, 3, 0xFFFFFFFF])
def test_pe_rejects_unknown_delay_import_attributes(attributes: int) -> None:
    image = _pe_image(delay=True)
    struct.pack_into("<I", image, 0x240, attributes)
    with pytest.raises(PythonEnvironmentIdentityError, match="attributes are invalid"):
        native._pe_dependency_names(bytes(image))


@pytest.mark.parametrize(
    "name", [b"libc.so", b"/loader/bound/lib.so", b"relative/lib.so"]
)
def test_elf_preserves_loader_name_including_path_qualified_needed(name: bytes) -> None:
    assert native._native_dependency_names(
        bytes(_elf_image(name)), "linux", architecture="x86_64"
    ) == (name.decode(),)


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<H", 54, 55, "program-header extent"),
        ("<Q", 120 + 32, 65, "dynamic table extent"),
        ("<q", 0x230, 1, "dynamic table is unterminated"),
        ("<q", 0x220, 5, "string table is duplicated"),
        ("<Q", 64 + 32, 0x300, "outside loaded segments"),
        ("<Q", 0x200 + 8, 0x400, "name is invalid"),
    ],
)
def test_elf_rejects_malformed_dynamic_metadata(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _elf_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._elf_dependency_names(bytes(image))


def test_elf_requires_name_termination_inside_string_table() -> None:
    image = _elf_image()
    image[0x308] = ord("x")
    with pytest.raises(PythonEnvironmentIdentityError, match="name is invalid"):
        native._elf_dependency_names(bytes(image))


@pytest.mark.parametrize("endian", ["<", ">"])
@pytest.mark.parametrize("fat64", [False, True])
@pytest.mark.parametrize(
    "architecture,name", [("x86_64", "@rpath/x86.dylib"), ("arm64", "@rpath/arm.dylib")]
)
def test_macho_universal_selects_only_explicit_architecture(
    endian: str, fat64: bool, architecture: str, name: str
) -> None:
    assert native._native_dependency_names(
        bytes(_fat_macho(endian=endian, fat64=fat64)),
        "macos",
        architecture=architecture,
    ) == (name,)


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<I", 32 + 4, 8, "dylib command is truncated"),
        ("<I", 32 + 8, 23, "name is invalid"),
        ("<I", 16, 0, "count/extent disagree"),
        ("<I", 20, 0x1000, "truncated load commands"),
    ],
)
def test_macho_rejects_malformed_commands(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _macho_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._macho_dependency_names(bytes(image), architecture="x86_64")


def test_macho_universal_rejects_overlapping_slices() -> None:
    image = _fat_macho()
    struct.pack_into(">I", image, 28 + 8, 0x100)
    with pytest.raises(PythonEnvironmentIdentityError, match="slice extent"):
        native._macho_dependency_names(bytes(image), architecture="arm64")


def test_macho_universal_rejects_slice_header_architecture_disagreement() -> None:
    image = _fat_macho()
    struct.pack_into("<I", image, 0x100 + 4, 0x0100000C)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="does not match runtime architecture"
    ):
        native._macho_dependency_names(bytes(image), architecture="x86_64")


def test_macho_universal_never_guesses_host_architecture() -> None:
    with pytest.raises(
        PythonEnvironmentIdentityError, match="explicit runtime architecture"
    ):
        native._macho_dependency_names(bytes(_fat_macho()))


@pytest.mark.parametrize(
    "operating_system,image",
    [
        ("windows", bytes(_pe_image())),
        ("linux", bytes(_elf_image())),
        ("macos", bytes(_macho_image())),
    ],
)
def test_native_images_reject_wrong_runtime_architecture(
    operating_system: str, image: bytes
) -> None:
    with pytest.raises(
        PythonEnvironmentIdentityError, match="does not match runtime architecture"
    ):
        native._native_dependency_names(image, operating_system, architecture="arm64")


@pytest.mark.parametrize(
    "operating_system", ["windows", "linux", "macos", "unsupported"]
)
@pytest.mark.parametrize("data", [b"", b"MZ", b"\x7fELF", b"\xcf\xfa\xed\xfe"])
def test_truncated_headers_raise_custody_error_not_struct_error(
    operating_system: str, data: bytes
) -> None:
    with pytest.raises(PythonEnvironmentIdentityError):
        native._native_dependency_names(data, operating_system, architecture="x86_64")


@pytest.mark.parametrize(
    "operating_system,contract,valid",
    [
        ("windows", "windows-api-set:api-ms-win-core-file-l1-1-0.dll", True),
        ("windows", "windows-api-set:ext-ms-win-ntuser-window-l1-1-0.dll", True),
        ("windows", "windows-system-import:missing.dll", False),
        ("windows", "windows-api-set:api-ms-arbitrary.dll", False),
        ("windows", "windows-api-set:api-ms-../evil-l1-1-0.dll", False),
        ("linux", "linux-loader-image:linux-vdso.so.1", True),
        ("linux", "linux-loader-image:arbitrary.so", False),
        ("macos", "macos-dyld-cache-image:libSystem.B.dylib", True),
        ("macos", "macos-dyld-cache-image:../libSystem.B.dylib", False),
        ("linux", "macos-dyld-cache-image:libSystem.B.dylib", False),
        ("windows", ["unhashable"], False),
    ],
)
def test_native_contracts_are_explicit_and_platform_gated(
    operating_system: str, contract: object, valid: bool
) -> None:
    assert _native_contract_valid(contract, operating_system) is valid


@pytest.mark.parametrize(
    "name,closed",
    [
        (b"missing-vendor.dll", False),
        (b"api-ms-not-a-contract.dll", False),
        (b"api-ms-win-core-file-l1-1-0.dll", True),
    ],
)
def test_closure_never_turns_arbitrary_missing_dll_into_system_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: bytes, closed: bool
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_pe_image(name))
    monkeypatch.setattr(
        native,
        "_loaded_native_module_paths",
        lambda _os: ((executable,), {"python.exe": executable}, ()),
    )
    if not closed:
        with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
            native._native_dependency_closure(
                {"base-executable": executable},
                operating_system="windows",
                architecture="x86_64",
                policy="pe-loaded-import-closure-v1",
                pool=_FileNodePool(),
            )
        return
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="windows",
        architecture="x86_64",
        policy="pe-loaded-import-closure-v1",
        pool=_FileNodePool(),
    )
    contract = "windows-api-set:" + name.decode()
    assert closure["contracts"] == [contract]
    assert closure["edges"] == [{"from": "native-component-0", "to": contract}]
    assert closure["status"] == "closed"
