from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Sequence


class NativeObjectFormat(str, Enum):
    ELF = "elf"
    MACHO = "macho"
    COFF = "coff"


class NativeLinkerKind(str, Enum):
    SYSTEM = "system"
    LLD = "lld"
    MOLD = "mold"


_ARCH_ALIASES = {
    "amd64": "x86_64",
    "x64": "x86_64",
    "arm64": "aarch64",
}
_BOLT_ARCHES = frozenset({"x86_64", "aarch64"})


@dataclass(frozen=True, slots=True)
class NativeTargetSpec:
    triple: str | None
    os: str
    arch: str
    object_format: NativeObjectFormat

    @property
    def bolt_support_error(self) -> str | None:
        if self.os != "linux" or self.object_format is not NativeObjectFormat.ELF:
            return (
                "BOLT requires a Linux ELF target; "
                f"resolved {self.os}/{self.object_format.value}"
            )
        if self.arch not in _BOLT_ARCHES:
            return (
                "BOLT supports Molt Linux targets only on x86_64 and aarch64; "
                f"resolved {self.arch}"
            )
        return None


@dataclass(frozen=True, slots=True)
class NativeLinkCapabilities:
    linker: NativeLinkerKind
    object_format: NativeObjectFormat
    explicit_no_icf_flag: str | None


@dataclass(frozen=True, slots=True)
class NativeLinkPolicy:
    preserve_function_identity: bool
    dead_strip: bool
    emit_relocations: bool
    strip_after_link: bool
    bolt_requested: bool


@dataclass(frozen=True, slots=True)
class NativeLinkPlan:
    target: NativeTargetSpec
    capabilities: NativeLinkCapabilities
    policy: NativeLinkPolicy
    command: tuple[str, ...]
    linker_hint: str | None
    normalized_target: str | None


def native_linker_name_from_driver_command(
    command: Sequence[str],
    *,
    hinted: str | None = None,
) -> str | None:
    selected = hinted
    for arg in command:
        if arg.startswith("-fuse-ld="):
            selected = arg.split("=", 1)[1].strip().lower()
            break
    name = Path(selected).name if selected else None
    if name in {
        "ld.lld",
        "ld.lld.exe",
        "ld64.lld",
        "ld64.lld.exe",
        "lld-link",
        "lld-link.exe",
    }:
        return "lld"
    if name in {"mold", "mold.exe"}:
        return "mold"
    return name


def native_link_policy_flags(
    *,
    target: NativeTargetSpec,
    capabilities: NativeLinkCapabilities,
    msvc_driver: bool = False,
    dead_strip: bool = True,
) -> tuple[str, ...]:
    """Return one driver-ready deterministic and identity-preserving policy."""
    if target.object_format is NativeObjectFormat.COFF:
        flags = ["/Brepro"]
        if dead_strip:
            flags.append("/OPT:REF")
        flags.append("/OPT:NOICF")
        if msvc_driver:
            return ("/link", *flags)
        return tuple(f"-Wl,{flag}" for flag in flags)
    if target.object_format is NativeObjectFormat.MACHO:
        flags = ["-Wl,-no_deduplicate"]
        if dead_strip:
            flags.insert(0, "-Wl,-dead_strip")
        return tuple(flags)
    flags = ["-Wl,--gc-sections"] if dead_strip else []
    if capabilities.explicit_no_icf_flag:
        flags.append(capabilities.explicit_no_icf_flag)
    return tuple(flags)


def _normalize_arch(raw: str) -> str:
    normalized = raw.strip().lower().replace(" ", "_")
    return _ARCH_ALIASES.get(normalized, normalized)


def resolve_native_target_spec(
    target_triple: str | None,
    *,
    host_platform: str,
    host_arch: str,
) -> NativeTargetSpec:
    triple = target_triple.lower() if target_triple else None
    if triple:
        arch = _normalize_arch(triple.split("-", 1)[0])
        if "windows" in triple or "msvc" in triple or "mingw" in triple:
            return NativeTargetSpec(target_triple, "windows", arch, NativeObjectFormat.COFF)
        if "apple" in triple or "darwin" in triple or "macos" in triple:
            return NativeTargetSpec(target_triple, "macos", arch, NativeObjectFormat.MACHO)
        if "linux" in triple:
            return NativeTargetSpec(target_triple, "linux", arch, NativeObjectFormat.ELF)
        raise RuntimeError(
            "Native linking has no object-format policy for target "
            f"{target_triple!r}."
        )

    arch = _normalize_arch(host_arch)
    if host_platform == "win32":
        return NativeTargetSpec(None, "windows", arch, NativeObjectFormat.COFF)
    if host_platform == "darwin":
        return NativeTargetSpec(None, "macos", arch, NativeObjectFormat.MACHO)
    if host_platform.startswith("linux"):
        return NativeTargetSpec(None, "linux", arch, NativeObjectFormat.ELF)
    raise RuntimeError(f"Native linking is unsupported on host platform {host_platform!r}.")


def native_link_capabilities(
    *,
    target: NativeTargetSpec,
    linker_hint: str | None,
) -> NativeLinkCapabilities:
    linker = (
        NativeLinkerKind(linker_hint)
        if linker_hint in {NativeLinkerKind.LLD.value, NativeLinkerKind.MOLD.value}
        else NativeLinkerKind.SYSTEM
    )
    no_icf: str | None = None
    if target.object_format is NativeObjectFormat.COFF:
        no_icf = "-Wl,/OPT:NOICF"
    elif target.object_format is NativeObjectFormat.MACHO:
        no_icf = "-Wl,-no_deduplicate"
    elif linker in {NativeLinkerKind.LLD, NativeLinkerKind.MOLD}:
        no_icf = "-Wl,--icf=none"
    return NativeLinkCapabilities(
        linker=linker,
        object_format=target.object_format,
        explicit_no_icf_flag=no_icf,
    )


def native_link_policy(
    *,
    target: NativeTargetSpec,
    profile: str,
    keep_symbols: bool,
    bolt_requested: bool,
) -> NativeLinkPolicy:
    if bolt_requested:
        if profile != "release":
            raise RuntimeError("BOLT requires the release build profile.")
        if error := target.bolt_support_error:
            raise RuntimeError(error)
    return NativeLinkPolicy(
        preserve_function_identity=True,
        dead_strip=True,
        emit_relocations=bolt_requested,
        strip_after_link=(
            profile == "release"
            and target.object_format
            in {NativeObjectFormat.ELF, NativeObjectFormat.MACHO}
            and not keep_symbols
            and not bolt_requested
        ),
        bolt_requested=bolt_requested,
    )


def native_strip_flags(target: NativeTargetSpec) -> tuple[str, ...]:
    """Return object-format flags for the canonical LLVM/native strip family."""
    if target.object_format is NativeObjectFormat.MACHO:
        return ("-x",)
    if target.object_format is NativeObjectFormat.ELF:
        return ("--strip-all",)
    raise RuntimeError(
        "Post-link stripping has no policy for object format "
        f"{target.object_format.value!r}."
    )
