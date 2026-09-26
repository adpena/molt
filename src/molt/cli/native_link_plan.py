from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
from enum import Enum
import hashlib
from pathlib import Path
import platform
import re
import sys
from typing import TYPE_CHECKING, Iterator, Sequence

if TYPE_CHECKING:
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkRequirements,
    )

from molt import file_publication
from molt.llvm_linker_roles import LlvmLinkerRole
from molt.native_artifact_header import (
    NativeArtifactError,
    OBJECT_KINDS,
    read_native_artifact,
)
from molt.native_target_shape import (
    NativeObjectFormat as NativeObjectFormat,
    native_artifact_shape,
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


class NativeLinkerKind(str, Enum):
    SYSTEM = "system"
    LLD = "lld"
    MOLD = "mold"


_BOLT_ARCHES = frozenset({"x86_64", "aarch64"})


@dataclass(frozen=True, slots=True)
class NativeTargetSpec:
    triple: str | None
    os: str
    arch: str
    object_format: NativeObjectFormat

    @property
    def link_dialect(self) -> LinkDialect:
        if self.object_format is NativeObjectFormat.ELF:
            return LinkDialect.ELF_GNU
        if self.object_format is NativeObjectFormat.MACHO:
            return LinkDialect.MACHO
        return (
            LinkDialect.COFF_GNU
            if (self.triple or "").split("-")[-1] in {"gnu", "gnullvm"}
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
class NativeLinkSidecar:
    """Semantic linker input kept in a plan, not a shared filesystem leaf."""

    role: str
    planned_path: Path
    content: bytes
    command_index: int
    operand_prefix: str

    def fact(self) -> dict[str, object]:
        return {
            "role": self.role,
            "sha256": hashlib.sha256(self.content).hexdigest(),
            "size_bytes": len(self.content),
        }


@dataclass(frozen=True, slots=True)
class NativeLinkPlan:
    target: NativeTargetSpec
    capabilities: NativeLinkCapabilities
    policy: NativeLinkPolicy
    command: tuple[str, ...]
    linker_hint: str | None
    normalized_target: str | None
    sidecars: tuple[NativeLinkSidecar, ...] = ()
    selection_requirements: SourceExtensionLinkRequirements | None = None

    def sidecar_facts(self) -> tuple[dict[str, object], ...]:
        return tuple(sidecar.fact() for sidecar in self.sidecars)


@contextmanager
def native_link_execution_command(
    plan: NativeLinkPlan,
    *,
    planned_output: Path,
    execution_output: Path,
    selection_arguments: Sequence[str] = (),
) -> Iterator[list[str]]:
    """Own private sidecars while retargeting one typed plan for execution."""

    result = [*plan.command, *selection_arguments]
    matches = [
        index
        for index in range(1, len(result))
        if result[index - 1] == "-o" and result[index] == str(planned_output)
    ]
    if len(matches) != 1:
        raise RuntimeError(
            "Native link plan must contain exactly one canonical output operand; "
            f"found {len(matches)} for {planned_output}."
        )
    result[matches[0]] = str(execution_output)
    staged_sidecars: list[Path] = []
    try:
        for sidecar in plan.sidecars:
            expected = f"{sidecar.operand_prefix}{sidecar.planned_path}"
            if (
                sidecar.command_index < 0
                or sidecar.command_index >= len(result)
                or result[sidecar.command_index] != expected
            ):
                raise RuntimeError(
                    "Native link plan sidecar operand mismatch: "
                    f"{sidecar.role}, index {sidecar.command_index}, {expected}."
                )
            staged = file_publication.staged_file_path(
                sidecar.planned_path,
                purpose="native-link",
                suffix=sidecar.planned_path.suffix,
            )
            staged_sidecars.append(staged)
            with staged.open("xb") as stream:
                stream.write(sidecar.content)
            result[sidecar.command_index] = f"{sidecar.operand_prefix}{staged}"
        yield result
    finally:
        for staged in staged_sidecars:
            staged.unlink(missing_ok=True)


def native_artifact_link_arguments(
    path: Path, *, kind: NativeArtifactKind, target: NativeTargetSpec
) -> tuple[str, ...]:
    """Keep every compiler partition, without affecting other archive inputs."""
    if kind is NativeArtifactKind.OBJECT:
        return (str(path),)
    if kind is not NativeArtifactKind.ARCHIVE:
        raise ValueError(f"Unknown native artifact kind: {kind!r}.")
    return whole_archive_link_arguments(str(path), dialect=target.link_dialect)


def whole_archive_link_arguments(
    argument: str, *, dialect: LinkDialect
) -> tuple[str, ...]:
    """Force one archive through the selected driver without splitting its path."""
    if dialect is LinkDialect.MACHO:
        return ("-Xlinker", "-force_load", "-Xlinker", argument)
    if dialect is LinkDialect.COFF_MSVC:
        return ("-Xlinker", f"/WHOLEARCHIVE:{argument}")
    if dialect is LinkDialect.WASM:
        return ("--whole-archive", argument, "--no-whole-archive")
    if dialect in {LinkDialect.ELF_GNU, LinkDialect.COFF_GNU}:
        return (
            "-Xlinker",
            "--whole-archive",
            argument,
            "-Xlinker",
            "--no-whole-archive",
        )
    raise ValueError(f"Unknown archive link dialect: {dialect!r}.")


def target_is_wasm(target_triple: str) -> bool:
    normalized = target_triple.strip().lower()
    if normalized in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        return True
    if normalized.startswith("wasm"):
        raise ValueError(f"Unsupported WASM target: {target_triple!r}")
    try:
        resolve_native_target_spec(normalized)
    except RuntimeError as exc:
        raise ValueError(str(exc)) from exc
    return False


def resolve_link_dialect(
    target_triple: str | None,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> LinkDialect:
    if target_triple is not None and target_is_wasm(target_triple):
        return LinkDialect.WASM
    return resolve_native_target_spec(
        target_triple, host_platform=host_platform, host_arch=host_arch
    ).link_dialect


def validate_native_object_artifact(path: Path, target: NativeTargetSpec) -> None:
    """Admit one relocatable header with the exact requested target shape."""
    try:
        shape = native_artifact_shape(
            target.arch, target_triple=target.triple, object_format=target.object_format
        )
        read_native_artifact(path).admit(
            object_format=target.object_format,
            kinds=OBJECT_KINDS,
            shape=shape,
            exact_target=True,
        )
    except (NativeArtifactError, RuntimeError) as exc:
        raise RuntimeError(
            f"Native object output {path} is not a relocatable {target.object_format.value} "
            f"object for {target.arch}: {exc}; --emit obj must produce one actual object, "
            "never an archive or image."
        ) from exc


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
    if target.link_dialect is LinkDialect.COFF_GNU:
        if msvc_driver:
            raise RuntimeError("A COFF-GNU target cannot use the MSVC driver dialect.")
        flags = ["-Wl,--no-insert-timestamp"]
        if dead_strip:
            flags.append("-Wl,--gc-sections")
        if capabilities.explicit_no_icf_flag:
            flags.append(capabilities.explicit_no_icf_flag)
        return tuple(flags)
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


def _host_target_triple(
    *, host_platform: str | None = None, host_arch: str | None = None
) -> str:
    """Project the same host facts used by the native object-format policy."""
    target = resolve_native_target_spec(
        None, host_platform=host_platform, host_arch=host_arch
    )
    suffix = {
        "windows": "pc-windows-msvc",
        "macos": "apple-darwin",
        "linux": "unknown-linux-gnu",
    }[target.os]
    return f"{target.arch}-{suffix}"


def resolve_native_target_spec(
    target_triple: str | None,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> NativeTargetSpec:
    if target_triple is not None:
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
                return NativeTargetSpec(
                    triple, "windows", arch, native_object_format_for_os("windows")
                )
            if os_parts in ({"darwin"}, {"macos"}):
                return NativeTargetSpec(
                    triple, "macos", arch, native_object_format_for_os("macos")
                )
            if os_parts == {"linux"} and not set(parts[1:]) & {"msvc", "mingw32"}:
                return NativeTargetSpec(
                    triple, "linux", arch, native_object_format_for_os("linux")
                )
        raise RuntimeError(
            f"Native linking has no object-format policy for target {target_triple!r}."
        )

    host_platform = sys.platform if host_platform is None else host_platform
    arch = _normalize_arch(platform.machine() if host_arch is None else host_arch)
    if host_platform == "win32":
        return NativeTargetSpec(
            None, "windows", arch, native_object_format_for_os("windows")
        )
    if host_platform == "darwin":
        return NativeTargetSpec(
            None, "macos", arch, native_object_format_for_os("macos")
        )
    if host_platform.startswith("linux"):
        return NativeTargetSpec(
            None, "linux", arch, native_object_format_for_os("linux")
        )
    raise RuntimeError(
        f"Native linking is unsupported on host platform {host_platform!r}."
    )


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
    if target.link_dialect is LinkDialect.COFF_GNU:
        # GNU PE ld has no ICF; LLD's MinGW driver can explicitly disable it.
        no_icf = "-Wl,--icf=none" if linker is NativeLinkerKind.LLD else None
    elif target.object_format is NativeObjectFormat.COFF:
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
