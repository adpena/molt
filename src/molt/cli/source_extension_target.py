from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum
from pathlib import Path

from molt.cli.native_link_plan import (
    NativeObjectFormat,
    NativeTargetSpec,
    _host_target_triple,
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
    normalized = target_triple.strip().lower()
    if normalized in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        return True
    if normalized.startswith("wasm"):
        raise ValueError(f"Unsupported source-extension WASM target: {target_triple!r}")
    try:
        resolve_native_target_spec(normalized)
    except RuntimeError as exc:
        raise ValueError(str(exc)) from exc
    return False


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
        host_platform=host_platform,
        host_arch=host_arch,
    )
    if native_target.object_format is NativeObjectFormat.ELF:
        return SourceExtensionLinkDialect.ELF_GNU
    if native_target.object_format is NativeObjectFormat.MACHO:
        return SourceExtensionLinkDialect.MACHO
    normalized = native_target.triple or ""
    return (
        SourceExtensionLinkDialect.COFF_GNU
        if (normalized.split("-")[-1] in {"gnu", "gnullvm"})
        else SourceExtensionLinkDialect.COFF_MSVC
    )


def resolve_source_extension_target_plan(
    requested: str,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> SourceExtensionTargetPlan:
    if not isinstance(requested, str) or not requested.strip():
        raise ValueError("target must be an explicit name or target triple")
    raw = requested.strip().lower()
    if any(character.isspace() for character in raw):
        raise ValueError("target must be 'native', 'wasm', or a Rust target triple")
    normalized = raw
    if normalized == "native":
        try:
            target_triple = _host_target_triple(
                host_platform=host_platform, host_arch=host_arch
            )
        except RuntimeError as exc:
            raise ValueError(str(exc)) from exc
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
        requested=normalized,
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


def source_extension_recorded_target_plan(
    requested: str, *, target_triple: str
) -> SourceExtensionTargetPlan:
    """Validate recorded build intent against artifact facts, never inspector facts."""
    if target_triple.strip().lower() in {"native", "wasm", "wasm-freestanding"}:
        raise ValueError("recorded target must be a canonical explicit target triple")
    if requested != requested.strip().lower():
        raise ValueError("recorded requested target must be canonical lowercase")
    artifact = resolve_source_extension_target_plan(target_triple)
    if (
        artifact.target_triple != target_triple
        or artifact.compiler_target_triple is None
    ):
        raise ValueError("recorded target must be a canonical explicit target triple")
    if requested == "native":
        if artifact.is_wasm:
            raise ValueError("recorded native target cannot describe a WASM artifact")
        return replace(artifact, requested="native", compiler_target_triple=None)
    plan = resolve_source_extension_target_plan(requested)
    if plan.requested != requested or plan.target_triple != target_triple:
        raise ValueError("recorded requested target differs from the artifact target")
    return plan
