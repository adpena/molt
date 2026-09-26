"""Canonical content identity for a source-extension object closure."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Collection, Mapping, MutableMapping, Sequence
from pathlib import Path
from typing import Any, cast

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES,
    WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES,
    wasm_import_signature,
)
from molt.cli.source_extension_manifest_codec import (
    _manifest_dependencies,
    _manifest_sequence,
)
from molt.cli.compiler_target import compiler_target_triple, validate_compiler_target
from molt.cli.source_extension_language import (
    require_source_extension_language,
    validate_source_extension_language_command,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
    SOURCE_EXTENSION_OBJECT_CLOSURE_FIELDS,
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_OBJECT_FIELDS,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.file_hashing import _sha256_file
from molt.c_api_symbols import is_cpython_abi_dynamic_import_symbol
from molt.wasm_artifact import (
    WASM_EXTERN_KIND_FUNCTION,
    WASM_EXTERN_KIND_GLOBAL,
    WASM_EXTERN_KIND_MEMORY,
    WASM_EXTERN_KIND_TABLE,
    WASM_EXTERN_KIND_TAG,
    WasmImport,
)


_WASM_IMPORT_KIND_NAMES = {
    WASM_EXTERN_KIND_FUNCTION: "function",
    WASM_EXTERN_KIND_TABLE: "table",
    WASM_EXTERN_KIND_MEMORY: "memory",
    WASM_EXTERN_KIND_GLOBAL: "global",
    WASM_EXTERN_KIND_TAG: "tag",
}
_WASM_IMPORT_KINDS = frozenset(_WASM_IMPORT_KIND_NAMES.values())
_LOWER_HEX = frozenset("0123456789abcdef")


class SourceExtensionObjectClosureError(ValueError):
    """An object closure is incomplete, non-canonical, or differs from its bytes."""


def _require_canonical_sha256(value: object, *, field: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in _LOWER_HEX for character in value)
    ):
        raise SourceExtensionObjectClosureError(
            f"extension object_closure {field} must be canonical lowercase SHA-256"
        )
    return value


def _producer_unit_identity(value: object) -> dict[str, str]:
    if not isinstance(value, Mapping) or set(value) != {"target_id", "object"}:
        raise SourceExtensionObjectClosureError(
            "extension object producer_unit requires exactly target_id and object"
        )
    target_id, output = value.get("target_id"), value.get("object")
    if (
        not isinstance(target_id, str)
        or not target_id.strip()
        or target_id != target_id.strip()
        or any(ord(char) < 32 for char in target_id)
        or not isinstance(output, str)
        or not output
        or "\\" in output
        or ":" in output
        or any(ord(char) < 32 for char in output)
        or any(part in {"", ".", ".."} for part in output.split("/"))
    ):
        raise SourceExtensionObjectClosureError(
            "extension object producer_unit requires a target ID and canonical "
            "build-root-relative object path"
        )
    return {"target_id": target_id, "object": output}


def source_extension_wasm_import_receipts(
    wasm_imports: Sequence[WasmImport],
) -> tuple[dict[str, str], ...]:
    receipts: list[dict[str, str]] = []
    for wasm_import in wasm_imports:
        kind = _WASM_IMPORT_KIND_NAMES.get(wasm_import.kind)
        if kind is None:
            raise SourceExtensionObjectClosureError(
                f"extension artifact has unsupported WASM import kind "
                f"{wasm_import.kind!r}"
            )
        if not wasm_import.module or not wasm_import.name:
            raise SourceExtensionObjectClosureError(
                "extension artifact has an empty WASM import module or name"
            )
        receipts.append(
            {
                "module": wasm_import.module,
                "name": wasm_import.name,
                "kind": kind,
            }
        )
    receipts.sort(key=lambda item: (item["module"], item["name"], item["kind"]))
    identities = {(item["module"], item["name"]) for item in receipts}
    if len(identities) != len(receipts):
        raise SourceExtensionObjectClosureError(
            "extension artifact has duplicate WASM import records"
        )
    return tuple(receipts)


def manifest_source_extension_wasm_import_receipts(
    object_closure: Mapping[str, Any],
    *,
    required: bool,
) -> tuple[dict[str, str], ...]:
    raw = object_closure.get("wasm_imports")
    if raw is None:
        if required:
            raise SourceExtensionObjectClosureError(
                "extension object_closure.wasm_imports is required"
            )
        return ()
    if not isinstance(raw, list):
        raise SourceExtensionObjectClosureError(
            "extension object_closure.wasm_imports must be an array"
        )
    receipts: list[dict[str, str]] = []
    for index, item in enumerate(raw):
        if not isinstance(item, Mapping) or set(item) != {"module", "name", "kind"}:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.wasm_imports[{index}] is invalid"
            )
        module = item.get("module")
        name = item.get("name")
        kind = item.get("kind")
        if (
            not isinstance(module, str)
            or not module
            or not isinstance(name, str)
            or not name
            or kind not in _WASM_IMPORT_KINDS
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.wasm_imports[{index}] is invalid"
            )
        receipts.append({"module": module, "name": name, "kind": str(kind)})
    canonical = tuple(
        sorted(receipts, key=lambda item: (item["module"], item["name"], item["kind"]))
    )
    if tuple(receipts) != canonical or len(
        {(item["module"], item["name"]) for item in receipts}
    ) != len(receipts):
        raise SourceExtensionObjectClosureError(
            "extension object_closure.wasm_imports is not canonical"
        )
    return canonical


def validate_source_extension_wasm_import_shapes(
    receipts: Sequence[Mapping[str, str]],
    *,
    provider_function_names: Collection[str] = (),
    provider_function_signatures: Mapping[str, tuple[tuple[str, ...], str]]
    | None = None,
    actual_function_signatures: Mapping[tuple[str, str], tuple[tuple[str, ...], str]],
) -> list[str]:
    errors: list[str] = []
    provider_function_signatures = provider_function_signatures or {}
    for receipt in receipts:
        name = receipt["name"]
        expected = WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES.get(name)
        generated_shape = expected is not None
        if expected is None:
            if (
                name not in provider_function_names
                and name not in provider_function_signatures
            ):
                errors.append(
                    f"WASM import {name!r} has no generated ABI or exact-link-provider "
                    "authority"
                )
                continue
            expected = ("env", "function")
        actual = (receipt["module"], receipt["kind"])
        if actual != expected:
            errors.append(
                f"WASM import {name!r} has module/kind {actual!r}; "
                f"expected {expected!r}"
            )
            continue
        if receipt["kind"] != "function":
            continue
        actual_signature = actual_function_signatures.get((receipt["module"], name))
        provider_signature = provider_function_signatures.get(name)
        if provider_signature is not None and actual_signature != provider_signature:
            errors.append(
                f"WASM function import {receipt['module']!r}.{name!r} has signature "
                f"{actual_signature!r}; package provider has {provider_signature!r}"
            )
        external_signature = WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES.get(
            (receipt["module"], name)
        )
        if external_signature is not None:
            params = external_signature["params"]
            result = external_signature["result"]
            if (
                not isinstance(params, (tuple, list))
                or not all(isinstance(value, str) for value in params)
                or not isinstance(result, str)
            ):
                errors.append(
                    f"WASM import {name!r} has malformed generated ABI signature"
                )
                continue
            expected_signature = (
                tuple(cast(Sequence[str], params)),
                result,
            )
        else:
            runtime_signature = wasm_import_signature(name)
            expected_signature = (
                None
                if runtime_signature is None
                else (
                    runtime_signature[0],
                    "nil"
                    if not runtime_signature[1]
                    else ", ".join(runtime_signature[1]),
                )
            )
        if expected_signature is None:
            if generated_shape:
                errors.append(
                    f"WASM function import {name!r} has no generated signature authority"
                )
            continue
        if actual_signature != expected_signature:
            errors.append(
                f"WASM function import {receipt['module']!r}.{name!r} has signature "
                f"{actual_signature!r}; expected {expected_signature!r}"
            )
    return errors


def _canonical_string_array(
    value: object,
    *,
    field: str,
) -> list[str]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item for item in value
    ):
        raise SourceExtensionObjectClosureError(
            f"extension object_closure.{field} must be a string array"
        )
    if value != sorted(set(value)):
        raise SourceExtensionObjectClosureError(
            f"extension object_closure.{field} is not canonical"
        )
    return list(cast(list[str], value))


def _object_sequence(
    manifest: Mapping[str, Any],
    item: Mapping[str, Any],
    field: str,
    *,
    required: bool = False,
    canonical: bool = False,
) -> list[str]:
    try:
        values = _manifest_sequence(manifest, item, field)
    except ValueError as exc:
        raise SourceExtensionObjectClosureError(str(exc)) from exc
    if values is None:
        if required:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure object is missing {field} custody"
            )
        return []
    if canonical and values != sorted(set(values)):
        raise SourceExtensionObjectClosureError(
            f"extension object_closure object {field} is not canonical"
        )
    return values


def source_extension_object_closure_identity_payload(
    object_closure: Mapping[str, Any],
    *,
    manifest: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    unknown_closure_fields = sorted(
        set(object_closure) - SOURCE_EXTENSION_OBJECT_CLOSURE_FIELDS
    )
    if unknown_closure_fields:
        raise SourceExtensionObjectClosureError(
            "extension object_closure has unknown field(s): "
            + ", ".join(unknown_closure_fields)
        )
    if "closure_sha256" in object_closure:
        _require_canonical_sha256(
            object_closure.get("closure_sha256"),
            field="closure_sha256",
        )
    if (
        object_closure.get("schema_version")
        != SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION
    ):
        raise SourceExtensionObjectClosureError(
            "extension object_closure.schema_version must be "
            f"{SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION}"
        )
    objects = object_closure.get("objects")
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.objects is empty"
        )
    root_symbol = object_closure.get("root_symbol")
    init_symbol_owner = object_closure.get("init_symbol_owner")
    if not isinstance(root_symbol, str) or not root_symbol:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.root_symbol must be non-empty"
        )
    if not isinstance(init_symbol_owner, str) or not init_symbol_owner:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.init_symbol_owner must be non-empty"
        )
    if manifest is not None and manifest.get("init_symbol") != root_symbol:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.root_symbol differs from manifest init_symbol"
        )
    closure_defined_symbols = _canonical_string_array(
        object_closure.get("defined_symbols"), field="defined_symbols"
    )
    closure_undefined_symbols = _canonical_string_array(
        object_closure.get("undefined_symbols"), field="undefined_symbols"
    )
    runtime_symbols = _canonical_string_array(
        object_closure.get("runtime_symbols"), field="runtime_symbols"
    )
    required_c_api_symbols = _canonical_string_array(
        object_closure.get("required_c_api_symbols", []),
        field="required_c_api_symbols",
    )
    required_capsules = _canonical_string_array(
        object_closure.get("required_capsules", []), field="required_capsules"
    )
    project_generated_c_api_symbols = _canonical_string_array(
        object_closure.get("project_generated_c_api_symbols", []),
        field="project_generated_c_api_symbols",
    )
    project_generated_c_api_prefixes = _canonical_string_array(
        object_closure.get("project_generated_c_api_prefixes", []),
        field="project_generated_c_api_prefixes",
    )
    authority = manifest if manifest is not None else {"object_closure": object_closure}
    digest_objects: list[dict[str, Any]] = []
    symbol_authorities: set[str] = set()
    object_names: set[str] = set()
    producer_outputs: set[str] = set()
    source_digests: dict[str, str] = {}
    root_owners: list[str] = []
    object_defined_union: set[str] = set()
    object_undefined_union: set[str] = set()
    object_required_c_api_union: set[str] = set()
    object_required_capsules_union: set[str] = set()
    object_project_generated_c_api_union: set[str] = set()
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] is not an object"
            )
        item = cast(Mapping[str, Any], item)
        unknown_object_fields = sorted(set(item) - SOURCE_EXTENSION_OBJECT_FIELDS)
        if unknown_object_fields:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] has unknown field(s): "
                + ", ".join(unknown_object_fields)
            )
        has_compact_authority = manifest is not None and "build_authorities" in manifest
        if "compile_command_operands" in item and "compile_command_ref" not in item:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] compile operands "
                "require compile_command_ref"
            )
        if "unit_sha256" in item and not has_compact_authority:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] unit_sha256 is valid "
                "only in compact authority form"
            )
        source = item.get("source")
        try:
            language = require_source_extension_language(item.get("language"))
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}]: {exc}"
            ) from exc
        object_path = item.get("object")
        source_sha256 = _require_canonical_sha256(
            item.get("source_sha256"),
            field=f"objects[{index}].source_sha256",
        )
        object_sha256 = _require_canonical_sha256(
            item.get("object_sha256"),
            field=f"objects[{index}].object_sha256",
        )
        if not (
            isinstance(source, str)
            and source
            and isinstance(object_path, str)
            and object_path
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] lacks checksum custody"
            )
        if object_path in object_names:
            raise SourceExtensionObjectClosureError(
                "extension object_closure has duplicate object custody"
            )
        object_names.add(object_path)
        # A source blob may back several compile units (different language,
        # flags, or logical originals with identical content). Object custody,
        # not the retained source path, identifies the unit.
        if source in source_digests and source_digests[source] != source_sha256:
            raise SourceExtensionObjectClosureError(
                "extension object_closure has conflicting source checksum custody"
            )
        source_digests[source] = source_sha256
        compile_command = _object_sequence(
            authority, item, "compile_command", required=True
        )
        if manifest is not None and isinstance(manifest.get("target_triple"), str):
            try:
                validate_compiler_target(
                    compile_command,
                    compiler_target_triple(compile_command, manifest["target_triple"]),
                )
            except ValueError as exc:
                raise SourceExtensionObjectClosureError(
                    f"extension object_closure.objects[{index}] target custody: {exc}"
                ) from exc
        try:
            validate_source_extension_language_command(language, compile_command)
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(str(exc)) from exc
        object_defined_symbols = _object_sequence(
            authority, item, "defined_symbols", canonical=True
        )
        object_undefined_symbols = _object_sequence(
            authority, item, "undefined_symbols", canonical=True
        )
        dynamic_python_imports = [
            symbol
            for symbol in object_undefined_symbols
            if is_cpython_abi_dynamic_import_symbol(symbol)
        ]
        if dynamic_python_imports:
            raise SourceExtensionObjectClosureError(
                "static extension retains dynamic CPython import-address requirements: "
                + ", ".join(dynamic_python_imports)
            )
        object_defined_union.update(object_defined_symbols)
        object_undefined_union.update(object_undefined_symbols)
        if root_symbol in object_defined_symbols:
            root_owners.append(object_path)
        symbol_authority = item.get("symbol_authority")
        if symbol_authority == SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY:
            symbol_command = _object_sequence(
                authority, item, "symbol_command", required=True
            )
        elif symbol_authority == SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY:
            symbol_command = _object_sequence(authority, item, "symbol_command")
            if (
                symbol_command
                or "symbol_command" in item
                or "symbol_command_ref" in item
            ):
                raise SourceExtensionObjectClosureError(
                    "WASM object closure facts must not claim an external symbol command"
                )
        else:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] has invalid "
                "symbol_authority"
            )
        symbol_authorities.add(symbol_authority)
        digest_object: dict[str, Any] = {
            "source": source,
            "language": language.value,
            "object": object_path,
            "source_sha256": source_sha256,
            "object_sha256": object_sha256,
            "defined_symbols": object_defined_symbols,
            "undefined_symbols": object_undefined_symbols,
            "compile_command": compile_command,
            "symbol_authority": symbol_authority,
        }
        if "producer_unit" in item:
            producer_unit = _producer_unit_identity(item["producer_unit"])
            if producer_unit["object"] in producer_outputs:
                raise SourceExtensionObjectClosureError(
                    "extension object_closure has duplicate producer object custody"
                )
            producer_outputs.add(producer_unit["object"])
            digest_object["producer_unit"] = producer_unit
        elif manifest is not None and isinstance(manifest.get("source_plan"), Mapping):
            raise SourceExtensionObjectClosureError(
                "source-plan object is missing producer_unit custody"
            )
        if symbol_command:
            digest_object["symbol_command"] = symbol_command
        try:
            raw_dependencies = _manifest_dependencies(authority, item)
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(str(exc)) from exc
        dependencies: list[dict[str, str]] = []
        for dependency_index, raw_dependency in enumerate(raw_dependencies):
            if not isinstance(raw_dependency, Mapping):
                raise SourceExtensionObjectClosureError(
                    "extension object_closure dependency is not an object"
                )
            dependency_path_raw = raw_dependency.get("path")
            dependency_sha256 = _require_canonical_sha256(
                raw_dependency.get("sha256"),
                field=(f"objects[{index}].dependencies[{dependency_index}].sha256"),
            )
            if not (isinstance(dependency_path_raw, str) and dependency_path_raw):
                raise SourceExtensionObjectClosureError(
                    "extension object_closure dependency lacks path/checksum "
                    f"at objects[{index}].dependencies[{dependency_index}]"
                )
            dependencies.append(
                {"path": dependency_path_raw, "sha256": dependency_sha256}
            )
        if dependencies != sorted(
            dependencies,
            key=lambda dependency: (
                dependency["sha256"],
                Path(dependency["path"]).name,
                dependency["path"],
            ),
        ) or len({dependency["path"] for dependency in dependencies}) != len(
            dependencies
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] dependencies are not canonical"
            )
        digest_object["dependencies"] = dependencies
        for field in (
            "required_c_api_symbols",
            "required_capsules",
            "project_generated_c_api_symbols",
        ):
            values = _object_sequence(authority, item, field, canonical=True)
            digest_object[field] = values
            if field == "required_c_api_symbols":
                object_required_c_api_union.update(values)
            elif field == "required_capsules":
                object_required_capsules_union.update(values)
            else:
                object_project_generated_c_api_union.update(values)
        digest_objects.append(digest_object)
    aggregate_object_fields = (
        (
            "required_c_api_symbols",
            set(required_c_api_symbols),
            object_required_c_api_union,
        ),
        (
            "required_capsules",
            set(required_capsules),
            object_required_capsules_union,
        ),
        (
            "project_generated_c_api_symbols",
            set(project_generated_c_api_symbols),
            object_project_generated_c_api_union,
        ),
    )
    for field, declared, derived in aggregate_object_fields:
        if declared != derived:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.{field} differs from the exact object union; "
                f"missing={sorted(derived - declared)!r}; "
                f"stale={sorted(declared - derived)!r}"
            )
    if root_symbol not in closure_defined_symbols:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.root_symbol is absent from defined_symbols"
        )
    missing_object_definitions = sorted(
        object_defined_union - set(closure_defined_symbols)
    )
    if missing_object_definitions:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.defined_symbols omits object definitions: "
            + ", ".join(missing_object_definitions)
        )
    expected_undefined_symbols = object_undefined_union - object_defined_union
    unaccounted_object_undefined = sorted(
        expected_undefined_symbols
        - set(closure_defined_symbols)
        - set(closure_undefined_symbols)
    )
    if unaccounted_object_undefined:
        raise SourceExtensionObjectClosureError(
            "extension object_closure omits unresolved object symbols: "
            + ", ".join(unaccounted_object_undefined)
        )
    overlapping_symbols = sorted(
        set(closure_defined_symbols) & set(closure_undefined_symbols)
    )
    if overlapping_symbols:
        raise SourceExtensionObjectClosureError(
            "extension object_closure defines and leaves unresolved the same symbols: "
            + ", ".join(overlapping_symbols)
        )
    non_undefined_runtime = sorted(
        set(runtime_symbols) - set(closure_undefined_symbols)
    )
    if non_undefined_runtime:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.runtime_symbols are not linker undefineds: "
            + ", ".join(non_undefined_runtime)
        )
    if root_owners != [init_symbol_owner]:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.init_symbol_owner does not uniquely own "
            "root_symbol"
        )
    identity_payload = {
        "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
        "root_symbol": root_symbol,
        "init_symbol_owner": init_symbol_owner,
        "objects": digest_objects,
        "defined_symbols": closure_defined_symbols,
        "undefined_symbols": closure_undefined_symbols,
        "runtime_symbols": runtime_symbols,
        "required_c_api_symbols": required_c_api_symbols,
        "required_capsules": required_capsules,
        "project_generated_c_api_symbols": project_generated_c_api_symbols,
        "project_generated_c_api_prefixes": project_generated_c_api_prefixes,
    }
    if len(symbol_authorities) != 1:
        raise SourceExtensionObjectClosureError(
            "extension object_closure mixes native and WASM symbol authorities"
        )
    wasm_symbol_authority = symbol_authorities == {
        SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY
    }
    if not wasm_symbol_authority:
        native_aggregate_fields = (
            (
                "defined_symbols",
                set(closure_defined_symbols),
                object_defined_union,
            ),
            (
                "undefined_symbols",
                set(closure_undefined_symbols),
                object_undefined_union - object_defined_union,
            ),
        )
        for field, declared, derived in native_aggregate_fields:
            if declared != derived:
                raise SourceExtensionObjectClosureError(
                    f"native extension object_closure.{field} differs from the "
                    "exact object projection; "
                    f"missing={sorted(derived - declared)!r}; "
                    f"stale={sorted(declared - derived)!r}"
                )
    artifact_kind = manifest.get("artifact_kind") if manifest is not None else None
    if artifact_kind == "wasm_relocatable_object" and not wasm_symbol_authority:
        raise SourceExtensionObjectClosureError(
            "WASM extension artifact has non-WASM object symbol authority"
        )
    if artifact_kind == "static_archive" and wasm_symbol_authority:
        raise SourceExtensionObjectClosureError(
            "native archive has WASM object symbol authority"
        )
    has_wasm_imports = "wasm_imports" in object_closure
    if wasm_symbol_authority and not has_wasm_imports:
        raise SourceExtensionObjectClosureError(
            "WASM extension object_closure.wasm_imports is required"
        )
    if not wasm_symbol_authority and has_wasm_imports:
        raise SourceExtensionObjectClosureError(
            "native archive object_closure must not contain wasm_imports"
        )
    if has_wasm_imports:
        identity_payload["wasm_imports"] = list(
            manifest_source_extension_wasm_import_receipts(
                object_closure,
                required=True,
            )
        )
    return identity_payload


def source_extension_object_closure_digest(
    object_closure: Mapping[str, Any],
    *,
    manifest: Mapping[str, Any] | None = None,
) -> str:
    identity = source_extension_object_closure_identity_payload(
        object_closure,
        manifest=manifest,
    )
    return _source_extension_object_closure_identity_digest(identity)


def _source_extension_object_closure_identity_digest(
    identity: Mapping[str, Any],
) -> str:
    encoded = json.dumps(
        identity,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def validate_source_extension_object_closure(
    manifest: Mapping[str, Any],
) -> tuple[dict[str, Any], str]:
    """Return the canonical projection and digest after validating custody."""

    if "build_authorities" in manifest:
        # Imported lazily because the compact codec consumes the primitive
        # sequence/dependency readers from this module's import surface.
        from molt.cli.source_extension_manifest_codec import (
            _validate_compact_source_extension_manifest,
        )

        try:
            _validate_compact_source_extension_manifest(manifest)
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(
                f"compact extension manifest authority is invalid: {exc}"
            ) from exc
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        raise SourceExtensionObjectClosureError(
            "extension manifest has no object_closure"
        )
    identity = source_extension_object_closure_identity_payload(
        object_closure,
        manifest=manifest,
    )
    digest = _source_extension_object_closure_identity_digest(identity)
    if object_closure.get("closure_sha256") != digest:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.closure_sha256 is false"
        )
    build = manifest.get("build")
    if not isinstance(build, Mapping) or build.get("object_closure_sha256") != digest:
        raise SourceExtensionObjectClosureError(
            "extension build.object_closure_sha256 differs from the canonical "
            "object closure identity"
        )
    return identity, digest


def validate_source_extension_object_closure_sources(
    object_closure: Mapping[str, Any],
    *,
    manifest_dir: Path,
    manifest: Mapping[str, Any] | None = None,
    allowed_root: Path | None = None,
) -> None:
    objects = object_closure.get("objects")
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.objects is empty"
        )
    authority = manifest if manifest is not None else {"object_closure": object_closure}
    resolved_allowed_root = (
        None if allowed_root is None else allowed_root.resolve(strict=True)
    )

    def resolve_custodied_path(raw_path: str, *, field: str) -> Path:
        path = Path(raw_path)
        if not path.is_absolute():
            path = manifest_dir / path
        resolved = path.resolve(strict=False)
        if resolved_allowed_root is not None and not resolved.is_relative_to(
            resolved_allowed_root
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure {field} escapes sealed payload root "
                f"{resolved_allowed_root}: {resolved}"
            )
        return resolved

    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] is not an object"
            )
        source = item.get("source")
        source_sha256 = item.get("source_sha256")
        if not isinstance(source, str) or not isinstance(source_sha256, str):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] lacks source custody"
            )
        source_path = resolve_custodied_path(
            source,
            field=f"objects[{index}].source",
        )
        if not source_path.is_file():
            raise SourceExtensionObjectClosureError(
                f"extension object_closure source is missing: {source_path}"
            )
        if _sha256_file(source_path) != source_sha256:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure source checksum mismatch: {source_path}"
            )
        try:
            dependencies = _manifest_dependencies(
                authority, cast(Mapping[str, Any], item)
            )
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(str(exc)) from exc
        for dependency_index, dependency in enumerate(dependencies):
            dependency_path = resolve_custodied_path(
                dependency["path"],
                field=(f"objects[{index}].dependencies[{dependency_index}].path"),
            )
            if not dependency_path.is_file():
                raise SourceExtensionObjectClosureError(
                    f"extension object_closure dependency is missing: {dependency_path}"
                )
            if _sha256_file(dependency_path) != dependency["sha256"]:
                raise SourceExtensionObjectClosureError(
                    "extension object_closure dependency checksum mismatch: "
                    f"{dependency_path}"
                )


def finalize_source_extension_object_closure(
    manifest: MutableMapping[str, Any],
) -> tuple[dict[str, Any], str]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, MutableMapping):
        raise SourceExtensionObjectClosureError(
            "extension manifest has no mutable object_closure"
        )
    identity = source_extension_object_closure_identity_payload(
        object_closure,
        manifest=manifest,
    )
    digest = _source_extension_object_closure_identity_digest(identity)
    object_closure["closure_sha256"] = digest
    build = manifest.get("build")
    if not isinstance(build, MutableMapping):
        raise SourceExtensionObjectClosureError(
            "extension manifest has no mutable build custody"
        )
    build["object_closure_sha256"] = digest
    return identity, digest
