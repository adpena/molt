"""Native target admission and platform facts, without linker execution imports."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum, StrEnum
from functools import cache
import platform
import re
import sys

from molt.llvm_linker_roles import LlvmLinkerRole
from molt.native_target_shape import (
    NativeObjectFormat,
    native_object_format_for_os,
    normalize_native_architecture as _normalize_arch,
)


class LinkDialect(str, Enum):
    ELF_GNU = "elf-gnu"
    MACHO = "macho"
    COFF_GNU = "coff-gnu"
    COFF_MSVC = "coff-msvc"
    WASM = "wasm"

    @property
    def llvm_linker_role(self) -> LlvmLinkerRole:
        if self is LinkDialect.WASM:
            return "wasm-ld"
        if self is LinkDialect.MACHO:
            return "ld64.lld"
        if self is LinkDialect.COFF_MSVC:
            return "lld-link"
        return "ld.lld"


class NativeArtifactKind(str, Enum):
    OBJECT = "object"
    ARCHIVE = "archive"

    @classmethod
    def for_emit_mode(cls, emit_mode: str) -> NativeArtifactKind:
        if emit_mode == "obj":
            return cls.OBJECT
        if emit_mode == "bin":
            return cls.ARCHIVE
        raise ValueError(f"Native artifacts do not support emit mode {emit_mode!r}.")

    def suffix(self, target: NativeTargetSpec) -> str:
        if target.object_format is NativeObjectFormat.COFF:
            return ".obj" if self is NativeArtifactKind.OBJECT else ".lib"
        return ".o" if self is NativeArtifactKind.OBJECT else ".a"


_BOLT_ARCHES = frozenset({"x86_64", "aarch64"})


@dataclass(frozen=True, slots=True)
class NativeTargetSpec:
    triple: str
    is_host: bool = False
    os: str = field(init=False)
    arch: str = field(init=False)
    object_format: NativeObjectFormat = field(init=False)

    def __post_init__(self) -> None:
        if type(self.is_host) is not bool:
            raise TypeError("native target is_host must be a bool")
        triple, target_os, arch = _admit_native_triple(self.triple)
        object.__setattr__(self, "triple", triple)
        object.__setattr__(self, "os", target_os)
        object.__setattr__(self, "arch", arch)
        object.__setattr__(
            self, "object_format", native_object_format_for_os(target_os)
        )

    @property
    def cargo_target(self) -> str | None:
        """Cargo's native selection wire value; artifact identity uses triple.

        An explicit --target remains an explicit Cargo/toolchain selection,
        even when it spells the host triple. This is request custody, not an
        executable-on-host or CPU-feature predicate.
        """
        return None if self.is_host else self.triple

    @property
    def link_dialect(self) -> LinkDialect:
        if self.object_format is NativeObjectFormat.ELF:
            return LinkDialect.ELF_GNU
        if self.object_format is NativeObjectFormat.MACHO:
            return LinkDialect.MACHO
        return (
            LinkDialect.COFF_GNU
            if self.triple.split("-")[-1] in {"gnu", "gnullvm"}
            else LinkDialect.COFF_MSVC
        )

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


class LinuxLibc(StrEnum):
    """The C library of a Linux host, as the triple's environment names it."""

    GNU = "gnu"
    MUSL = "musl"


@cache
def host_linux_libc() -> LinuxLibc:
    """The running interpreter's C library, read once: a fact of the process."""
    return _probe_linux_libc()


def _probe_linux_libc() -> LinuxLibc:
    # sysconfig carries the interpreter's configured ABI even where libc_ver
    # cannot identify musl's loader; libc_ver reads the executable in chunks.
    import sysconfig

    multiarch = str(sysconfig.get_config_var("MULTIARCH") or "").lower()
    if "musl" in multiarch or platform.libc_ver()[0].lower() == "musl":
        return LinuxLibc.MUSL
    return LinuxLibc.GNU


def _host_target_triple(
    *, host_platform: str | None = None, host_arch: str | None = None
) -> str:
    """Resolve the native host ABI without invoking a build toolchain."""
    host_platform = sys.platform if host_platform is None else host_platform
    arch = _normalize_arch(platform.machine() if host_arch is None else host_arch)
    if host_platform == "win32":
        suffix = "pc-windows-msvc"
    elif host_platform == "darwin":
        suffix = "apple-darwin"
    elif host_platform.startswith("linux"):
        suffix = f"unknown-linux-{host_linux_libc()}"
    else:
        raise RuntimeError(
            f"Native linking is unsupported on host platform {host_platform!r}."
        )
    return f"{arch}-{suffix}"


def resolve_native_target_spec(
    target_triple: str | None,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> NativeTargetSpec:
    is_host = target_triple is None
    if is_host:
        target_triple = _host_target_triple(
            host_platform=host_platform, host_arch=host_arch
        )
    assert target_triple is not None
    return NativeTargetSpec(target_triple, is_host=is_host)


def _admit_native_triple(target_triple: str) -> tuple[str, str, str]:
    if type(target_triple) is not str:
        raise TypeError("native target triple must be a string")
    triple = target_triple.strip().lower()
    parts = triple.split("-")
    # Classify target components, never substrings (e.g. notlinux, apple-ios,
    # or a conflicting linux/windows triple). Toolchains own arch support.
    if (
        len(parts) >= 3
        and not parts[0].startswith("wasm")
        and all(re.fullmatch(r"[a-z0-9_+.]+", p) for p in parts)
    ):
        arch = _normalize_arch(parts[0])
        os_parts = set(parts[1:]) & {
            "linux",
            "windows",
            "darwin",
            "macos",
            "ios",
            "tvos",
            "watchos",
            "visionos",
            "android",
        }
        if os_parts == {"windows"}:
            return triple, "windows", arch
        if os_parts in ({"darwin"}, {"macos"}):
            return triple, "macos", arch
        if os_parts == {"linux"} and not set(parts[1:]) & {"msvc", "mingw32"}:
            return triple, "linux", arch
    raise RuntimeError(
        f"Native linking has no object-format policy for target {target_triple!r}."
    )
