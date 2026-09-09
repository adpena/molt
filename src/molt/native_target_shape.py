"""Known native artifact encodings, not a compiler support matrix.

Backend/toolchain capability admission remains authoritative. These shape facts
let every artifact consumer check the same explicit architecture/ABI identity.
Numeric encodings follow ELF e_machine, PE/COFF Machine and Mach-O cpu_type_t.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum
import re
from typing import Literal


ByteOrder = Literal["little", "big"]


class NativeObjectFormat(str, Enum):
    ELF = "elf"
    MACHO = "macho"
    COFF = "coff"


@dataclass(frozen=True, slots=True)
class NativeArtifactShape:
    architecture: str
    header_bits: int
    pointer_bits: int
    byte_order: ByteOrder
    meson_cpu_family: str
    elf_machine: int | None
    coff_machines: tuple[int, ...] = ()
    macho_cpu: int | None = None
    macho_subtype: int | None = None
    macho_runtime_family: bool = True
    elf_flags_mask: int = 0
    elf_flags_value: int = 0

    def machines(self, object_format: NativeObjectFormat) -> tuple[int, ...]:
        if object_format is NativeObjectFormat.ELF:
            return () if self.elf_machine is None else (self.elf_machine,)
        if object_format is NativeObjectFormat.MACHO:
            return () if self.macho_cpu is None else (self.macho_cpu,)
        return self.coff_machines


_ARCH_ALIASES = {
    "amd64": "x86_64",
    "x64": "x86_64",
    "x86-64": "x86_64",
    "arm64": "aarch64",
    "arm64_32": "aarch64_32",
    "ppc": "powerpc",
    "ppc64": "powerpc64",
    "ppc64le": "powerpc64le",
    "sparcv9": "sparc64",
}
# Rows describe encodings known by the native backend/source-extension toolchain
# families; they do not enable a backend, operating system, ABI or release cell.
_SHAPES = {
    row.architecture: row
    for row in (
        NativeArtifactShape(
            "x86_64", 64, 64, "little", "x86_64", 62, (0x8664,), 0x01000007, 3
        ),
        NativeArtifactShape(
            "x86_64h", 64, 64, "little", "x86_64", None, (), 0x01000007, 8, False
        ),
        NativeArtifactShape("x86", 32, 32, "little", "x86", 3, (0x014C,), 7, 3),
        NativeArtifactShape(
            "aarch64", 64, 64, "little", "aarch64", 183, (0xAA64,), 0x0100000C, 0
        ),
        NativeArtifactShape("aarch64_be", 64, 64, "big", "aarch64", 183),
        NativeArtifactShape(
            "aarch64_32", 64, 32, "little", "aarch64", None, (), 0x0200000C, 1
        ),
        NativeArtifactShape(
            "arm64e", 64, 64, "little", "aarch64", None, (), 0x0100000C, 2, False
        ),
        NativeArtifactShape("arm64ec", 64, 64, "little", "aarch64", None, (0xA641,)),
        NativeArtifactShape("arm64x", 64, 64, "little", "aarch64", None, (0xA64E,)),
        NativeArtifactShape(
            "arm", 32, 32, "little", "arm", 40, (0x01C0, 0x01C2, 0x01C4), 12, 0
        ),
        NativeArtifactShape("armeb", 32, 32, "big", "arm", 40),
        NativeArtifactShape("riscv32", 32, 32, "little", "riscv32", 243),
        NativeArtifactShape("riscv64", 64, 64, "little", "riscv64", 243),
        NativeArtifactShape("s390x", 64, 64, "big", "s390x", 22),
        NativeArtifactShape("powerpc", 32, 32, "big", "ppc", 20, (), 18, 0),
        NativeArtifactShape("powerpcle", 32, 32, "little", "ppc", 20),
        NativeArtifactShape("powerpc64", 64, 64, "big", "ppc64", 21, (), 0x01000012, 0),
        NativeArtifactShape("powerpc64le", 64, 64, "little", "ppc64", 21),
        NativeArtifactShape("sparc", 32, 32, "big", "sparc", 2, (), 14, 0),
        NativeArtifactShape("sparc64", 64, 64, "big", "sparc64", 43),
        NativeArtifactShape("mips", 32, 32, "big", "mips", 8),
        NativeArtifactShape("mipsel", 32, 32, "little", "mips", 8),
        NativeArtifactShape("mips64", 64, 64, "big", "mips64", 8),
        NativeArtifactShape("mips64el", 64, 64, "little", "mips64", 8),
        NativeArtifactShape("loongarch32", 32, 32, "little", "loongarch32", 258),
        NativeArtifactShape("loongarch64", 64, 64, "little", "loongarch64", 258),
        NativeArtifactShape("m68k", 32, 32, "big", "m68k", 4, (), 6, 1),
    )
}


def normalize_native_architecture(raw: str) -> str:
    normalized = raw.strip().lower()
    arch = _ARCH_ALIASES.get(normalized, normalized)
    if not re.fullmatch(r"[a-z0-9_]+", arch) or arch == "unknown":
        raise RuntimeError(f"Native target has no valid architecture: {raw!r}.")
    return arch


def native_artifact_shape(
    architecture: str,
    *,
    target_triple: str | None = None,
    object_format: NativeObjectFormat | None = None,
) -> NativeArtifactShape:
    """Resolve a known encoding without granting target/toolchain support."""
    arch = normalize_native_architecture(architecture)
    family = arch
    if re.fullmatch(r"i[3-6]86", arch):
        family = "x86"
    elif re.fullmatch(r"(?:arm|thumb)v[4-9][a-z0-9_]*", arch):
        family = "armeb" if arch.endswith("eb") else "arm"
    elif re.fullmatch(r"(?:armeb|thumbeb)v[4-9][a-z0-9_]*", arch):
        family = "armeb"
    elif re.fullmatch(r"riscv(?:32|64)[a-z0-9_]*", arch):
        family = "riscv32" if arch.startswith("riscv32") else "riscv64"
    try:
        shape = _SHAPES[family]
    except KeyError as exc:
        raise RuntimeError(
            f"No native artifact identity shape is known for architecture {arch!r}; "
            "this is an encoding gate, not a host fallback or support claim."
        ) from exc
    if family in {"mips", "mipsel", "mips64", "mips64el"}:
        shape = replace(shape, elf_flags_mask=0x20)  # EF_MIPS_ABI2 distinguishes n32.
    if target_triple is not None:
        abi = target_triple.lower().split("-")[-1]
        if abi in {"gnux32", "muslx32"}:
            if family != "x86_64" or object_format not in (
                None,
                NativeObjectFormat.ELF,
            ):
                raise RuntimeError(
                    f"x32 artifact ABI conflicts with {target_triple!r}."
                )
            shape = replace(shape, header_bits=32, pointer_bits=32)
        elif abi in {"gnuabin32", "muslabin32"}:
            if family not in {"mips64", "mips64el"} or object_format not in (
                None,
                NativeObjectFormat.ELF,
            ):
                raise RuntimeError(
                    f"MIPS n32 artifact ABI conflicts with {target_triple!r}."
                )
            shape = replace(
                shape, header_bits=32, pointer_bits=32, elf_flags_value=0x20
            )
        elif abi in {"ilp32", "gnu_ilp32", "musl_ilp32"}:
            if family not in {"aarch64", "aarch64_be"} or object_format not in (
                None,
                NativeObjectFormat.ELF,
            ):
                raise RuntimeError(
                    f"AArch64 ILP32 artifact ABI conflicts with {target_triple!r}."
                )
            shape = replace(shape, header_bits=32, pointer_bits=32)
    if family == "arm" and arch != "arm" and object_format is NativeObjectFormat.MACHO:
        subtype = {
            "armv6": 6,
            "armv7": 9,
            "armv7s": 11,
            "armv7k": 12,
            "armv8": 13,
            "armv6m": 14,
            "armv7m": 15,
            "armv7em": 16,
            "thumbv6m": 14,
            "thumbv7m": 15,
            "thumbv7em": 16,
        }.get(arch)
        if subtype is None:
            raise RuntimeError(f"No exact Mach-O subtype is known for {arch!r}.")
        shape = replace(shape, macho_subtype=subtype, macho_runtime_family=False)
    if object_format is not None and not shape.machines(object_format):
        raise RuntimeError(
            f"No {object_format.value} artifact identity encoding is known for {arch!r}."
        )
    return shape


def coff_machine_bits(machine: int) -> int:
    bits = {
        shape.header_bits
        for shape in _SHAPES.values()
        if machine in shape.coff_machines
    }
    if len(bits) != 1:
        raise RuntimeError(
            f"Unknown or ambiguous COFF artifact machine 0x{machine:04x}."
        )
    return next(iter(bits))


def native_object_format_for_os(operating_system: str) -> NativeObjectFormat:
    try:
        return {
            "windows": NativeObjectFormat.COFF,
            "linux": NativeObjectFormat.ELF,
            "macos": NativeObjectFormat.MACHO,
        }[operating_system]
    except KeyError as exc:
        raise RuntimeError(
            f"Native artifact format is unsupported on {operating_system!r}."
        ) from exc
