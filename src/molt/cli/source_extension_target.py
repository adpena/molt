from __future__ import annotations

from molt.compilation_target import (
    Target,
    TargetKind,
    admit_target,
    is_wasm_target,
    native_target_spec,
    target_triple as project_target_triple,
)

from dataclasses import dataclass, replace
from pathlib import Path

from molt.native_target import LinkDialect, NativeTargetSpec, resolve_native_target_spec
from molt.compilation_target import (
    resolve_link_dialect,
    target_is_wasm as source_extension_target_is_wasm,
)
from molt.native_target_shape import NativeObjectFormat

SourceExtensionLinkDialect = LinkDialect
source_extension_link_dialect = resolve_link_dialect
SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION = 6


@dataclass(frozen=True, slots=True)
class SourceExtensionTargetPlan:
    requested: Target
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


def source_extension_artifact_kind(target_triple: str) -> str:
    return (
        "wasm_relocatable_object"
        if source_extension_target_is_wasm(target_triple)
        else "static_archive"
    )


def source_extension_artifact_suffix(target_triple: str) -> str:
    return ".molt.wasm" if source_extension_target_is_wasm(target_triple) else ".molt.a"


def resolve_source_extension_target_plan(
    requested: Target,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> SourceExtensionTargetPlan:
    if is_wasm_target(requested):
        triple = project_target_triple(requested)
        assert triple is not None
        native_target = None
        compiler_target_triple = triple
    else:
        try:
            native_target = (
                resolve_native_target_spec(
                    None, host_platform=host_platform, host_arch=host_arch
                )
                if requested is TargetKind.NATIVE
                else native_target_spec(requested)
            )
        except RuntimeError as exc:
            raise ValueError(str(exc)) from exc
        triple = native_target.triple
        compiler_target_triple = native_target.cargo_target
    return SourceExtensionTargetPlan(
        requested=requested,
        target_triple=triple,
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
    artifact = resolve_source_extension_target_plan(admit_target(target_triple))
    if (
        artifact.target_triple != target_triple
        or artifact.compiler_target_triple is None
    ):
        raise ValueError("recorded target must be a canonical explicit target triple")
    if requested == "native":
        if artifact.is_wasm:
            raise ValueError("recorded native target cannot describe a WASM artifact")
        return replace(
            artifact, requested=TargetKind.NATIVE, compiler_target_triple=None
        )
    admitted = admit_target(requested)
    plan = resolve_source_extension_target_plan(admitted)
    if plan.requested != admitted or plan.target_triple != target_triple:
        raise ValueError("recorded requested target differs from the artifact target")
    return plan
