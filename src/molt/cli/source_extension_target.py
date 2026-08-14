from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

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
        return self.target_triple.startswith("wasm32")

    @property
    def artifact_kind(self) -> str:
        return "wasm_relocatable_object" if self.is_wasm else "static_archive"

    @property
    def artifact_suffix(self) -> str:
        return ".molt.wasm" if self.is_wasm else ".molt.a"

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
        if target_triple.startswith("wasm32")
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
