from __future__ import annotations

from dataclasses import dataclass, field, replace
import json
import mmap
import os
from pathlib import Path
from typing import (
    TYPE_CHECKING,
    BinaryIO,
    Callable,
    Collection,
    Iterable,
    Literal,
    Mapping,
    Sequence,
    TypedDict,
    cast,
    overload,
)

from molt.file_publication import PublicationDurability

if TYPE_CHECKING:
    from molt.capability_manifest import ResolvedRuntimePolicy
    from molt.toolchain_identity import StableRegularFileIdentity
    from molt.wasm_linking_symbols import WasmLinkingSymbolTable


def encode_wasm_facts_request(modules: Iterable[bytes]) -> tuple[str, bytes]:
    """Frame the scanner's stdin with lengths carried separately in argv."""
    buffers = tuple(modules)
    lengths = ",".join(str(len(module)) for module in buffers)
    return lengths, buffers[0] if len(buffers) == 1 else b"".join(buffers)


def require_current_wasm_execution_manifest(payload: object) -> Mapping[str, object]:
    from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

    if (
        not isinstance(payload, Mapping)
        or type(payload.get("version")) is not int
        or payload.get("version") != WASM_EXECUTION_MANIFEST_VERSION
    ):
        raise ValueError(
            f"Molt deployment manifest version must be {WASM_EXECUTION_MANIFEST_VERSION}; "
            "rebuild this deployment with the current Molt compiler"
        )
    _wasm_launch_environment(payload.get("launch"))
    return payload


def _wasm_launch_environment(launch: object) -> Mapping[str, str]:
    from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_REQUIRED_ENVIRONMENT

    if not isinstance(launch, Mapping) or set(launch) != {"environment"}:
        raise ValueError("WASM deployment launch requires exactly environment")
    environment = launch["environment"]
    if not isinstance(environment, Mapping):
        raise ValueError("WASM deployment launch.environment must be a string map")
    for name, value in environment.items():
        if not isinstance(name, str) or not name or "=" in name or "\x00" in name:
            raise ValueError("WASM deployment launch environment name is invalid")
        if not isinstance(value, str) or "\x00" in value:
            raise ValueError("WASM deployment launch environment value is invalid")
    if any(
        name not in environment for name in WASM_EXECUTION_MANIFEST_REQUIRED_ENVIRONMENT
    ):
        raise ValueError(
            "WASM deployment launch environment lacks required policy entries"
        )
    return cast(Mapping[str, str], environment)


def wasm_execution_manifest_launch(policy: ResolvedRuntimePolicy) -> dict[str, object]:
    """One producer-owned projection; hosts do not resolve policy again."""
    launch = {"environment": policy.to_env_vars()}
    _wasm_launch_environment(launch)
    return launch


def wasm_execution_manifest_environment(payload: object) -> dict[str, str]:
    """Copy the admitted launch overlay, preserving omitted ambient options."""
    manifest = require_current_wasm_execution_manifest(payload)
    launch = cast(Mapping[str, object], manifest["launch"])
    return dict(cast(Mapping[str, str], launch["environment"]))


def wasm_execution_manifest_module_descriptor(
    payload: object, role: str
) -> Mapping[str, object]:
    manifest = require_current_wasm_execution_manifest(payload)
    modules = manifest.get("modules")
    descriptor = modules.get(role) if isinstance(modules, Mapping) else None
    if not isinstance(descriptor, Mapping):
        raise ValueError(f"WASM execution manifest missing modules.{role}")
    name, size, digest = (
        descriptor.get("path"),
        descriptor.get("size"),
        descriptor.get("sha256"),
    )
    if (
        not isinstance(name, str)
        or not name
        or name in {".", ".."}
        or any(character in name for character in "/\\:")
    ):
        raise ValueError(
            f"WASM execution manifest modules.{role}.path must name an adjacent file"
        )
    if type(size) is not int or size < 0:
        raise ValueError(f"WASM execution manifest modules.{role}.size is invalid")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise ValueError(f"WASM execution manifest modules.{role}.sha256 is invalid")
    return descriptor


def verify_wasm_execution_manifest_module(
    payload: object, manifest_path: Path, role: str
) -> StableRegularFileIdentity:
    from molt.toolchain_identity import stable_regular_file_identity

    descriptor = wasm_execution_manifest_module_descriptor(payload, role)
    observed = stable_regular_file_identity(
        manifest_path.parent / str(descriptor["path"]), label=f"WASM deployment {role}"
    )
    if observed.size != descriptor["size"]:
        raise ValueError(f"WASM execution manifest modules.{role} size mismatch")
    if observed.sha256 != descriptor["sha256"]:
        raise ValueError(f"WASM execution manifest modules.{role} SHA-256 mismatch")
    return observed


WASM_HEADER = b"\x00asm\x01\x00\x00\x00"
WASM_FINAL_ARTIFACT_FORBIDDEN_CUSTOM_SECTIONS = frozenset({"linking"})
WASM_RUNTIME_MANIFEST_NAME = "manifest.json"

WASM_SECTION_NAMES: dict[int, str] = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data_count",
}

_VALUE_TYPE_NAMES = {
    0x7F: "i32",
    0x7E: "i64",
    0x7D: "f32",
    0x7C: "f64",
    0x7B: "v128",
    0x70: "funcref",
    0x6F: "externref",
}

WASM_EXTERN_KIND_FUNCTION = 0
WASM_EXTERN_KIND_TABLE = 1
WASM_EXTERN_KIND_MEMORY = 2
WASM_EXTERN_KIND_GLOBAL = 3
WASM_EXTERN_KIND_TAG = 4
WASM_VALUE_TYPE_I32 = 0x7F


class _WasmFileBuffer:
    """Bounded reads of sections from an already opened stable regular file."""

    def __init__(self, stream: BinaryIO, size: int) -> None:
        self._stream = stream
        self._size = size

    def __len__(self) -> int:
        return self._size

    @overload
    def __getitem__(self, key: int) -> int: ...

    @overload
    def __getitem__(self, key: slice) -> bytes: ...

    def __getitem__(self, key: int | slice) -> int | bytes:
        if isinstance(key, int):
            return self._read(key, 1)[0]
        start, stop, step = key.indices(self._size)
        if step != 1:
            raise ValueError("WASM file reads require a contiguous slice")
        return self._read(start, max(0, stop - start))

    def _read(self, offset: int, size: int) -> bytes:
        self._stream.seek(offset)
        data = self._stream.read(size)
        if len(data) != size:
            raise ValueError("WASM file changed while reading its sections")
        return data


WasmBuffer = bytes | mmap.mmap | _WasmFileBuffer


def wasm_runtime_manifest_path(artifact: Path) -> Path:
    """Return the sole execution manifest adjacent to a produced WASM artifact."""
    manifest = artifact.parent / WASM_RUNTIME_MANIFEST_NAME
    if not manifest.is_file():
        raise FileNotFoundError(
            f"WASM execution manifest missing for {artifact}: {manifest}"
        )
    return manifest


def wasm_runtime_manifest_entry_path(manifest: Path) -> Path:
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    require_current_wasm_execution_manifest(payload)
    mode = payload.get("mode")
    label = {"linked": "linked", "split-runtime": "app"}.get(mode)
    if label is None:
        raise ValueError(
            f"WASM execution manifest has unsupported mode {mode!r}: {manifest}"
        )
    modules = payload.get("modules")
    descriptor = modules.get(label) if isinstance(modules, dict) else None
    module_path = descriptor.get("path") if isinstance(descriptor, dict) else None
    if not isinstance(module_path, str) or not module_path:
        raise ValueError(
            f"WASM execution manifest missing modules.{label}.path: {manifest}"
        )
    return manifest.parent / module_path


def copy_wasm_runtime_manifest_for_artifact(
    source_artifact: Path, destination_artifact: Path
) -> Path:
    """Copy an execution manifest while rebasing every module path.

    Artifact profilers use this when measuring a fresh pathname. The copied
    manifest remains the sole module authority and keeps every unchanged sibling
    module bound to its original bytes while pointing the selected descriptor at
    the copied artifact.
    """
    source_manifest = wasm_runtime_manifest_path(source_artifact)
    payload = json.loads(source_manifest.read_text(encoding="utf-8"))
    require_current_wasm_execution_manifest(payload)
    modules = payload.get("modules")
    if not isinstance(modules, dict):
        raise ValueError(
            f"WASM execution manifest has no modules object: {source_manifest}"
        )
    source_resolved = source_artifact.resolve()
    destination_root = destination_artifact.parent.resolve()
    matched = False
    for label, descriptor in modules.items():
        if not isinstance(descriptor, dict) or not isinstance(
            descriptor.get("path"), str
        ):
            continue
        resolved = (source_manifest.parent / descriptor["path"]).resolve()
        if resolved == source_resolved:
            resolved = destination_artifact.resolve()
            matched = True
        descriptor["path"] = os.path.relpath(resolved, destination_root).replace(
            os.sep, "/"
        )
    if not matched:
        raise ValueError(
            f"WASM execution manifest does not reference {source_artifact}: {source_manifest}"
        )
    destination_manifest = destination_artifact.parent / WASM_RUNTIME_MANIFEST_NAME
    destination_manifest.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return destination_manifest


def wasm_section_name(section_id: int) -> str:
    return WASM_SECTION_NAMES.get(section_id, f"unknown({section_id})")


@dataclass(frozen=True)
class WasmSectionSpan:
    id: int
    offset: int
    size: int
    custom_name: str = ""

    @property
    def name(self) -> str:
        return wasm_section_name(self.id)

    def to_dict(self) -> dict[str, object]:
        data: dict[str, object] = {
            "id": self.id,
            "name": self.name,
            "offset": self.offset,
            "size": self.size,
        }
        if self.custom_name:
            data["custom_name"] = self.custom_name
        return data


@dataclass(frozen=True)
class WasmImport:
    module: str
    name: str
    kind: int
    type_index: int | None = None
    limits_flags: int | None = None
    minimum: int | None = None
    maximum: int | None = None
    reference_type: bytes | None = None
    value_type: bytes | None = None
    mutable: bool | None = None
    tag_attribute: int | None = None
    description: bytes = field(default=b"", compare=False, repr=False)


@dataclass(frozen=True)
class WasmImportInterface:
    imports: tuple[WasmImport, ...]
    function_signatures: tuple[
        tuple[str, str, tuple[str, ...], str],
        ...,
    ]


@dataclass(frozen=True)
class WasmExport:
    name: str
    kind: int
    index: int

    def to_tuple(self) -> tuple[str, int, int]:
        return self.name, self.kind, self.index


@dataclass(frozen=True)
class WasmRelocatableObjectInterface:
    imports: tuple[WasmImport, ...]
    function_import_signatures: tuple[
        tuple[str, str, tuple[str, ...], str],
        ...,
    ]
    function_exports: tuple[WasmExport, ...]
    linking_symbols: WasmLinkingSymbolTable
    function_definition_signatures: tuple[tuple[str, tuple[str, ...], str], ...] = ()


@dataclass(frozen=True)
class WasmTypeEntry:
    """One type-index-bearing entry from a WebAssembly type section."""

    encoding: bytes
    function_signature: tuple[tuple[bytes, ...], tuple[bytes, ...]] | None
    is_plain_function: bool


@dataclass(frozen=True)
class WasmTypeGroup:
    """One top-level type-section group and its sequential type entries."""

    recursive: bool
    entries: tuple[WasmTypeEntry, ...]


@dataclass(frozen=True)
class WasmGlobal:
    """One defined WebAssembly global with its absolute global index.

    ``i32_const`` is populated only for the canonical address-bearing shape:
    an immutable or mutable i32 global initialized directly by ``i32.const``.
    Consumers must still check ``value_type`` and ``mutable`` before treating
    the value as a published linear-memory address.
    """

    index: int
    value_type: int
    mutable: bool
    initializer_opcode: int
    i32_const: int | None
    i32_const_canonical: bool


@dataclass(frozen=True)
class WasmCodeMetrics:
    defined_function_count: int
    code_section_size: int

    def to_tuple(self) -> tuple[int, int]:
        return self.defined_function_count, self.code_section_size


@dataclass(frozen=True)
class WasmPublicationTransformMetrics:
    changed: bool
    input_bytes: int
    output_bytes: int
    scanned_bytes: int
    written_bytes: int
    max_buffer_bytes: int


@dataclass(frozen=True)
class WasmFunctionBody:
    index: int
    offset: int
    body_size_bytes: int
    name: str | None = None

    def to_dict(self) -> dict[str, object]:
        data: dict[str, object] = {
            "index": self.index,
            "offset": self.offset,
            "body_size_bytes": self.body_size_bytes,
        }
        if self.name is not None:
            data["name"] = self.name
        return data


def _read_wasm_varuint(
    data: WasmBuffer, offset: int, *, limit: int | None = None
) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data) or (limit is not None and offset >= limit):
            raise ValueError("Unexpected EOF while reading wasm varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 63:
            raise ValueError("wasm varuint is too large")


def _read_wasm_string(
    data: WasmBuffer, offset: int, *, limit: int | None = None
) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset, limit=limit)
    end = offset + length
    if end > len(data) or (limit is not None and end > limit):
        raise ValueError("Unexpected EOF while reading wasm string")
    return data[offset:end].decode("utf-8"), end


def _write_wasm_varuint(value: int) -> bytes:
    if value < 0:
        raise ValueError("wasm varuint must be non-negative")
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _write_wasm_string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _write_wasm_varuint(len(encoded)) + encoded


def _read_wasm_custom_section_name(
    data: WasmBuffer, offset: int = 0, limit: int | None = None
) -> str:
    if limit is None:
        limit = len(data)
    if offset >= limit:
        return ""
    try:
        name, _ = _read_wasm_string(data, offset, limit=limit)
    except (UnicodeDecodeError, ValueError):
        return "<unparseable>"
    return name


def wasm_custom_section_names(data: bytes) -> tuple[str, ...]:
    return tuple(
        _read_wasm_custom_section_name(payload)
        for section_id, payload in parse_wasm_sections(data)
        if section_id == 0
    )


def is_wasm_debug_custom_section(name: str) -> bool:
    return (
        name == "name" or name.startswith(".debug") or name.startswith("reloc..debug")
    )


def is_wasm_final_artifact_forbidden_custom_section(name: str) -> bool:
    return name in WASM_FINAL_ARTIFACT_FORBIDDEN_CUSTOM_SECTIONS or name.startswith(
        "reloc."
    )


def wasm_publication_policy_violations(
    data: bytes,
    *,
    final_artifact: bool,
    preserve_debug: bool,
) -> tuple[str, ...]:
    violations: list[str] = []
    for name in wasm_custom_section_names(data):
        if final_artifact and is_wasm_final_artifact_forbidden_custom_section(name):
            violations.append(name)
        elif not preserve_debug and is_wasm_debug_custom_section(name):
            violations.append(name)
    return tuple(violations)


def parse_wasm_section_spans(
    data: WasmBuffer,
    *,
    allow_duplicate_standard_sections: bool = False,
    strict_custom_names: bool = False,
) -> list[WasmSectionSpan]:
    if len(data) < len(WASM_HEADER) or data[0 : len(WASM_HEADER)] != WASM_HEADER:
        raise ValueError("Invalid wasm binary")
    offset = len(WASM_HEADER)
    sections: list[WasmSectionSpan] = []
    standard_section_ids: set[int] = set()
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        if section_id != 0:
            if (
                section_id in standard_section_ids
                and not allow_duplicate_standard_sections
            ):
                raise ValueError(f"Duplicate wasm section id {section_id}")
            standard_section_ids.add(section_id)
        section_size, offset = _read_wasm_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError("Invalid wasm section length")
        custom_name = ""
        if section_id == 0:
            if strict_custom_names:
                custom_name, _ = _read_wasm_string(data, offset, limit=section_end)
            else:
                custom_name = _read_wasm_custom_section_name(data, offset, section_end)
        sections.append(
            WasmSectionSpan(
                id=section_id,
                offset=offset,
                size=section_size,
                custom_name=custom_name,
            )
        )
        offset = section_end
    return sections


def read_wasm_section_spans(path: Path) -> list[WasmSectionSpan]:
    from molt.toolchain_identity import open_stable_regular_file

    with open_stable_regular_file(
        path.resolve(strict=True), label="WASM section directory"
    ) as source:
        return parse_wasm_section_spans(
            _WasmFileBuffer(source.stream, source.stat.st_size)
        )


def _wasm_sections_from_spans(
    data: bytes,
    spans: Sequence[WasmSectionSpan],
) -> list[tuple[int, bytes]]:
    return [(span.id, data[span.offset : span.offset + span.size]) for span in spans]


def parse_wasm_sections(
    data: bytes,
    *,
    allow_duplicate_standard_sections: bool = False,
) -> list[tuple[int, bytes]]:
    """Return the ordered section payloads for one core module.

    Duplicate standard sections are rejected by default. The explicit opt-in is
    reserved for the linker's normalization pass, which consumes invalid
    intermediate linker output and returns a strict single-section module.
    """

    return _wasm_sections_from_spans(
        data,
        parse_wasm_section_spans(
            data,
            allow_duplicate_standard_sections=allow_duplicate_standard_sections,
        ),
    )


def _parse_wasm_file_sections(path: Path) -> list[tuple[int, bytes]]:
    return parse_wasm_sections(path.read_bytes())


def inspect_wasm_binary(path: Path) -> Literal["missing", "invalid", "valid"]:
    try:
        read_wasm_section_spans(path)
    except OSError:
        return "missing"
    except ValueError:
        return "invalid"
    return "valid"


def is_valid_wasm_binary(path: Path) -> bool:
    return inspect_wasm_binary(path) == "valid"


def has_nonempty_wasm_code_section(path: Path) -> bool:
    return read_wasm_code_metrics(path).defined_function_count > 0


def _build_wasm_sections(sections: Sequence[tuple[int, bytes]]) -> bytes:
    out = bytearray(WASM_HEADER)
    for section_id, payload in sections:
        out.append(section_id)
        out.extend(_write_wasm_varuint(len(payload)))
        out.extend(payload)
    return bytes(out)


def read_wasm_limits(data: bytes, offset: int) -> tuple[int, int, int | None, int]:
    flags, offset = _read_wasm_varuint(data, offset)
    minimum, offset = _read_wasm_varuint(data, offset)
    maximum: int | None = None
    if flags & 0x1:
        maximum, offset = _read_wasm_varuint(data, offset)
    return flags, minimum, maximum, offset


def write_wasm_limits(flags: int, minimum: int, maximum: int | None) -> bytes:
    output = bytearray(_write_wasm_varuint(flags))
    output.extend(_write_wasm_varuint(minimum))
    if flags & 0x1:
        if maximum is None:
            raise ValueError("WASM limits with a maximum flag require a maximum")
        output.extend(_write_wasm_varuint(maximum))
    return bytes(output)


def _read_wasm_import_description(
    payload: bytes, cursor: int, *, module: str, name: str, kind: int
) -> tuple[WasmImport, int]:
    if kind == 0:
        type_index, cursor = _read_wasm_varuint(payload, cursor)
        return WasmImport(module, name, kind, type_index=type_index), cursor
    if kind == 1:
        reference_type, cursor = _read_wasm_value_type_encoding(payload, cursor)
        limits_flags, minimum, maximum, cursor = read_wasm_limits(payload, cursor)
        return (
            WasmImport(
                module,
                name,
                kind,
                limits_flags=limits_flags,
                minimum=minimum,
                maximum=maximum,
                reference_type=reference_type,
            ),
            cursor,
        )
    if kind == 2:
        limits_flags, minimum, maximum, cursor = read_wasm_limits(payload, cursor)
        return (
            WasmImport(
                module,
                name,
                kind,
                limits_flags=limits_flags,
                minimum=minimum,
                maximum=maximum,
            ),
            cursor,
        )
    if kind == 3:
        value_type, cursor = _read_wasm_value_type_encoding(payload, cursor)
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading global mutability")
        mutable = payload[cursor]
        cursor += 1
        if mutable not in (0, 1):
            raise ValueError(f"Invalid wasm global mutability {mutable}")
        return (
            WasmImport(
                module,
                name,
                kind,
                value_type=value_type,
                mutable=bool(mutable),
            ),
            cursor,
        )
    if kind == 4:
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading tag attribute")
        tag_attribute = payload[cursor]
        cursor += 1
        type_index, cursor = _read_wasm_varuint(payload, cursor)
        return (
            WasmImport(
                module,
                name,
                kind,
                type_index=type_index,
                tag_attribute=tag_attribute,
            ),
            cursor,
        )
    raise ValueError(f"Unknown wasm import kind {kind}")


def skip_wasm_import_description(data: bytes, offset: int, kind: int) -> int:
    """Return the end of one canonical import descriptor."""

    _, offset = _read_wasm_import_description(
        data,
        offset,
        module="",
        name="",
        kind=kind,
    )
    return offset


def _iter_wasm_imports(
    sections: Sequence[tuple[int, bytes]],
) -> list[WasmImport]:
    for section_id, payload in sections:
        if section_id == 2:
            return parse_wasm_import_section(payload)
    return []


def parse_wasm_import_section(payload: bytes) -> list[WasmImport]:
    """Parse one complete import-section payload and reject trailing data."""

    imports: list[WasmImport] = []
    cursor = 0
    count, cursor = _read_wasm_varuint(payload, cursor)
    for _ in range(count):
        module, cursor = _read_wasm_string(payload, cursor)
        name, cursor = _read_wasm_string(payload, cursor)
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading import")
        kind = payload[cursor]
        cursor += 1
        description_start = cursor
        wasm_import, cursor = _read_wasm_import_description(
            payload, cursor, module=module, name=name, kind=kind
        )
        imports.append(
            replace(
                wasm_import,
                description=payload[description_start:cursor],
            )
        )
    if cursor != len(payload):
        raise ValueError("Trailing bytes in wasm import section")
    return imports


def parse_wasm_imports(
    data: bytes, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmImport]:
    try:
        return _iter_wasm_imports(parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return []
        raise


def _wasm_import_interface_from_sections(
    sections: Sequence[tuple[int, bytes]],
    *,
    signature_import_names: Collection[str] | None = None,
) -> WasmImportInterface:
    imports = tuple(_iter_wasm_imports(sections))
    type_signatures = _read_wasm_type_signature_encodings(sections)
    signatures: list[tuple[str, str, tuple[str, ...], str]] = []
    for wasm_import in imports:
        if wasm_import.kind != WASM_EXTERN_KIND_FUNCTION:
            continue
        type_index = wasm_import.type_index
        if type_index is None:
            raise ValueError(
                f"Missing wasm function import type for {wasm_import.name}"
            )
        if type_index not in type_signatures:
            raise ValueError(
                f"Missing wasm type index {type_index} for import "
                f"{wasm_import.module}.{wasm_import.name}"
            )
        encoded_signature = type_signatures[type_index]
        if encoded_signature is None:
            raise ValueError(
                f"Wasm function import {wasm_import.module}.{wasm_import.name} "
                f"references non-function type index {type_index}"
            )
        if (
            signature_import_names is not None
            and wasm_import.name not in signature_import_names
        ):
            continue
        encoded_params, encoded_results = encoded_signature
        params = tuple(
            _format_wasm_value_type_encoding(value) for value in encoded_params
        )
        results = tuple(
            _format_wasm_value_type_encoding(value) for value in encoded_results
        )
        signatures.append(
            (
                wasm_import.module,
                wasm_import.name,
                params,
                _format_wasm_result_kind(results),
            )
        )
    return WasmImportInterface(imports, tuple(signatures))


def parse_wasm_import_interface(
    data: bytes,
    *,
    on_error: Literal["raise", "ignore"] = "raise",
) -> WasmImportInterface:
    try:
        return _wasm_import_interface_from_sections(parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return WasmImportInterface((), ())
        raise


def read_wasm_imports(
    path: Path, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmImport]:
    try:
        return parse_wasm_imports(path.read_bytes(), on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return []
        raise


def read_wasm_import_interface(
    path: Path,
    *,
    on_error: Literal["raise", "ignore"] = "raise",
) -> WasmImportInterface:
    try:
        return parse_wasm_import_interface(path.read_bytes(), on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return WasmImportInterface((), ())
        raise


def _read_wasm_global_initializer(
    data: bytes, offset: int
) -> tuple[int, int | None, bool, int]:
    """Read one MVP/reference-types constant initializer without evaluating it."""
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm global initializer")
    opcode = data[offset]
    offset += 1
    i32_const: int | None = None
    i32_const_canonical = False
    if opcode == 0x41:  # i32.const
        immediate_start = offset
        i32_const, offset = _read_wasm_varint(data, offset, 32)
        i32_const_canonical = data[immediate_start:offset] == _encode_wasm_varint(
            i32_const, 32
        )
    elif opcode == 0x42:  # i64.const
        _, offset = _read_wasm_varint(data, offset, 64)
    elif opcode == 0x43:  # f32.const
        offset += 4
    elif opcode == 0x44:  # f64.const
        offset += 8
    elif opcode == 0x23:  # global.get
        _, offset = _read_wasm_varuint(data, offset)
    elif opcode == 0xD0:  # ref.null
        _, offset = _read_wasm_varint(data, offset, 33)
    elif opcode == 0xD2:  # ref.func
        _, offset = _read_wasm_varuint(data, offset)
    else:
        raise ValueError(f"Unsupported wasm global initializer opcode 0x{opcode:02x}")
    if offset > len(data) or offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Malformed wasm global initializer")
    return opcode, i32_const, i32_const_canonical, offset + 1


def _iter_wasm_defined_globals(
    sections: Sequence[tuple[int, bytes]],
) -> list[WasmGlobal]:
    imported_globals = sum(
        wasm_import.kind == WASM_EXTERN_KIND_GLOBAL
        for wasm_import in _iter_wasm_imports(sections)
    )
    globals_: list[WasmGlobal] = []
    for section_id, payload in sections:
        if section_id != 6:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for defined_index in range(count):
            if cursor + 2 > len(payload):
                raise ValueError("Unexpected EOF while reading wasm global type")
            value_type = payload[cursor]
            mutable_raw = payload[cursor + 1]
            cursor += 2
            if mutable_raw not in (0, 1):
                raise ValueError(f"Invalid wasm global mutability {mutable_raw}")
            opcode, i32_const, i32_const_canonical, cursor = (
                _read_wasm_global_initializer(payload, cursor)
            )
            globals_.append(
                WasmGlobal(
                    index=imported_globals + defined_index,
                    value_type=value_type,
                    mutable=bool(mutable_raw),
                    initializer_opcode=opcode,
                    i32_const=i32_const,
                    i32_const_canonical=i32_const_canonical,
                )
            )
        if cursor != len(payload):
            raise ValueError("Trailing wasm global bytes")
        break
    return globals_


def parse_wasm_defined_globals(
    data: bytes, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmGlobal]:
    try:
        return _iter_wasm_defined_globals(parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return []
        raise


def read_wasm_defined_globals(
    path: Path, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmGlobal]:
    try:
        return parse_wasm_defined_globals(path.read_bytes(), on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return []
        raise


def _collect_wasm_export_names(path: Path) -> set[str]:
    try:
        return {
            export.name
            for export in _iter_wasm_exports(_parse_wasm_file_sections(path))
        }
    except (OSError, UnicodeDecodeError, ValueError, IndexError):
        return set()


def _wasm_import_minima(path: Path) -> tuple[int | None, int | None]:
    return _wasm_import_minima_from_imports(
        _iter_wasm_imports(_parse_wasm_file_sections(path))
    )


def _wasm_import_minima_from_imports(
    imports: Iterable[WasmImport],
) -> tuple[int | None, int | None]:
    memory_min: int | None = None
    table_min: int | None = None
    for wasm_import in imports:
        if (
            wasm_import.kind == 1
            and wasm_import.module == "env"
            and wasm_import.name == "__indirect_function_table"
        ):
            table_min = wasm_import.minimum
        elif (
            wasm_import.kind == 2
            and wasm_import.module == "env"
            and wasm_import.name == "memory"
        ):
            memory_min = wasm_import.minimum
    return memory_min, table_min


def _read_wasm_varint(data: bytes, offset: int, bits: int) -> tuple[int, int]:
    result = 0
    shift = 0
    maximum_bytes = (bits + 6) // 7
    for byte_index in range(maximum_bytes):
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            break
        if byte_index + 1 == maximum_bytes:
            raise ValueError("varint too large")
    else:  # pragma: no cover - the continuation-bit case raises in the loop
        raise ValueError("varint too large")
    if byte & 0x40:
        result |= -1 << shift
    minimum = -(1 << (bits - 1))
    maximum = (1 << (bits - 1)) - 1
    if not minimum <= result <= maximum:
        raise ValueError(f"signed {bits}-bit integer is out of range")
    return result, offset


def _encode_wasm_varint(value: int, bits: int) -> bytes:
    minimum = -(1 << (bits - 1))
    maximum = (1 << (bits - 1)) - 1
    if not minimum <= value <= maximum:
        raise ValueError(f"signed {bits}-bit integer is out of range")
    encoded = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        done = (remaining == 0 and byte & 0x40 == 0) or (
            remaining == -1 and byte & 0x40 != 0
        )
        encoded.append(byte if done else byte | 0x80)
        if done:
            return bytes(encoded)


def _read_wasm_const_expr_i32(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading const expr")
    opcode = data[offset]
    offset += 1
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varint(data, offset, 32)
    elif opcode == 0x42:  # i64.const
        value, offset = _read_wasm_varint(data, offset, 64)
    else:
        raise ValueError("Unsupported const expr opcode")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Invalid const expr terminator")
    offset += 1
    return value, offset


def _wasm_table_min_from_sections(
    sections: Sequence[tuple[int, bytes]],
) -> int | None:
    for wasm_import in _iter_wasm_imports(sections):
        if (
            wasm_import.kind == 1
            and wasm_import.module == "env"
            and wasm_import.name == "__indirect_function_table"
        ):
            return wasm_import.minimum
    return None


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        return _wasm_table_min_from_sections(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return None


def _read_wasm_data_end(path: Path) -> int | None:
    try:
        sections = _parse_wasm_file_sections(path)
    except (OSError, ValueError):
        return None
    return _wasm_data_end_from_sections(sections)


def _wasm_data_end_from_sections(sections: Sequence[tuple[int, bytes]]) -> int | None:
    max_end = None
    try:
        for section_id, payload in sections:
            if section_id != 11:
                continue
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading data segment")
                flags = payload[cursor]
                cursor += 1
                is_passive = flags & 0x1
                has_memidx = flags & 0x2
                if has_memidx:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                if is_passive:
                    size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                    cursor += size_bytes
                    continue
                offset_val, cursor = _read_wasm_const_expr_i32(payload, cursor)
                size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                cursor += size_bytes
                if offset_val < 0:
                    continue
                end_val = offset_val + size_bytes
                if max_end is None or end_val > max_end:
                    max_end = end_val
    except ValueError:
        return None
    return max_end


def _read_wasm_memory_min_bytes(path: Path) -> int | None:
    try:
        sections = _parse_wasm_file_sections(path)
    except (OSError, ValueError):
        return None
    return _wasm_memory_min_bytes_from_sections(sections)


def _wasm_memory_min_bytes_from_sections(
    sections: Sequence[tuple[int, bytes]],
) -> int | None:
    memory_pages: int | None = None
    try:
        for wasm_import in _iter_wasm_imports(sections):
            if (
                wasm_import.kind == 2
                and wasm_import.module == "env"
                and wasm_import.name == "memory"
            ):
                memory_pages = max(memory_pages or 0, wasm_import.minimum or 0)
        for section_id, payload in sections:
            cursor = 0
            if section_id == 5:
                count, cursor = _read_wasm_varuint(payload, cursor)
                for _ in range(count):
                    _, minimum, _, cursor = read_wasm_limits(payload, cursor)
                    memory_pages = max(memory_pages or 0, minimum)
    except ValueError:
        return None
    if memory_pages is None:
        return None
    return memory_pages * 65536


def _collect_wasm_module_import_names(path: Path, module_name: str) -> set[str]:
    try:
        imports = _iter_wasm_imports(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return set()
    return {
        wasm_import.name for wasm_import in imports if wasm_import.module == module_name
    }


def _read_wasm_import_metrics(path: Path) -> dict[str, int] | None:
    try:
        imports = _iter_wasm_imports(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return None
    return {
        "total": len(imports),
        "functions": sum(1 for wasm_import in imports if wasm_import.kind == 0),
        "tables": sum(1 for wasm_import in imports if wasm_import.kind == 1),
    }


def _read_wasm_value_type(data: bytes, offset: int) -> tuple[str, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm value type")
    code = data[offset]
    offset += 1
    name = _VALUE_TYPE_NAMES.get(code)
    if name is None:
        raise ValueError(f"Unsupported wasm value type 0x{code:02x}")
    return name, offset


def _read_wasm_value_type_encoding(data: bytes, offset: int) -> tuple[bytes, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm value type")
    start = offset
    form = data[offset]
    offset += 1
    if form in (0x63, 0x64):
        _, offset = _read_wasm_varint(data, offset, 33)
    return data[start:offset], offset


def _format_wasm_value_type_encoding(encoded: bytes) -> str:
    if len(encoded) == 1 and (name := _VALUE_TYPE_NAMES.get(encoded[0])) is not None:
        return name
    if encoded and encoded[0] in (0x63, 0x64):
        heap_type, end = _read_wasm_varint(encoded, 1, 33)
        if end == len(encoded):
            qualifier = "ref null" if encoded[0] == 0x63 else "ref"
            return f"({qualifier} {heap_type})"
    raise ValueError(f"Unsupported wasm value type encoding {encoded.hex()}")


def _read_wasm_encoded_value_type_vec(
    data: bytes, offset: int
) -> tuple[list[bytes], int]:
    count, offset = _read_wasm_varuint(data, offset)
    values: list[bytes] = []
    for _ in range(count):
        value, offset = _read_wasm_value_type_encoding(data, offset)
        values.append(value)
    return values, offset


def _read_wasm_value_type_vec(data: bytes, offset: int) -> tuple[list[str], int]:
    count, offset = _read_wasm_varuint(data, offset)
    values: list[str] = []
    for _ in range(count):
        value, offset = _read_wasm_value_type(data, offset)
        values.append(value)
    return values, offset


def _format_wasm_result_kind(results: Sequence[str]) -> str:
    if not results:
        return "nil"
    return ", ".join(results)


def _read_wasm_type_signatures(
    sections: Sequence[tuple[int, bytes]],
) -> dict[int, tuple[list[str], str]]:
    return {
        type_index: (
            [_format_wasm_value_type_encoding(value) for value in params],
            _format_wasm_result_kind(
                [_format_wasm_value_type_encoding(value) for value in results]
            ),
        )
        for type_index, signature in _read_wasm_type_signature_encodings(
            sections
        ).items()
        if signature is not None
        for params, results in (signature,)
    }


def _read_wasm_gc_field_type(data: bytes, offset: int) -> int:
    _storage_type, offset = _read_wasm_value_type_encoding(data, offset)
    if offset >= len(data) or data[offset] not in (0, 1):
        raise ValueError("Invalid wasm GC field mutability")
    return offset + 1


def _read_wasm_composite_type(
    data: bytes,
    offset: int,
) -> tuple[tuple[tuple[bytes, ...], tuple[bytes, ...]] | None, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm composite type")
    form = data[offset]
    offset += 1
    if form == 0x60:
        params, offset = _read_wasm_encoded_value_type_vec(data, offset)
        results, offset = _read_wasm_encoded_value_type_vec(data, offset)
        return (tuple(params), tuple(results)), offset
    if form == 0x5F:
        field_count, offset = _read_wasm_varuint(data, offset)
        for _ in range(field_count):
            offset = _read_wasm_gc_field_type(data, offset)
        return None, offset
    if form == 0x5E:
        return None, _read_wasm_gc_field_type(data, offset)
    raise ValueError(f"Unsupported wasm composite type form 0x{form:02x}")


def _read_wasm_subtype(
    data: bytes,
    offset: int,
) -> tuple[tuple[tuple[bytes, ...], tuple[bytes, ...]] | None, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm subtype")
    if data[offset] in (0x4F, 0x50):
        offset += 1
        supertype_count, offset = _read_wasm_varuint(data, offset)
        for _ in range(supertype_count):
            _supertype, offset = _read_wasm_varint(data, offset, 33)
    return _read_wasm_composite_type(data, offset)


def parse_wasm_type_section_groups(payload: bytes) -> tuple[WasmTypeGroup, ...]:
    """Parse the canonical type-entry/group structure from a type payload.

    Entries retain their exact encoding so consumers can inspect signatures or
    rewrite group boundaries without implementing another GC-aware byte walk.
    """

    cursor = 0
    group_count, cursor = _read_wasm_varuint(payload, cursor)
    groups: list[WasmTypeGroup] = []
    for _ in range(group_count):
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading wasm type section")
        recursive = payload[cursor] == 0x4E
        if recursive:
            cursor += 1
            member_count, cursor = _read_wasm_varuint(payload, cursor)
            if member_count == 0:
                raise ValueError("Empty wasm recursive type group")
        else:
            member_count = 1

        entries: list[WasmTypeEntry] = []
        for _ in range(member_count):
            start = cursor
            signature, cursor = _read_wasm_subtype(payload, cursor)
            entries.append(
                WasmTypeEntry(
                    encoding=payload[start:cursor],
                    function_signature=signature,
                    is_plain_function=payload[start] == 0x60,
                )
            )
        groups.append(WasmTypeGroup(recursive=recursive, entries=tuple(entries)))

    if cursor != len(payload):
        raise ValueError("Trailing bytes in wasm type section")
    return tuple(groups)


def flatten_wasm_plain_function_rec_groups(data: bytes) -> bytes | None:
    """Flatten recursive groups made solely of plain function types.

    The rewrite preserves entry order and therefore every type index. Real GC
    recursion or explicit subtyping fails closed because removing that group
    boundary would change semantics. Unrelated standalone GC types are retained
    byte-for-byte.
    """

    sections = parse_wasm_sections(data)
    type_section_index = next(
        (
            index
            for index, (section_id, _payload) in enumerate(sections)
            if section_id == 1
        ),
        None,
    )
    if type_section_index is None:
        return None
    payload = sections[type_section_index][1]
    groups = parse_wasm_type_section_groups(payload)
    if not any(group.recursive for group in groups):
        return None

    flattened_entries: list[bytes] = []
    for group in groups:
        if group.recursive:
            for entry in group.entries:
                if not entry.is_plain_function:
                    form = entry.encoding[0]
                    raise ValueError(
                        "rec group flatten: group member is not a plain func "
                        f"type (form {hex(form)}); cannot flatten a real "
                        "recursive/subtype group without changing semantics"
                    )
                flattened_entries.append(entry.encoding)
        else:
            flattened_entries.append(group.entries[0].encoding)

    rewritten_payload = _write_wasm_varuint(len(flattened_entries)) + b"".join(
        flattened_entries
    )
    rewritten_sections = list(sections)
    rewritten_sections[type_section_index] = (1, rewritten_payload)
    return _build_wasm_sections(rewritten_sections)


def _read_wasm_type_signature_encodings(
    sections: Sequence[tuple[int, bytes]],
) -> dict[int, tuple[tuple[bytes, ...], tuple[bytes, ...]] | None]:
    signatures: dict[int, tuple[tuple[bytes, ...], tuple[bytes, ...]] | None] = {}
    for section_id, payload in sections:
        if section_id != 1:
            continue
        type_index = 0
        for group in parse_wasm_type_section_groups(payload):
            for entry in group.entries:
                signatures[type_index] = entry.function_signature
                type_index += 1
        break
    return signatures


def _read_wasm_import_function_type_indices(
    sections: Sequence[tuple[int, bytes]],
) -> tuple[list[tuple[str, str, int]], int]:
    imports: list[tuple[str, str, int]] = []
    import_function_count = 0
    for wasm_import in _iter_wasm_imports(sections):
        if wasm_import.kind != 0:
            continue
        if wasm_import.type_index is None:
            raise ValueError(
                f"Missing wasm function import type for {wasm_import.name}"
            )
        imports.append((wasm_import.module, wasm_import.name, wasm_import.type_index))
        import_function_count += 1
    return imports, import_function_count


def _count_wasm_function_imports(sections: Sequence[tuple[int, bytes]]) -> int:
    return sum(
        1 for wasm_import in _iter_wasm_imports(sections) if wasm_import.kind == 0
    )


def _read_wasm_function_type_indices(
    sections: Sequence[tuple[int, bytes]],
    import_function_type_indices: Sequence[int],
) -> dict[int, int]:
    function_type_indices = {
        index: type_index
        for index, type_index in enumerate(import_function_type_indices)
    }
    function_index = len(import_function_type_indices)
    for section_id, payload in sections:
        if section_id != 3:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for _ in range(count):
            type_index, cursor = _read_wasm_varuint(payload, cursor)
            function_type_indices[function_index] = type_index
            function_index += 1
        break
    return function_type_indices


def _read_wasm_function_name_map(
    data: bytes,
    section_spans: Sequence[WasmSectionSpan],
) -> dict[int, str]:
    name_map: dict[int, str] = {}
    for span in section_spans:
        if span.id != 0 or span.custom_name != "name":
            continue
        pos = span.offset + len(_write_wasm_string("name"))
        end = span.offset + span.size
        while pos < end:
            try:
                subsection_id = data[pos]
                pos += 1
                subsection_size, pos = _read_wasm_varuint(data, pos)
                subsection_end = pos + subsection_size
                if subsection_end > end:
                    raise ValueError("WASM name subsection extends past section")
                if subsection_id == 1:
                    count, pos = _read_wasm_varuint(data, pos)
                    for _ in range(count):
                        if pos >= subsection_end:
                            break
                        func_index, pos = _read_wasm_varuint(data, pos)
                        func_name, pos = _read_wasm_string(data, pos)
                        name_map[func_index] = func_name
                pos = subsection_end
            except (UnicodeDecodeError, ValueError, IndexError):
                break
    return name_map


def read_wasm_function_bodies(path: Path) -> list[WasmFunctionBody]:
    data = path.read_bytes()
    section_spans = parse_wasm_section_spans(data)
    sections = [
        (span.id, data[span.offset : span.offset + span.size]) for span in section_spans
    ]
    imported_function_count = _count_wasm_function_imports(sections)
    name_map = _read_wasm_function_name_map(data, section_spans)

    bodies: list[WasmFunctionBody] = []
    for span in section_spans:
        if span.id != 10:
            continue
        pos = span.offset
        end = span.offset + span.size
        count, pos = _read_wasm_varuint(data, pos)
        for body_index in range(count):
            body_size, pos = _read_wasm_varuint(data, pos)
            body_start = pos
            body_end = body_start + body_size
            if body_end > end:
                raise ValueError("WASM function body extends past code section")
            function_index = imported_function_count + body_index
            bodies.append(
                WasmFunctionBody(
                    index=function_index,
                    offset=body_start,
                    body_size_bytes=body_size,
                    name=name_map.get(function_index),
                )
            )
            pos = body_end
        break
    return bodies


def read_wasm_code_metrics(path: Path) -> WasmCodeMetrics:
    try:
        data = path.read_bytes()
        for span in parse_wasm_section_spans(data):
            if span.id != 10:
                continue
            count, _ = _read_wasm_varuint(data, span.offset)
            return WasmCodeMetrics(count, span.size)
    except (OSError, ValueError, IndexError):
        return WasmCodeMetrics(0, 0)
    return WasmCodeMetrics(0, 0)


def _iter_wasm_exports(
    sections: Sequence[tuple[int, bytes]],
) -> list[WasmExport]:
    for section_id, payload in sections:
        if section_id == 7:
            return parse_wasm_export_section(payload)
    return []


def parse_wasm_export_section(payload: bytes) -> list[WasmExport]:
    """Parse one complete export-section payload and reject trailing data."""

    exports: list[WasmExport] = []
    cursor = 0
    count, cursor = _read_wasm_varuint(payload, cursor)
    for _ in range(count):
        name, cursor = _read_wasm_string(payload, cursor)
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading export")
        kind = payload[cursor]
        cursor += 1
        index, cursor = _read_wasm_varuint(payload, cursor)
        exports.append(WasmExport(name, kind, index))
    if cursor != len(payload):
        raise ValueError("Trailing bytes in wasm export section")
    return exports


def parse_wasm_exports(
    data: bytes,
    *,
    kind: int | None = None,
    on_error: Literal["raise", "ignore"] = "raise",
) -> list[WasmExport]:
    try:
        exports = _iter_wasm_exports(parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return []
        raise
    if kind is None:
        return exports
    return [export for export in exports if export.kind == kind]


def parse_wasm_relocatable_object_interface(
    data: bytes,
    *,
    signature_import_names: Collection[str] | None = (),
) -> WasmRelocatableObjectInterface:
    """Parse one relocatable artifact once for all closure consumers."""
    from molt.wasm_linking_symbols import parse_wasm_linking_symbols

    spans = tuple(parse_wasm_section_spans(data))
    sections = _wasm_sections_from_spans(data, spans)
    import_interface = _wasm_import_interface_from_sections(
        sections,
        signature_import_names=signature_import_names,
    )

    linking_symbols = parse_wasm_linking_symbols(
        data,
        wasm_imports=import_interface.imports,
        section_spans=spans,
    )
    function_types = _read_wasm_function_type_indices(
        sections,
        [
            item.type_index
            for item in import_interface.imports
            if item.kind == WASM_EXTERN_KIND_FUNCTION and item.type_index is not None
        ],
    )
    type_signatures = _read_wasm_type_signature_encodings(sections)
    definitions: list[tuple[str, tuple[str, ...], str]] = []
    for symbol in linking_symbols.function_symbols:
        if not symbol.is_externally_linkable:
            continue
        type_index = (
            function_types.get(symbol.index) if symbol.index is not None else None
        )
        signature = type_signatures.get(type_index) if type_index is not None else None
        if signature is None:
            raise ValueError(
                f"Missing function signature for linking definition {symbol.name!r}"
            )
        params, results = signature
        definitions.append(
            (
                symbol.name,
                tuple(_format_wasm_value_type_encoding(value) for value in params),
                _format_wasm_result_kind(
                    tuple(_format_wasm_value_type_encoding(value) for value in results)
                ),
            )
        )
    return WasmRelocatableObjectInterface(
        imports=import_interface.imports,
        function_import_signatures=import_interface.function_signatures,
        function_exports=tuple(
            export
            for export in _iter_wasm_exports(sections)
            if export.kind == WASM_EXTERN_KIND_FUNCTION
        ),
        linking_symbols=linking_symbols,
        function_definition_signatures=tuple(definitions),
    )


def _renamed_wasm_export_payload(
    payload: bytes,
    rename_map: Mapping[str, str],
    *,
    require_function_exports: bool,
) -> bytes | None:
    exports = parse_wasm_export_section(payload)
    original_names = (
        {export.name for export in exports} if require_function_exports else set()
    )
    renamed: set[str] = set()
    modified = False
    rewritten: list[WasmExport] = []
    seen: dict[str, tuple[int, int]] = {}
    for export in exports:
        name = export.name
        if not require_function_exports or export.kind == WASM_EXTERN_KIND_FUNCTION:
            name = rename_map.get(export.name, export.name)
            if require_function_exports and export.name in rename_map:
                if name in original_names:
                    raise ValueError(
                        f"app export {name!r} collides with an existing export"
                    )
                renamed.add(export.name)
        modified |= name != export.name
        identity = (export.kind, export.index)
        if (previous := seen.get(name)) is not None:
            if require_function_exports or previous != identity:
                raise ValueError(
                    f"WASM export rename collision for {name!r}: {previous!r} vs {identity!r}"
                )
            modified = True
            continue
        seen[name] = identity
        rewritten.append(WasmExport(name, export.kind, export.index))
    if require_function_exports and (missing := rename_map.keys() - renamed):
        raise ValueError(
            "the link kept no export for app symbol(s): " + ", ".join(sorted(missing))
        )
    if not modified:
        return None
    rebuilt = bytearray(_write_wasm_varuint(len(rewritten)))
    for export in rewritten:
        rebuilt.extend(_write_wasm_string(export.name))
        rebuilt.append(export.kind)
        rebuilt.extend(_write_wasm_varuint(export.index))
    return bytes(rebuilt)


def transform_wasm_publication_file(
    path: Path,
    *,
    rename_map: Mapping[str, str],
    final_artifact: bool,
    preserve_debug: bool,
    require_function_exports: bool = False,
    validate_imports: Callable[[Sequence[WasmImport]], None] | None = None,
    durability: PublicationDurability = PublicationDurability.DURABLE,
) -> WasmPublicationTransformMetrics:
    """Rewrite exports/custom sections with bounded memory and one output pass."""

    from molt.file_publication import atomic_replace, staged_file_path
    from molt.toolchain_identity import open_stable_regular_file

    staged = staged_file_path(path, purpose="wasm-transform")
    owned_stage = False
    try:
        with open_stable_regular_file(path, label="WASM publication input") as source:
            before = source.stat
            reader = _WasmFileBuffer(source.stream, before.st_size)
            actions: list[tuple[int, int, bytes | None, bool]] = []
            changed = False
            max_buffer = 0
            section_start = len(WASM_HEADER)
            has_export_section = False
            for span in parse_wasm_section_spans(reader, strict_custom_names=True):
                payload_end = span.offset + span.size
                replacement: bytes | None = None
                keep = True
                if span.id == 0:
                    name = span.custom_name
                    keep = not (
                        (
                            final_artifact
                            and is_wasm_final_artifact_forbidden_custom_section(name)
                        )
                        or (not preserve_debug and is_wasm_debug_custom_section(name))
                    )
                    changed |= not keep
                elif span.id == 2 and validate_imports is not None:
                    validate_imports(
                        parse_wasm_import_section(reader[span.offset : payload_end])
                    )
                    max_buffer = max(max_buffer, span.size)
                elif span.id == 7 and rename_map:
                    has_export_section = True
                    max_buffer = max(max_buffer, span.size)
                    replacement = _renamed_wasm_export_payload(
                        reader[span.offset : payload_end],
                        rename_map,
                        require_function_exports=require_function_exports,
                    )
                    if replacement is not None:
                        changed = True
                        max_buffer = max(max_buffer, len(replacement))
                actions.append((section_start, payload_end, replacement, keep))
                section_start = payload_end
            if rename_map and require_function_exports and not has_export_section:
                raise ValueError(
                    "the link kept no export for app symbol(s): "
                    + ", ".join(sorted(rename_map))
                )
            if not changed:
                return WasmPublicationTransformMetrics(
                    changed=False,
                    input_bytes=before.st_size,
                    output_bytes=before.st_size,
                    scanned_bytes=before.st_size,
                    written_bytes=0,
                    max_buffer_bytes=max_buffer,
                )

            output_size = len(WASM_HEADER)
            with (
                mmap.mmap(source.stream.fileno(), 0, access=mmap.ACCESS_READ) as mapped,
                staged.open("xb") as output,
            ):
                owned_stage = True
                output.write(WASM_HEADER)
                for section_start, section_end, replacement, keep in actions:
                    if not keep:
                        continue
                    if replacement is not None:
                        section_id = mapped[section_start]
                        header = bytes([section_id]) + _write_wasm_varuint(
                            len(replacement)
                        )
                        output.write(header)
                        output.write(replacement)
                        output_size += len(header) + len(replacement)
                        continue
                    for start in range(section_start, section_end, 8 * 1024 * 1024):
                        chunk_end = min(section_end, start + 8 * 1024 * 1024)
                        view = memoryview(mapped)[start:chunk_end]
                        try:
                            output.write(view)
                        finally:
                            view.release()
                        output_size += chunk_end - start
                output.flush()
                staged_stat = os.fstat(output.fileno())
            if staged_stat.st_size != output_size:
                raise ValueError("incomplete WASM publication transform")
            staged.chmod(before.st_mode & 0o777)
        atomic_replace(staged, path, durability=durability)
        return WasmPublicationTransformMetrics(
            changed=True,
            input_bytes=before.st_size,
            output_bytes=output_size,
            scanned_bytes=before.st_size,
            written_bytes=output_size,
            max_buffer_bytes=max_buffer,
        )
    finally:
        if owned_stage:
            staged.unlink(missing_ok=True)


def read_wasm_exports(
    path: Path,
    *,
    kind: int | None = None,
    on_error: Literal["raise", "ignore"] = "raise",
) -> list[WasmExport]:
    try:
        return parse_wasm_exports(path.read_bytes(), kind=kind, on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return []
        raise


def read_wasm_function_exports(
    path: Path, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmExport]:
    return read_wasm_exports(path, kind=0, on_error=on_error)


def _read_wasm_function_export_map(
    sections: Sequence[tuple[int, bytes]],
) -> dict[str, int]:
    return {
        export.name: export.index
        for export in _iter_wasm_exports(sections)
        if export.kind == 0
    }


class WasmExportSignature(TypedDict):
    params: list[str]
    result: str


def _wasm_export_function_signatures(
    path: Path,
    *,
    export_name_prefix: str | None = None,
    export_names: Iterable[str] | None = None,
) -> dict[str, WasmExportSignature]:
    return _wasm_export_function_signatures_from_sections(
        _parse_wasm_file_sections(path),
        export_name_prefix=export_name_prefix,
        export_names=export_names,
    )


def _wasm_export_function_signatures_from_sections(
    sections: Sequence[tuple[int, bytes]],
    *,
    export_name_prefix: str | None = None,
    export_names: Iterable[str] | None = None,
) -> dict[str, WasmExportSignature]:
    if (export_name_prefix is None) == (export_names is None):
        raise ValueError(
            "exactly one of export_name_prefix or export_names must be provided"
        )
    export_name_set = set(export_names or ())
    type_signatures = _read_wasm_type_signatures(sections)
    imports, _ = _read_wasm_import_function_type_indices(sections)
    function_type_indices = _read_wasm_function_type_indices(
        sections, [type_index for _, _, type_index in imports]
    )
    exports = _read_wasm_function_export_map(sections)

    export_signatures: dict[str, WasmExportSignature] = {}
    for export_name, function_index in exports.items():
        if export_name_prefix is not None and not export_name.startswith(
            export_name_prefix
        ):
            continue
        if export_names is not None and export_name not in export_name_set:
            continue
        type_index = function_type_indices.get(function_index)
        if type_index is None:
            raise ValueError(
                f"Missing wasm function type index for export {export_name}"
            )
        signature = type_signatures.get(type_index)
        if signature is None:
            raise ValueError(
                f"Missing wasm type index {type_index} for export {export_name}"
            )
        params, result_kind = signature
        export_signatures[export_name] = {
            "params": list(params),
            "result": result_kind,
        }
    return export_signatures


@dataclass(frozen=True, slots=True)
class WasmRuntimeFacts:
    """Compact immutable runtime semantics; no file bytes or section buffers survive."""

    exports: tuple[WasmExport, ...]
    globals: tuple[WasmGlobal, ...]
    function_signatures: tuple[tuple[str, tuple[str, ...], str], ...]
    data_end: int | None
    memory_min_bytes: int | None
    table_min: int | None
    shared_import_abi: bool
    code_functions: int


def parse_wasm_runtime_facts(data: bytes, *, relocatable: bool) -> WasmRuntimeFacts:
    """Derive compact runtime layout facts from the caller's admitted bytes."""
    spans = parse_wasm_section_spans(data)
    # Never copy code/debug/linking sections into a semantic snapshot. A reloc
    # layout needs no type/export/global parser; those sections may use valid
    # extended-const features irrelevant to layout and export admission.
    needed = {2, 5, 11} if relocatable else {1, 2, 3, 5, 6, 7, 11}
    sections = _wasm_sections_from_spans(
        data, [span for span in spans if span.id in needed]
    )
    imports = tuple(_iter_wasm_imports(sections))
    import_memory, import_table = _wasm_import_minima_from_imports(imports)
    signatures = (
        {}
        if relocatable
        else _wasm_export_function_signatures_from_sections(
            sections, export_name_prefix=""
        )
    )
    code_functions = 0
    for span in spans:
        if span.id == 10:
            # The count is a bounded u32 prefix; function bodies are not layout.
            code_functions, _ = _read_wasm_varuint(
                data[span.offset : span.offset + min(span.size, 5)], 0
            )
    return WasmRuntimeFacts(
        tuple(_iter_wasm_exports(sections)),
        tuple(_iter_wasm_defined_globals(sections)),
        tuple(
            (name, tuple(value["params"]), value["result"])
            for name, value in signatures.items()
        ),
        _wasm_data_end_from_sections(sections),
        _wasm_memory_min_bytes_from_sections(sections),
        _wasm_table_min_from_sections(sections),
        import_memory is not None and import_table is not None,
        code_functions,
    )
