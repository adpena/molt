"""Compilation intent, separate from backend choice and artifact shape.

Strings enter only through admission and leave through explicit projections.
The default host is symbolic: constructing a request discovers no host facts.
An explicit native triple carries the existing admitted native specification.
"""

from __future__ import annotations

from enum import Enum
from typing import Literal, assert_never

from molt.native_target import LinkDialect, NativeTargetSpec, resolve_native_target_spec


class TargetKind(Enum):
    NATIVE = "native"
    WASM = "wasm"
    WASM_FREESTANDING = "wasm-freestanding"
    WASI = "wasm32-wasip1"
    WASM_UNKNOWN = "wasm32-unknown-unknown"
    RUST = "rust"
    LUAU = "luau"
    MLIR = "mlir"


type Target = TargetKind | NativeTargetSpec

# The artifact a build produces, with the host resolved: a native spec, or one
# of the two WASM artifact triples. Target facts are keyed by this type, never
# by `str | None` with None meaning the host (Cargo's CompileKind, LLVM Triple).
type WasmArtifactKind = Literal[TargetKind.WASI, TargetKind.WASM_UNKNOWN]
type ArtifactTarget = NativeTargetSpec | WasmArtifactKind


def validate_target(target: Target) -> None:
    """Reject resolved-host specs where symbolic compilation intent is required."""
    if isinstance(target, TargetKind):
        return
    if isinstance(target, NativeTargetSpec):
        if target.is_host:
            raise ValueError(
                "Use TargetKind.NATIVE for host intent; compilation native specs "
                "must describe explicit triples"
            )
        return
    raise TypeError("target must be admitted compilation intent")


def admit_target(value: str) -> Target:
    """Admit public target syntax; backend aliases are resolved by their owner."""
    if type(value) is not str or not value.strip():
        raise ValueError("target must be a nonempty name or native target triple")
    normalized = value.strip().lower()
    try:
        return TargetKind(normalized)
    except ValueError:
        pass
    try:
        return resolve_native_target_spec(normalized)
    except RuntimeError as exc:
        raise ValueError(str(exc)) from exc


def target_name(target: Target) -> str:
    """Project public syntax for argv, metadata, diagnostics and cache records."""
    validate_target(target)
    match target:
        case TargetKind():
            return target.value
        case NativeTargetSpec():
            return target.triple
        case _:
            assert_never(target)


def is_wasm_target(target: Target) -> bool:
    validate_target(target)
    match target:
        case (
            TargetKind.WASM
            | TargetKind.WASM_FREESTANDING
            | TargetKind.WASI
            | TargetKind.WASM_UNKNOWN
        ):
            return True
        case TargetKind.NATIVE | TargetKind.RUST | TargetKind.LUAU | TargetKind.MLIR:
            return False
        case NativeTargetSpec():
            return False
        case _:
            assert_never(target)


def target_triple(target: Target) -> str | None:
    """Project toolchain selection, preserving symbolic host and WASM intent."""
    validate_target(target)
    match target:
        case TargetKind.NATIVE | TargetKind.RUST | TargetKind.LUAU | TargetKind.MLIR:
            return None
        case TargetKind.WASM | TargetKind.WASI:
            return "wasm32-wasip1"
        case TargetKind.WASM_FREESTANDING | TargetKind.WASM_UNKNOWN:
            return "wasm32-unknown-unknown"
        case NativeTargetSpec():
            return target.cargo_target
        case _:
            assert_never(target)


def resolve_artifact_target(target: Target) -> ArtifactTarget:
    """Resolve compilation intent to its artifact target: the one host lookup."""
    validate_target(target)
    match target:
        case TargetKind.NATIVE:
            return resolve_native_target_spec(None)
        case NativeTargetSpec():
            return target
        case TargetKind.WASM | TargetKind.WASI:
            return TargetKind.WASI
        case TargetKind.WASM_FREESTANDING | TargetKind.WASM_UNKNOWN:
            return TargetKind.WASM_UNKNOWN
        case TargetKind.RUST | TargetKind.LUAU | TargetKind.MLIR:
            raise ValueError(f"Target {target.value!r} has no artifact target")
        case _:
            assert_never(target)


def artifact_triple(target: ArtifactTarget) -> str:
    """The concrete triple of an artifact target; the host is already resolved."""
    match target:
        case NativeTargetSpec():
            return target.triple
        case TargetKind.WASI:
            return "wasm32-wasip1"
        case TargetKind.WASM_UNKNOWN:
            return "wasm32-unknown-unknown"
        case _:
            assert_never(target)


def native_target_spec(target: Target) -> NativeTargetSpec:
    validate_target(target)
    match target:
        case TargetKind.NATIVE:
            return resolve_native_target_spec(None)
        case NativeTargetSpec():
            return target
        case (
            TargetKind.WASM
            | TargetKind.WASM_FREESTANDING
            | TargetKind.WASI
            | TargetKind.WASM_UNKNOWN
            | TargetKind.RUST
            | TargetKind.LUAU
            | TargetKind.MLIR
        ):
            raise ValueError(f"Target {target.value!r} is not native")
        case _:
            assert_never(target)


def target_is_wasm(target_triple: str) -> bool:
    """Admit an explicit artifact triple, excluding symbolic compiler aliases."""
    target = admit_target(target_triple)
    validate_target(target)
    match target:
        case TargetKind.WASI | TargetKind.WASM_UNKNOWN:
            return True
        case NativeTargetSpec():
            return False
        case (
            TargetKind.NATIVE
            | TargetKind.WASM
            | TargetKind.WASM_FREESTANDING
            | TargetKind.RUST
            | TargetKind.LUAU
            | TargetKind.MLIR
        ):
            raise ValueError(
                f"Expected an explicit artifact target triple: {target_triple!r}"
            )
        case _:
            assert_never(target)


def resolve_link_dialect(
    target_triple: str | None,
    *,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> LinkDialect:
    if target_triple is None:
        return resolve_native_target_spec(
            None, host_platform=host_platform, host_arch=host_arch
        ).link_dialect
    target = admit_target(target_triple)
    validate_target(target)
    match target:
        case TargetKind.WASI | TargetKind.WASM_UNKNOWN:
            return LinkDialect.WASM
        case NativeTargetSpec():
            return target.link_dialect
        case (
            TargetKind.NATIVE
            | TargetKind.WASM
            | TargetKind.WASM_FREESTANDING
            | TargetKind.RUST
            | TargetKind.LUAU
            | TargetKind.MLIR
        ):
            raise ValueError(
                f"Expected an explicit artifact target triple: {target_triple!r}"
            )
        case _:
            assert_never(target)
