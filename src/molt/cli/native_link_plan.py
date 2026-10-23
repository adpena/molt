from __future__ import annotations

from molt.native_target import LinkDialect, NativeArtifactKind, NativeTargetSpec

from contextlib import contextmanager
from dataclasses import dataclass
from enum import Enum
import hashlib
from pathlib import Path
from typing import TYPE_CHECKING, Generator, Sequence

if TYPE_CHECKING:
    from molt.cli.native_link_manifest import NativeLinkInputs
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkRequirements,
    )

from molt import file_publication
from molt.llvm_linker_roles import executable_entrypoint_name
from molt.native_artifact_header import (
    NativeArtifactError,
    OBJECT_KINDS,
    read_native_artifact,
)
from molt.native_target_shape import (
    NativeObjectFormat,
    native_artifact_shape,
)


class NativeLinkerKind(str, Enum):
    SYSTEM = "system"
    LLD = "lld"
    MOLD = "mold"
    # `zig cc` links with zig's own linker, which runs no identical-code
    # folding and rejects the system linkers' flags that disable it.
    ZIG = "zig"


class NativeSymbolStrip(str, Enum):
    """Where a release plan removes non-global symbols from the image.

    ``AT_LINK`` passes the strip to the linker (ld64 ``-x -S``, ELF
    ``--strip-all``), as Go's ``-ldflags=-s -w`` and rustc's ELF
    ``--strip-all`` do. The linked image is final: no tool rewrites it, and
    Apple ld's ad hoc signature stands. ``AFTER_LINK`` runs the strip tool on
    the linked image, for a linker that cannot omit every local symbol.
    """

    NONE = "none"
    AT_LINK = "at-link"
    AFTER_LINK = "after-link"


@dataclass(frozen=True, slots=True)
class NativeLinkCapabilities:
    linker: NativeLinkerKind
    object_format: NativeObjectFormat
    explicit_no_icf_flag: str | None
    # Apple ld signs a Mach-O image ad hoc under its -final_output name, so a
    # private stage is signed as the published binary at link time. ld64.lld
    # and zig name the signature after the -o operand instead.
    signs_as_final_output: bool
    # The linker omits every non-global symbol on request. zig's Mach-O
    # linker keeps the local thunks it synthesizes under -x.
    strips_local_symbols: bool


@dataclass(frozen=True, slots=True)
class NativeLinkPolicy:
    preserve_function_identity: bool
    dead_strip: bool
    emit_relocations: bool
    symbol_strip: NativeSymbolStrip
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
    runtime_inputs: NativeLinkInputs | None = None

    def sidecar_facts(self) -> tuple[dict[str, object], ...]:
        return tuple(sidecar.fact() for sidecar in self.sidecars)


@contextmanager
def native_link_execution_command(
    plan: NativeLinkPlan,
    *,
    planned_output: Path,
    execution_output: Path,
    selection_arguments: Sequence[str] = (),
) -> Generator[list[str]]:
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
            with staged.open("xb") as stream:
                staged_sidecars.append(staged)
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


def is_zig_compiler_command(command: Sequence[str]) -> bool:
    return (
        len(command) >= 2
        and executable_entrypoint_name(Path(command[0])) == "zig"
        and command[1] in {"cc", "c++"}
    )


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
    else:
        if is_zig_compiler_command(command):
            return NativeLinkerKind.ZIG.value
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
        flags = ["-Wl,-dead_strip"] if dead_strip else []
        if capabilities.explicit_no_icf_flag:
            flags.append(capabilities.explicit_no_icf_flag)
        return tuple(flags)
    flags = ["-Wl,--gc-sections"] if dead_strip else []
    if capabilities.explicit_no_icf_flag:
        flags.append(capabilities.explicit_no_icf_flag)
    return tuple(flags)


def native_link_output_flags(
    *, capabilities: NativeLinkCapabilities, output: Path
) -> tuple[str, ...]:
    """Name the published binary to a link that writes a private stage.

    Apple ld then signs the stage ad hoc as ``output``: the bytes carry no
    stage name and need no post-link codesign pass (1.4-3.2 s on a 68 MiB dev
    binary). ``-adhoc_codesign`` signs x86_64 images too, which ld leaves
    unsigned by default, so every Mach-O arch publishes the same way.
    ``-Xlinker`` keeps a path with a comma intact.
    """
    if not capabilities.signs_as_final_output:
        return ()
    return ("-Wl,-adhoc_codesign", "-Xlinker", "-final_output", "-Xlinker", str(output))


def native_link_capabilities(
    *,
    target: NativeTargetSpec,
    linker_hint: str | None,
) -> NativeLinkCapabilities:
    linker = (
        NativeLinkerKind(linker_hint)
        if linker_hint
        in {
            NativeLinkerKind.LLD.value,
            NativeLinkerKind.MOLD.value,
            NativeLinkerKind.ZIG.value,
        }
        else NativeLinkerKind.SYSTEM
    )
    no_icf: str | None = None
    if linker is NativeLinkerKind.ZIG:
        no_icf = None
    elif target.link_dialect is LinkDialect.COFF_GNU:
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
        signs_as_final_output=(
            target.object_format is NativeObjectFormat.MACHO
            and linker is NativeLinkerKind.SYSTEM
        ),
        strips_local_symbols=not (
            target.object_format is NativeObjectFormat.MACHO
            and linker is NativeLinkerKind.ZIG
        ),
    )


def native_link_policy(
    *,
    target: NativeTargetSpec,
    capabilities: NativeLinkCapabilities,
    profile: str,
    keep_symbols: bool,
    bolt_requested: bool,
) -> NativeLinkPolicy:
    if bolt_requested:
        if profile != "release":
            raise RuntimeError("BOLT requires the release build profile.")
        if error := target.bolt_support_error:
            raise RuntimeError(error)
    # A BOLT plan keeps symbols and relocations; its executor strips the
    # optimized image.
    if (
        profile != "release"
        or target.object_format
        not in {NativeObjectFormat.ELF, NativeObjectFormat.MACHO}
        or keep_symbols
        or bolt_requested
    ):
        symbol_strip = NativeSymbolStrip.NONE
    elif capabilities.strips_local_symbols:
        symbol_strip = NativeSymbolStrip.AT_LINK
    else:
        symbol_strip = NativeSymbolStrip.AFTER_LINK
    return NativeLinkPolicy(
        preserve_function_identity=True,
        dead_strip=True,
        emit_relocations=bolt_requested,
        symbol_strip=symbol_strip,
        bolt_requested=bolt_requested,
    )


def native_link_strip_flags(target: NativeTargetSpec) -> tuple[str, ...]:
    """Return the driver flags of an ``AT_LINK`` strip.

    They remove what ``native_strip_flags`` removes after a link: ld64 ``-x``
    drops the non-global symbols and ``-S`` the debug-map stabs, both of
    which ``strip -x`` drops.
    """
    if target.object_format is NativeObjectFormat.MACHO:
        return ("-Wl,-x", "-Wl,-S")
    if target.object_format is NativeObjectFormat.ELF:
        return ("-Wl,--strip-all",)
    raise RuntimeError(
        "Link-time stripping has no policy for object format "
        f"{target.object_format.value!r}."
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
