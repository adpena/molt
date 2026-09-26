"""Compact, lossless authority codec for source-extension manifests."""

from __future__ import annotations

import hashlib
import json
from copy import deepcopy
from collections.abc import Mapping, Sequence
from typing import Any, cast

from molt.cli.compiler_target import compiler_argument_spans
from molt.cli.source_extension_link_projection import SourceExtensionLinkProjection
from molt.cli.source_extension_python_provider import (
    validate_static_python_provider_requirements,
)
from molt.cli.source_extension_link_requirements import (
    parse_source_extension_link_requirements,
)
from molt.cli.source_extension_language import (
    require_source_extension_language,
    validate_source_extension_language_command,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)

_BUILD_SEQUENCE_FIELDS = (
    "compiler",
    "extra_compile_args",
    "include_dirs",
    "linker",
)
_OBJECT_SEQUENCE_FIELDS = (
    "defined_symbols",
    "undefined_symbols",
    "required_c_api_symbols",
    "required_capsules",
    "project_generated_c_api_symbols",
)


def _canonical_sequence_digest(values: Sequence[str]) -> str:
    return hashlib.sha256(
        json.dumps(list(values), separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def _intern_sequence(pool: dict[str, list[str]], values: Sequence[str]) -> str:
    if not values or not all(isinstance(value, str) and value for value in values):
        raise ValueError("source-extension sequence must contain non-empty strings")
    digest = _canonical_sequence_digest(values)
    prior = pool.setdefault(digest, list(values))
    if prior != list(values):
        raise ValueError(f"source-extension sequence digest collision: {digest}")
    return digest


def _manifest_sequence(
    manifest: Mapping[str, Any], owner: Mapping[str, Any], field: str
) -> list[str] | None:
    inline = owner.get(field)
    reference = owner.get(f"{field}_ref")
    has_inline = field in owner
    has_reference = f"{field}_ref" in owner
    if has_inline and has_reference:
        raise ValueError(f"{field} has both inline and referenced authority")
    if has_inline:
        if not isinstance(inline, list):
            raise ValueError(f"{field} inline authority is invalid")
        values = [value for value in inline if isinstance(value, str) and value]
        if len(values) != len(inline):
            raise ValueError(f"{field} inline authority is invalid")
        return values
    if not has_reference:
        return None
    if not isinstance(reference, str):
        raise ValueError(f"{field} references an invalid sequence authority")
    authorities = manifest.get("build_authorities")
    sequences = (
        authorities.get("sequences") if isinstance(authorities, Mapping) else None
    )
    strings = authorities.get("strings") if isinstance(authorities, Mapping) else None
    encoded = sequences.get(reference) if isinstance(sequences, Mapping) else None
    if not isinstance(strings, list) or not isinstance(encoded, list):
        raise ValueError(f"{field} references an invalid sequence authority")
    string_values = [value for value in strings if isinstance(value, str) and value]
    indexes = [
        index
        for index in encoded
        if isinstance(index, int)
        and not isinstance(index, bool)
        and 0 <= index < len(string_values)
    ]
    if len(string_values) != len(strings) or len(indexes) != len(encoded):
        raise ValueError(f"{field} references an invalid sequence authority")
    result = [string_values[index] for index in indexes]
    if _canonical_sequence_digest(result) != reference:
        raise ValueError(f"{field} sequence authority digest is false")
    if field == "compile_command":
        operands = owner.get("compile_command_operands")
        if "compile_command_operands" in owner:
            if not isinstance(operands, list):
                raise ValueError("compile_command_operands is invalid")
            replacements: list[tuple[int, str]] = []
            for item in operands:
                if not isinstance(item, Mapping) or set(item) != {"index", "value"}:
                    raise ValueError("compile_command_operands is invalid")
                index = item.get("index")
                value = item.get("value")
                if (
                    not isinstance(index, int)
                    or isinstance(index, bool)
                    or not isinstance(value, str)
                    or not value
                    or not 0 <= index < len(result)
                ):
                    raise ValueError("compile_command_operands is invalid")
                if result[index] != "%{operand}":
                    raise ValueError(
                        "compile_command_operands does not reference an operand slot"
                    )
                replacements.append((index, value))
            operand_indexes = [index for index, _value in replacements]
            if operand_indexes != sorted(set(operand_indexes)):
                raise ValueError("compile_command_operands indexes are not canonical")
            placeholder_indexes = [
                index for index, value in enumerate(result) if value == "%{operand}"
            ]
            if operand_indexes != placeholder_indexes:
                raise ValueError(
                    "compile_command_operands does not exactly cover operand slots"
                )
            for index, value in replacements:
                result[index] = value
    return result


def _manifest_dependencies(
    manifest: Mapping[str, Any], owner: Mapping[str, Any]
) -> list[dict[str, str]]:
    inline = owner.get("dependencies")
    if "dependencies" in owner and "dependencies_ref" in owner:
        raise ValueError("dependencies has both inline and referenced authority")
    if "dependencies" in owner:
        if not isinstance(inline, list):
            raise ValueError("inline dependencies authority is invalid")
        dependencies: list[dict[str, str]] = []
        for item in inline:
            if not isinstance(item, Mapping) or set(item) != {"path", "sha256"}:
                raise ValueError("inline dependencies authority is invalid")
            path = item.get("path")
            sha256 = item.get("sha256")
            if (
                not isinstance(path, str)
                or not path
                or not isinstance(sha256, str)
                or not sha256
            ):
                raise ValueError("inline dependencies authority is invalid")
            dependencies.append({"path": path, "sha256": sha256})
        return dependencies
    flattened = _manifest_sequence(manifest, owner, "dependencies")
    if flattened is None or len(flattened) % 2:
        raise ValueError("referenced dependencies authority is invalid")
    return [
        {"path": flattened[index], "sha256": flattened[index + 1]}
        for index in range(0, len(flattened), 2)
    ]


def _compile_command_template(
    values: Sequence[str],
) -> tuple[list[str], list[dict[str, Any]]]:
    template = list(values)
    operands: list[dict[str, Any]] = []
    for span in compiler_argument_spans(values):
        if span.context != "driver":
            continue
        output = span.output_option
        if span.option in {"-c", "/c"} or (output is not None and len(span.raw) == 2):
            operand_index = span.index + 1
            if operand_index >= len(template):
                raise ValueError("compile command has a missing source/output operand")
        elif output is not None:
            operand_index = span.index
        else:
            continue
        operands.append({"index": operand_index, "value": template[operand_index]})
        template[operand_index] = "%{operand}"
    operand_indexes = [item["index"] for item in operands]
    if len(operand_indexes) != len(set(operand_indexes)):
        raise ValueError("compile command has overlapping operand encodings")
    return template, sorted(operands, key=lambda item: item["index"])


def _object_unit_identity(
    manifest: Mapping[str, Any], item: Mapping[str, Any]
) -> dict[str, Any]:
    excluded = {
        "unit_sha256",
        "compile_command_operands",
        "dependencies",
        "dependencies_ref",
        "compile_command",
        "symbol_command",
        "compile_command_ref",
        "symbol_command_ref",
    }
    excluded.update(_OBJECT_SEQUENCE_FIELDS)
    excluded.update(f"{field}_ref" for field in _OBJECT_SEQUENCE_FIELDS)
    payload = {key: item.get(key) for key in sorted(item) if key not in excluded}
    compile_command = _manifest_sequence(manifest, item, "compile_command")
    if compile_command is None:
        raise ValueError("source-extension object is missing compile_command authority")
    validate_source_extension_language_command(
        require_source_extension_language(item.get("language")), compile_command
    )
    payload["compile_command"] = compile_command
    symbol_command = _manifest_sequence(manifest, item, "symbol_command")
    if symbol_command is not None:
        payload["symbol_command"] = symbol_command
    payload["dependencies"] = _manifest_dependencies(manifest, item)
    for field in _OBJECT_SEQUENCE_FIELDS:
        if field in item or f"{field}_ref" in item:
            payload[field] = _manifest_sequence(manifest, item, field)
    return payload


def _object_unit_sha256(manifest: Mapping[str, Any], item: Mapping[str, Any]) -> str:
    return hashlib.sha256(
        json.dumps(
            _object_unit_identity(manifest, item),
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def _expand_source_extension_manifest_authorities(
    manifest: Mapping[str, Any],
) -> dict[str, Any]:
    """Materialize a compact manifest before an owned semantic mutation."""

    expanded = deepcopy(dict(manifest))
    if "build_authorities" not in expanded:
        return expanded
    _validate_compact_source_extension_manifest(expanded)
    build = expanded.get("build")
    if isinstance(build, dict):
        for field in _BUILD_SEQUENCE_FIELDS:
            values = _manifest_sequence(expanded, build, field)
            build.pop(f"{field}_ref", None)
            if values is not None:
                build[field] = values
    closure = expanded.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    assert isinstance(objects, list)
    for raw_item in objects:
        assert isinstance(raw_item, dict)
        item = cast(dict[str, Any], raw_item)
        for field in ("compile_command", "symbol_command"):
            values = _manifest_sequence(expanded, item, field)
            item.pop(f"{field}_ref", None)
            if values is not None:
                item[field] = values
        item.pop("compile_command_operands", None)
        dependencies = _manifest_dependencies(expanded, item)
        item.pop("dependencies_ref", None)
        item["dependencies"] = dependencies
        for field in _OBJECT_SEQUENCE_FIELDS:
            values = _manifest_sequence(expanded, item, field)
            item.pop(f"{field}_ref", None)
            if values is not None:
                item[field] = values
        item.pop("unit_sha256", None)
    expanded.pop("build_authorities")
    return expanded


def _compact_source_extension_manifest(manifest: dict[str, Any]) -> dict[str, Any]:
    """Compact an owned manifest in place while preserving exact argv."""

    if "build_authorities" in manifest:
        raise ValueError("source-extension manifest must be expanded before compaction")
    pool: dict[str, list[str]] = {}
    build = manifest.get("build")
    if isinstance(build, dict):
        build = cast(dict[str, Any], build)
        stale_build_refs = sorted(key for key in build if key.endswith("_ref"))
        if stale_build_refs:
            raise ValueError(
                "expanded source-extension build retains compact reference(s): "
                + ", ".join(stale_build_refs)
            )
        for field in _BUILD_SEQUENCE_FIELDS:
            if field not in build:
                continue
            raw = build.pop(field)
            if not isinstance(raw, list) or not all(
                isinstance(value, str) and value for value in raw
            ):
                raise ValueError(f"build.{field} must be a string array")
            values = [value for value in raw if isinstance(value, str)]
            if values:
                build[f"{field}_ref"] = _intern_sequence(pool, values)
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise ValueError("source-extension object closure is empty")
    for index, item in enumerate(objects):
        if not isinstance(item, dict):
            raise ValueError(f"object_closure.objects[{index}] must be an object")
        item = cast(dict[str, Any], item)
        require_source_extension_language(item.get("language"))
        stale_compact_fields = sorted(
            key
            for key in item
            if key.endswith("_ref")
            or key in {"compile_command_operands", "unit_sha256"}
        )
        if stale_compact_fields:
            raise ValueError(
                f"expanded object_closure.objects[{index}] retains compact field(s): "
                + ", ".join(stale_compact_fields)
            )
        for field in ("compile_command", "symbol_command"):
            if field not in item and field == "symbol_command":
                continue
            raw = item.pop(field, None)
            if not isinstance(raw, list) or not raw:
                raise ValueError(f"object_closure.objects[{index}].{field} is invalid")
            values = [value for value in raw if isinstance(value, str) and value]
            if len(values) != len(raw):
                raise ValueError(f"object_closure.objects[{index}].{field} is invalid")
            if field == "compile_command":
                values, operands = _compile_command_template(values)
                if operands:
                    item["compile_command_operands"] = operands
            item[f"{field}_ref"] = _intern_sequence(pool, values)
        raw_dependencies = item.pop("dependencies", None)
        try:
            dependencies = _manifest_dependencies(
                {"object_closure": {}}, {"dependencies": raw_dependencies}
            )
        except ValueError as exc:
            raise ValueError(
                f"object_closure.objects[{index}].dependencies is invalid"
            ) from exc
        flattened = [
            value
            for dependency in dependencies
            for value in (dependency["path"], dependency["sha256"])
        ]
        if flattened:
            item["dependencies_ref"] = _intern_sequence(pool, flattened)
        else:
            item["dependencies"] = []
        for field in _OBJECT_SEQUENCE_FIELDS:
            if field not in item:
                continue
            raw = item.pop(field)
            if not isinstance(raw, list):
                raise ValueError(f"object_closure.objects[{index}].{field} is invalid")
            values = [value for value in raw if isinstance(value, str) and value]
            if len(values) != len(raw):
                raise ValueError(f"object_closure.objects[{index}].{field} is invalid")
            if values:
                item[f"{field}_ref"] = _intern_sequence(pool, values)
    strings = sorted({value for values in pool.values() for value in values})
    string_indexes = {value: index for index, value in enumerate(strings)}
    manifest["build_authorities"] = {
        "schema_version": 1,
        "strings": strings,
        "sequences": {
            digest: [string_indexes[value] for value in pool[digest]]
            for digest in sorted(pool)
        },
    }
    for raw_item in objects:
        item = cast(dict[str, Any], raw_item)
        item["unit_sha256"] = _object_unit_sha256(manifest, item)
    return manifest


def _validate_compact_source_extension_manifest(manifest: Mapping[str, Any]) -> None:
    target_triple = manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        raise ValueError("compact extension manifest has no target triple")
    source_plan = manifest.get("source_plan")
    if (
        isinstance(source_plan, Mapping)
        and source_plan.get("kind") == "meson-intro-targets"
    ):
        projection = SourceExtensionLinkProjection.from_manifest(
            source_plan.get("link_projection")
        )
        projection.validate_dialect(target_triple)
    link_requirements, link_requirement_errors = (
        parse_source_extension_link_requirements(
            manifest,
            expected_target_triple=target_triple,
        )
    )
    if link_requirement_errors:
        raise ValueError(
            "compact extension manifest link requirements are invalid: "
            + "; ".join(link_requirement_errors)
        )
    assert link_requirements is not None
    if isinstance(source_plan, Mapping):
        validate_static_python_provider_requirements(
            source_plan.get("python_provider"), link_requirements
        )
    if manifest.get("link_requirements") != link_requirements.manifest_payload():
        raise ValueError(
            "compact extension manifest link requirements are not canonical"
        )
    authorities = manifest.get("build_authorities")
    strings = authorities.get("strings") if isinstance(authorities, Mapping) else None
    sequences = (
        authorities.get("sequences") if isinstance(authorities, Mapping) else None
    )
    if not (
        isinstance(authorities, Mapping)
        and authorities.get("schema_version") == 1
        and isinstance(strings, list)
        and all(isinstance(value, str) and value for value in strings)
        and strings == sorted(set(strings))
        and isinstance(sequences, Mapping)
        and sequences
    ):
        raise ValueError("extension manifest build authority is invalid")
    referenced_string_indexes: set[int] = set()
    for digest, encoded in sequences.items():
        if not isinstance(encoded, list) or not all(
            isinstance(index, int)
            and not isinstance(index, bool)
            and 0 <= index < len(strings)
            for index in encoded
        ):
            raise ValueError("extension manifest has invalid sequence indexes")
        if _canonical_sequence_digest([strings[index] for index in encoded]) != digest:
            raise ValueError("extension manifest has a false sequence digest")
        referenced_string_indexes.update(encoded)
    used_sequences: set[str] = set()

    def require_sequence(
        owner: Mapping[str, Any], field: str, *, required: bool
    ) -> list[str] | None:
        if field in owner:
            raise ValueError(f"compact manifest retains inline {field}")
        reference = owner.get(f"{field}_ref")
        if f"{field}_ref" not in owner:
            if required:
                raise ValueError(f"compact manifest is missing {field}_ref")
            return None
        values = _manifest_sequence(manifest, owner, field)
        used_sequences.add(cast(str, reference))
        return values

    build = manifest.get("build")
    if isinstance(build, Mapping):
        for field in _BUILD_SEQUENCE_FIELDS:
            require_sequence(build, field, required=False)
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if (
        not isinstance(closure, Mapping)
        or closure.get("schema_version")
        != SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION
    ):
        raise ValueError(
            "extension manifest object closure schema_version must be "
            f"{SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION}"
        )
    if not isinstance(objects, list) or not objects:
        raise ValueError("extension manifest object closure is empty")
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise ValueError(f"object_closure.objects[{index}] is invalid")
        item = cast(Mapping[str, Any], item)
        require_source_extension_language(item.get("language"))
        if require_sequence(item, "compile_command", required=True) is None:
            raise ValueError(
                f"object_closure.objects[{index}] compile command is missing"
            )
        if "compile_command_operands" in item and "compile_command_ref" not in item:
            raise ValueError(
                f"object_closure.objects[{index}] compile operands require "
                "compile_command_ref"
            )
        symbol_authority = item.get("symbol_authority")
        if symbol_authority == SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY:
            if require_sequence(item, "symbol_command", required=True) is None:
                raise ValueError(
                    f"object_closure.objects[{index}] symbol command is missing"
                )
        elif symbol_authority == SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY:
            if "symbol_command" in item or "symbol_command_ref" in item:
                raise ValueError(
                    f"object_closure.objects[{index}] WASM symbol authority "
                    "must not retain a symbol command"
                )
        else:
            raise ValueError(
                f"object_closure.objects[{index}] symbol authority is invalid"
            )
        dependencies = item.get("dependencies")
        if dependencies == []:
            if "dependencies_ref" in item:
                raise ValueError("empty dependencies has a redundant reference")
        else:
            require_sequence(item, "dependencies", required=True)
        _manifest_dependencies(manifest, item)
        for field in _OBJECT_SEQUENCE_FIELDS:
            require_sequence(item, field, required=False)
        if item.get("unit_sha256") != _object_unit_sha256(manifest, item):
            raise ValueError(f"object_closure.objects[{index}] unit identity is false")
    if used_sequences != set(sequences):
        raise ValueError("extension manifest has unused or dangling sequence authority")
    if referenced_string_indexes != set(range(len(strings))):
        raise ValueError("extension manifest has unused string authority")
