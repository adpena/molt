"""Requested backend bytes: one authority for naming, cache identity and admission.

This contract validates container/target shape, not program semantics. Native
symbol closure remains a separate required cache admission step; text outputs
never enter that native reader. Filename extensions are projections, not input
evidence, and no encoding fact enables a compiler support-matrix cell.

NativeTargetSpec preserves whether the request selected the default host or an
explicit target. Its triple always names artifact identity; is_host controls
host toolchain policy, and cargo_target alone projects Cargo's native None wire
value. Explicitly spelling the host triple remains an explicit toolchain/Cargo
selection while sharing the same artifact target identity.
"""

from __future__ import annotations

from molt.compilation_target import (
    Target,
    TargetKind,
    is_wasm_target,
    native_target_spec,
    target_triple as project_target_triple,
)

import codecs
from contextlib import nullcontext
from dataclasses import InitVar, dataclass, field
from enum import Enum
import hashlib
import json
from pathlib import Path
from typing import BinaryIO

from molt.native_target import (
    NativeArtifactKind,
    NativeTargetSpec,
    resolve_native_target_spec,
)
from molt.compilation_target import target_is_wasm
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
from molt.toolchain_identity import (
    StableRegularFileHandle,
    open_stable_regular_file,
    iter_stable_regular_file_chunks,
)


class BackendArtifactKind(str, Enum):
    NATIVE_OBJECT = "native-object"
    NATIVE_ARCHIVE = "native-archive"
    # LLVM's relocatable wasm32 object: the application half of a WASM link
    # whose other half is the shared stdlib archive. It is compiled by a
    # native job with a wasm32 target triple.
    WASM_OBJECT = "wasm-object"
    # The shared stdlib archive of relocatable wasm32 objects that a
    # WASM_OBJECT application links with.
    WASM_ARCHIVE = "wasm-archive"
    MLIR = "mlir"


class BackendArtifactValidationError(OSError):
    """A backend artifact does not satisfy its explicit output request."""


@dataclass(frozen=True, slots=True)
class BackendArtifactContract:
    kind: BackendArtifactKind
    target_triple: str | None = None
    admitted_native: InitVar[NativeTargetSpec | None] = None
    _native_target: NativeTargetSpec | None = field(init=False, repr=False)

    def __post_init__(self, admitted_native: NativeTargetSpec | None) -> None:
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
                raise ValueError(f"WASM artifacts require a WASM target: {triple}")
        try:
            native_target = (
                (admitted_native or resolve_native_target_spec(triple))
                if self.is_native
                else None
            )
        except RuntimeError as error:
            raise ValueError(str(error)) from error
        if admitted_native is not None and (
            not self.is_native or triple not in (None, admitted_native.triple)
        ):
            raise ValueError("Backend artifact and admitted native target disagree")
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
        return self.kind in {
            BackendArtifactKind.WASM_OBJECT,
            BackendArtifactKind.WASM_ARCHIVE,
        }

    @property
    def native_job_kind(self) -> NativeArtifactKind | None:
        """The transport kind a native compile job writes, if one does."""
        if self.kind is BackendArtifactKind.WASM_OBJECT:
            return NativeArtifactKind.OBJECT
        return self.native_kind

    @property
    def is_text(self) -> bool:
        return self.kind in {
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
            BackendArtifactKind.WASM_OBJECT: ".wasm",
            BackendArtifactKind.WASM_ARCHIVE: ".a",
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
        if enabled and self.kind not in {
            BackendArtifactKind.NATIVE_ARCHIVE,
            BackendArtifactKind.WASM_OBJECT,
        }:
            raise ValueError(
                "Shared stdlib extraction requires native archive or wasm32 "
                "object output"
            )

    def validate_archive_shape(
        self, path: Path, *, opened: StableRegularFileHandle | None = None
    ) -> None:
        """Admit a compiler archive: every member is an object of its target."""
        if self.kind is BackendArtifactKind.WASM_ARCHIVE:
            self._validate_wasm_archive(path, opened=opened)
            return
        if self.kind is not BackendArtifactKind.NATIVE_ARCHIVE:
            raise ValueError("Archive shape validation requires an archive contract")
        self.validate_native_shape(path, opened=opened)

    def _validate_wasm_archive(
        self, path: Path, *, opened: StableRegularFileHandle | None
    ) -> None:
        def visit_member(member: StaticArchiveMember, stream: BinaryIO) -> None:
            stream.seek(member.content_offset)
            error = wasm_relocatable_object_error(stream.read(member.size))
            if error is not None:
                raise ValueError(f"archive member {member.name!r}: {error}")

        try:
            with (
                open_stable_regular_file(path, label="backend wasm archive")
                if opened is None
                else nullcontext(opened)
            ) as opened:
                if (
                    visit_static_archive_members(
                        path, visit_member=visit_member, opened=opened
                    )
                    == 0
                ):
                    raise ValueError("backend archive has no relocatable members")
        except (OSError, ValueError, RuntimeError) as error:
            raise BackendArtifactValidationError(
                f"Invalid backend {self.kind.value} artifact {path}: {error}"
            ) from error

    def validate_native_shape(
        self, path: Path, *, opened: StableRegularFileHandle | None = None
    ) -> None:
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
            if opened is not None and opened.path != path.expanduser().absolute():
                raise NativeArtifactError("native shape handle belongs to another path")
            with (
                open_stable_regular_file(path, label="backend native artifact")
                if opened is None
                else nullcontext(opened)
            ) as opened:
                opened.stream.seek(0)
                if self.native_kind is NativeArtifactKind.ARCHIVE:
                    member_count = visit_static_archive_members(
                        path, visit_member=visit_member, opened=opened
                    )
                    if member_count == 0:
                        raise NativeArtifactError(
                            "backend archive has no relocatable members"
                        )
                else:
                    admit(native_artifact_from_file(opened.stream))
        except (OSError, ValueError, RuntimeError) as error:
            raise BackendArtifactValidationError(
                f"Invalid backend {self.kind.value} artifact {path}: {error}"
            ) from error

    def validate(self, path: Path) -> None:
        if self.is_native:
            self.validate_native_shape(path)
            return
        if self.kind is BackendArtifactKind.WASM_ARCHIVE:
            self._validate_wasm_archive(path, opened=None)
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
                for block in iter_stable_regular_file_chunks(
                    opened, chunk_bytes=64 * 1024
                ):
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


def wasm_relocatable_object_error(data: bytes) -> str | None:
    """Why ``data`` is not a relocatable wasm32 object, or None.

    A core module is relocatable exactly when it carries the ``linking``
    custom section wasm-ld requires of every input.
    """
    if data[:8] != b"\0asm\x01\0\0\0":
        return "not a WebAssembly core module"
    offset = 8

    def leb(position: int) -> tuple[int, int]:
        value = shift = 0
        while True:
            if position >= len(data) or shift > 28:
                raise ValueError("truncated LEB128")
            byte = data[position]
            position += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return value, position

    try:
        while offset < len(data):
            section_id = data[offset]
            size, payload = leb(offset + 1)
            end = payload + size
            if end > len(data):
                return "section extends past the module"
            if section_id == 0:
                name_length, name_start = leb(payload)
                if data[name_start : name_start + name_length] == b"linking":
                    return None
            offset = end
    except ValueError as error:
        return str(error)
    return "module has no linking section (not relocatable)"


def stdlib_archive_contract(target_triple: str | None) -> BackendArtifactContract:
    """The shared stdlib archive's contract for one application target."""
    if target_triple is not None and target_is_wasm(target_triple):
        return BackendArtifactContract(BackendArtifactKind.WASM_ARCHIVE, target_triple)
    return BackendArtifactContract(BackendArtifactKind.NATIVE_ARCHIVE, target_triple)


def resolve_backend_artifact_contract(
    *, target: Target, emit_mode: str
) -> BackendArtifactContract:
    """Project admitted compilation intent into the emitted-byte contract."""
    match target:
        case TargetKind.MLIR:
            text_kind = BackendArtifactKind.MLIR
        case _:
            text_kind = None
    if text_kind is not None:
        if emit_mode != "bin":
            raise ValueError(
                f"Textual backend {text_kind.value} does not support emit mode {emit_mode!r}"
            )
        return BackendArtifactContract(text_kind)
    if is_wasm_target(target):
        if emit_mode != "wasm":
            raise ValueError(f"WASM output does not support emit mode {emit_mode!r}")
        triple = project_target_triple(target)
        return BackendArtifactContract(BackendArtifactKind.WASM_OBJECT, triple)
    spec = native_target_spec(target)
    native_kind = NativeArtifactKind.for_emit_mode(emit_mode)
    return BackendArtifactContract(
        BackendArtifactKind.NATIVE_OBJECT
        if native_kind is NativeArtifactKind.OBJECT
        else BackendArtifactKind.NATIVE_ARCHIVE,
        spec.triple,
        admitted_native=spec,
    )
