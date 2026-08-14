from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import platform
import sys

from molt.cli.native_link_plan import (
    NativeObjectFormat,
    NativeTargetSpec,
    resolve_native_target_spec,
)


@dataclass(frozen=True, slots=True)
class SourceExtensionTargetPlan:
    requested: str
    target_triple: str
    compiler_target_triple: str | None
    native_target: NativeTargetSpec | None

    @property
    def is_wasm(self) -> bool:
        return source_extension_target_is_wasm(self.target_triple)

    @property
    def artifact_kind(self) -> str:
        return source_extension_artifact_kind(self.target_triple)

    @property
    def artifact_suffix(self) -> str:
        return source_extension_artifact_suffix(self.target_triple)

    @property
    def requires_position_independent_code(self) -> bool:
        return (
            self.native_target is not None
            and self.native_target.object_format is not NativeObjectFormat.COFF
        )

    @property
    def preprocessor_symbols(self) -> tuple[str, ...]:
        if self.is_wasm:
            return ("MOLT_EXTENSION_WASM_STATIC_LINK",)
        return ()


class SourceExtensionLinkDialect(str, Enum):
    ELF_GNU = "elf-gnu"
    MACHO = "macho"
    COFF_GNU = "coff-gnu"
    COFF_MSVC = "coff-msvc"
    WASM = "wasm"


def source_extension_target_is_wasm(target_triple: str) -> bool:
    return target_triple.strip().lower().startswith("wasm32")


def source_extension_artifact_kind(target_triple: str) -> str:
    return (
        "wasm_relocatable_object"
        if source_extension_target_is_wasm(target_triple)
        else "static_archive"
    )


def source_extension_artifact_suffix(target_triple: str) -> str:
    return ".molt.wasm" if source_extension_target_is_wasm(target_triple) else ".molt.a"


def source_extension_link_dialect(
    target_triple: str | None,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> SourceExtensionLinkDialect:
    if target_triple is not None and source_extension_target_is_wasm(target_triple):
        return SourceExtensionLinkDialect.WASM
    native_target = resolve_native_target_spec(
        target_triple,
        host_platform=sys.platform if host_platform is None else host_platform,
        host_arch=platform.machine() if host_arch is None else host_arch,
    )
    if native_target.object_format is NativeObjectFormat.ELF:
        return SourceExtensionLinkDialect.ELF_GNU
    if native_target.object_format is NativeObjectFormat.MACHO:
        return SourceExtensionLinkDialect.MACHO
    normalized = (target_triple or "").lower()
    return (
        SourceExtensionLinkDialect.COFF_GNU
        if (
            "mingw" in normalized
            or "gnullvm" in normalized
            or normalized.endswith("-gnu")
        )
        else SourceExtensionLinkDialect.COFF_MSVC
    )


def resolve_source_extension_target_plan(
    requested: str | None,
    *,
    host_target_triple: str,
    host_platform: str,
    host_arch: str,
) -> SourceExtensionTargetPlan:
    raw = (requested or "native").strip()
    if not raw:
        raw = "native"
    if any(character.isspace() for character in raw):
        raise ValueError("target must be 'native', 'wasm', or a Rust target triple")
    normalized = raw.lower()
    if normalized == "native":
        target_triple = host_target_triple.lower()
        compiler_target_triple = None
    elif normalized == "wasm":
        target_triple = "wasm32-wasip1"
        compiler_target_triple = target_triple
    elif normalized == "wasm-freestanding":
        target_triple = "wasm32-unknown-unknown"
        compiler_target_triple = target_triple
    else:
        target_triple = normalized
        compiler_target_triple = target_triple
    native_target = (
        None
        if source_extension_target_is_wasm(target_triple)
        else resolve_native_target_spec(
            compiler_target_triple,
            host_platform=host_platform,
            host_arch=host_arch,
        )
    )
    return SourceExtensionTargetPlan(
        requested=raw,
        target_triple=target_triple,
        compiler_target_triple=compiler_target_triple,
        native_target=native_target,
    )


def source_extension_artifact_path(
    module_parts: list[str],
    target_plan: SourceExtensionTargetPlan,
) -> Path:
    return Path(
        *module_parts[:-1],
        module_parts[-1] + target_plan.artifact_suffix,
    )
