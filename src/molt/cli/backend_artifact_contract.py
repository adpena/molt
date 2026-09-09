"""Requested backend bytes: one authority for naming, cache identity and admission.

This contract validates container/target shape, not program semantics. Native
symbol closure remains a separate required cache admission step; text outputs
never enter that native reader. Filename extensions are projections, not input
evidence, and no encoding fact enables a compiler support-matrix cell.
"""

from __future__ import annotations

import codecs
from dataclasses import dataclass, field
from enum import Enum
import hashlib
import json
from pathlib import Path
from typing import BinaryIO

from molt.cli.native_link_plan import (
    NativeArtifactKind,
    NativeTargetSpec,
    _host_target_triple,
    resolve_native_target_spec,
    target_is_wasm,
)
from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    visit_static_archive_members,
)
from molt.native_artifact_header import (
    NativeArtifact,
    NativeArtifactError,
    NativeReader,
    OBJECT_KINDS,
    decode_native_artifact,
    native_artifact_from_file,
)
from molt.native_target_shape import native_artifact_shape
from molt.toolchain_identity import open_stable_regular_file


class BackendArtifactKind(str, Enum):
    NATIVE_OBJECT = "native-object"
    NATIVE_ARCHIVE = "native-archive"
    WASM = "wasm"
    RUST = "rust"
    LUAU = "luau"
    MLIR = "mlir"


class BackendArtifactValidationError(OSError):
    """A backend artifact does not satisfy its explicit output request."""


@dataclass(frozen=True, slots=True)
class BackendArtifactContract:
    kind: BackendArtifactKind
    target_triple: str | None = None
    _native_target: NativeTargetSpec | None = field(init=False, repr=False)

    def __post_init__(self) -> None:
        if not isinstance(self.kind, BackendArtifactKind):
            raise ValueError(f"Invalid backend artifact kind: {self.kind!r}")
        triple = self.target_triple
        if triple is not None:
            triple = triple.strip().lower()
            if not triple:
                raise ValueError("An explicit backend target triple must not be empty")
        if self.is_wasm:
            triple = triple or "wasm32-wasip1"
            if not target_is_wasm(triple):
                raise ValueError(
                    f"WASM backend artifacts require a WASM target: {triple}"
                )
        try:
            native_target = (
                resolve_native_target_spec(triple or _host_target_triple())
                if self.is_native
                else None
            )
        except RuntimeError as error:
            raise ValueError(str(error)) from error
        if native_target is not None:
            # Bind every later consumer to the same resolved host/target that
            # owns cache identity; None must not re-resolve ambient host state.
            triple = native_target.triple
        object.__setattr__(self, "target_triple", triple)
        object.__setattr__(self, "_native_target", native_target)

    @property
    def is_native(self) -> bool:
        return self.kind in {
            BackendArtifactKind.NATIVE_OBJECT,
            BackendArtifactKind.NATIVE_ARCHIVE,
        }

    @property
    def is_wasm(self) -> bool:
        return self.kind is BackendArtifactKind.WASM

    @property
    def is_text(self) -> bool:
        return self.kind in {
            BackendArtifactKind.RUST,
            BackendArtifactKind.LUAU,
            BackendArtifactKind.MLIR,
        }

    @property
    def native_kind(self) -> NativeArtifactKind | None:
        if self.kind is BackendArtifactKind.NATIVE_OBJECT:
            return NativeArtifactKind.OBJECT
        if self.kind is BackendArtifactKind.NATIVE_ARCHIVE:
            return NativeArtifactKind.ARCHIVE
        return None

    @property
    def native_target(self) -> NativeTargetSpec | None:
        return self._native_target

    @property
    def suffix(self) -> str:
        native_kind = self.native_kind
        if native_kind is not None:
            assert self.native_target is not None
            return native_kind.suffix(self.native_target)
        return {
            BackendArtifactKind.WASM: ".wasm",
            BackendArtifactKind.RUST: ".rs",
            BackendArtifactKind.LUAU: ".luau",
            BackendArtifactKind.MLIR: ".mlir",
        }[self.kind]

    @property
    def cache_identity(self) -> str:
        target = (
            self.native_target.triple
            if self.native_target is not None
            else self.target_triple
        )
        payload = {
            "schema": "molt.backend-artifact-contract.v1",
            "kind": self.kind.value,
            "target": target,
        }
        return hashlib.sha256(
            json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()

    def validate_shared_stdlib(self, *, enabled: bool) -> None:
        if enabled and self.kind is not BackendArtifactKind.NATIVE_ARCHIVE:
            raise ValueError("Shared stdlib extraction requires native archive output")

    def validate_native_shape(self, path: Path) -> None:
        """Admit exact native object shape, including every archive content member."""
        target = self.native_target
        if target is None:
            raise ValueError(
                "Native shape validation requires a native output contract"
            )

        def admit(artifact: NativeArtifact) -> None:
            artifact.admit(
                object_format=target.object_format,
                kinds=OBJECT_KINDS,
                shape=shape,
                exact_target=True,
            )

        def visit_member(member: StaticArchiveMember, stream: BinaryIO) -> None:
            def read_at(offset: int, size: int) -> bytes:
                stream.seek(member.content_offset + offset)
                return stream.read(size)

            try:
                admit(decode_native_artifact(NativeReader(member.size, read_at)))
            except NativeArtifactError as error:
                raise NativeArtifactError(
                    f"archive member {member.name!r}: {error}"
                ) from error

        try:
            shape = native_artifact_shape(
                target.arch,
                target_triple=target.triple,
                object_format=target.object_format,
            )
            if self.native_kind is NativeArtifactKind.ARCHIVE:
                member_count = visit_static_archive_members(
                    path, visit_member=visit_member
                )
                if member_count == 0:
                    raise NativeArtifactError(
                        "backend archive has no relocatable members"
                    )
            else:
                with open_stable_regular_file(
                    path, label="backend native object"
                ) as opened:
                    admit(native_artifact_from_file(opened.stream))
        except (OSError, ValueError, RuntimeError) as error:
            raise BackendArtifactValidationError(
                f"Invalid backend {self.kind.value} artifact {path}: {error}"
            ) from error

    def validate(self, path: Path) -> None:
        if self.is_native:
            self.validate_native_shape(path)
            return
        try:
            if self.is_wasm:
                # Keep the existing structural validator and its provisioning
                # failure policy; magic bytes alone do not prove WASM reuse.
                from molt.cli.runtime_wasm_validation import (
                    _reusable_wasm_artifact_validation_error,
                )

                error = _reusable_wasm_artifact_validation_error(path)
                if error is not None:
                    raise ValueError(error)
                return
            decoder = codecs.getincrementaldecoder("utf-8")(errors="strict")
            meaningful_text = False
            with open_stable_regular_file(
                path, label="backend textual output"
            ) as opened:
                while block := opened.stream.read(64 * 1024):
                    text = decoder.decode(block)
                    if "\0" in text:
                        raise ValueError("textual output contains a NUL character")
                    meaningful_text = meaningful_text or bool(text.strip())
                tail = decoder.decode(b"", final=True)
                meaningful_text = meaningful_text or bool(tail.strip())
            if not meaningful_text:
                raise ValueError("textual output is empty or whitespace-only")
        except (OSError, ValueError) as error:
            raise BackendArtifactValidationError(
                f"Invalid backend {self.kind.value} artifact {path}: {error}"
            ) from error


def resolve_backend_artifact_contract(
    *, target: str, emit_mode: str, target_triple: str | None = None
) -> BackendArtifactContract:
    """Resolve requested bytes without inferring anything from an output path."""
    target = target.strip().lower()
    text_kind = {
        "rust": BackendArtifactKind.RUST,
        "luau": BackendArtifactKind.LUAU,
        "mlir": BackendArtifactKind.MLIR,
    }.get(target)
    if text_kind is not None:
        if emit_mode != "bin":
            raise ValueError(
                f"Textual backend {target} does not support emit mode {emit_mode!r}"
            )
        return BackendArtifactContract(text_kind, target_triple)
    wasm_alias = target in {"wasm", "wasm-freestanding"}
    if wasm_alias or (target.startswith("wasm") and target_is_wasm(target)):
        if emit_mode != "wasm":
            raise ValueError(f"WASM backend does not support emit mode {emit_mode!r}")
        triple = target_triple or (
            "wasm32-unknown-unknown"
            if target == "wasm-freestanding"
            else "wasm32-wasip1"
            if target == "wasm"
            else target
        )
        if (
            not wasm_alias
            and target_triple is not None
            and target_triple.strip().lower() != target
        ):
            raise ValueError("Backend target and explicit target triple disagree")
        return BackendArtifactContract(BackendArtifactKind.WASM, triple)
    if target == "native":
        triple = target_triple
    else:
        if target_triple is not None and target_triple.strip().lower() != target:
            raise ValueError("Backend target and explicit target triple disagree")
        triple = target
    native_kind = NativeArtifactKind.for_emit_mode(emit_mode)
    return BackendArtifactContract(
        BackendArtifactKind.NATIVE_OBJECT
        if native_kind is NativeArtifactKind.OBJECT
        else BackendArtifactKind.NATIVE_ARCHIVE,
        triple,
    )
