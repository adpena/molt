"""Compact, lossless authority codec for source-extension manifests."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping, Sequence
from typing import Any

_BUILD_SEQUENCE_FIELDS = (
    "compiler",
    "extra_compile_args",
    "include_dirs",
    "linker",
    "extra_link_args",
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
    if inline is not None and reference is not None:
        raise ValueError(f"{field} has both inline and referenced authority")
    if inline is not None:
        if not isinstance(inline, list) or not all(
            isinstance(value, str) and value for value in inline
        ):
            raise ValueError(f"{field} inline authority is invalid")
        return list(inline)
    if reference is None:
        return None
    authorities = manifest.get("build_authorities")
    sequences = (
        authorities.get("sequences") if isinstance(authorities, Mapping) else None
    )
    strings = authorities.get("strings") if isinstance(authorities, Mapping) else None
    encoded = sequences.get(reference) if isinstance(sequences, Mapping) else None
    if not (
        isinstance(reference, str)
        and isinstance(strings, list)
        and isinstance(encoded, list)
        and all(
            isinstance(index, int) and 0 <= index < len(strings) for index in encoded
        )
    ):
        raise ValueError(f"{field} references an invalid sequence authority")
    result = [strings[index] for index in encoded]
    if (
        not all(isinstance(value, str) and value for value in result)
        or _canonical_sequence_digest(result) != reference
    ):
        raise ValueError(f"{field} sequence authority digest is false")
    if field == "compile_command":
        operands = owner.get("compile_command_operands")
        if operands is not None:
            if not isinstance(operands, list) or not all(
                isinstance(item, Mapping)
                and set(item) == {"index", "value"}
                and isinstance(item.get("index"), int)
                and isinstance(item.get("value"), str)
                and 0 <= item["index"] < len(result)
                for item in operands
            ):
                raise ValueError("compile_command_operands is invalid")
            indexes = [item["index"] for item in operands]
            if indexes != sorted(set(indexes)):
                raise ValueError("compile_command_operands indexes are not canonical")
            for item in operands:
                result[item["index"]] = item["value"]
    return result


def _manifest_dependencies(
    manifest: Mapping[str, Any], owner: Mapping[str, Any]
) -> list[dict[str, str]]:
    inline = owner.get("dependencies")
    reference = owner.get("dependencies_ref")
    if inline is not None and reference is not None:
        raise ValueError("dependencies has both inline and referenced authority")
    if inline is not None:
        if not isinstance(inline, list) or not all(
            isinstance(item, Mapping)
            and set(item) == {"path", "sha256"}
            and isinstance(item.get("path"), str)
            and bool(item.get("path"))
            and isinstance(item.get("sha256"), str)
            and bool(item.get("sha256"))
            for item in inline
        ):
            raise ValueError("inline dependencies authority is invalid")
        return [dict(item) for item in inline]
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
    operand_flags = {"-c", "-o", "-MF", "-MT"}
    for index, value in enumerate(template[:-1]):
        if value in operand_flags:
            operand_index = index + 1
            operands.append({"index": operand_index, "value": template[operand_index]})
            template[operand_index] = "%{operand}"
    for index, value in enumerate(template):
        if value.startswith("/Fo") and len(value) > 3:
            operands.append({"index": index, "value": value})
            template[index] = "%{operand}"
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
    payload["compile_command"] = _manifest_sequence(manifest, item, "compile_command")
    payload["symbol_command"] = _manifest_sequence(manifest, item, "symbol_command")
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


def _compact_source_extension_manifest(manifest: Mapping[str, Any]) -> dict[str, Any]:
    """Compact an owned manifest in place while preserving exact argv."""

    if not isinstance(manifest, dict):
        raise ValueError("source-extension manifest compaction requires owned dict")
    pool: dict[str, list[str]] = {}
    build = manifest.get("build")
    if isinstance(build, dict):
        for field in _BUILD_SEQUENCE_FIELDS:
            raw = build.pop(field, None)
            if raw is None:
                continue
            if not isinstance(raw, list) or not all(
                isinstance(value, str) and value for value in raw
            ):
                raise ValueError(f"build.{field} must be a string array")
            values = list(raw)
            if values:
                build[f"{field}_ref"] = _intern_sequence(pool, values)
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise ValueError("source-extension object closure is empty")
    for index, item in enumerate(objects):
        if not isinstance(item, dict):
            raise ValueError(f"object_closure.objects[{index}] must be an object")
        for field in ("compile_command", "symbol_command"):
            raw = item.pop(field, None)
            if not isinstance(raw, list):
                raise ValueError(f"object_closure.objects[{index}].{field} is invalid")
            values = raw
            if field == "compile_command":
                values, operands = _compile_command_template(raw)
                if operands:
                    item["compile_command_operands"] = operands
            item[f"{field}_ref"] = _intern_sequence(pool, values)
        dependencies = item.pop("dependencies", None)
        if not isinstance(dependencies, list) or not all(
            isinstance(dep, Mapping)
            and set(dep) == {"path", "sha256"}
            and isinstance(dep.get("path"), str)
            and bool(dep.get("path"))
            and isinstance(dep.get("sha256"), str)
            and bool(dep.get("sha256"))
            for dep in dependencies
        ):
            raise ValueError(f"object_closure.objects[{index}].dependencies is invalid")
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
            raw = item.pop(field, None)
            if raw:
                if not isinstance(raw, list):
                    raise ValueError(
                        f"object_closure.objects[{index}].{field} is invalid"
                    )
                item[f"{field}_ref"] = _intern_sequence(pool, raw)
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
    for item in objects:
        item["unit_sha256"] = _object_unit_sha256(manifest, item)
    return manifest


def _validate_compact_source_extension_manifest(manifest: Mapping[str, Any]) -> None:
    authorities = manifest.get("build_authorities")
    strings = authorities.get("strings") if isinstance(authorities, Mapping) else None
    sequences = (
        authorities.get("sequences") if isinstance(authorities, Mapping) else None
    )
    if not (
        isinstance(authorities, Mapping)
        and authorities.get("schema_version") == 1
        and isinstance(strings, list)
        and strings == sorted(set(strings))
        and all(isinstance(value, str) and value for value in strings)
        and isinstance(sequences, Mapping)
        and sequences
    ):
        raise ValueError("extension manifest build authority is invalid")
    referenced_string_indexes: set[int] = set()
    for digest, encoded in sequences.items():
        if not isinstance(encoded, list) or not all(
            isinstance(index, int) and 0 <= index < len(strings) for index in encoded
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
        if reference is None:
            if required:
                raise ValueError(f"compact manifest is missing {field}_ref")
            return None
        if not isinstance(reference, str):
            raise ValueError(f"compact manifest {field}_ref is invalid")
        used_sequences.add(reference)
        return _manifest_sequence(manifest, owner, field)

    build = manifest.get("build")
    if isinstance(build, Mapping):
        for field in _BUILD_SEQUENCE_FIELDS:
            require_sequence(build, field, required=False)
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise ValueError("extension manifest object closure is empty")
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise ValueError(f"object_closure.objects[{index}] is invalid")
        if require_sequence(item, "compile_command", required=True) is None:
            raise ValueError(
                f"object_closure.objects[{index}] compile command is missing"
            )
        if require_sequence(item, "symbol_command", required=True) is None:
            raise ValueError(
                f"object_closure.objects[{index}] symbol command is missing"
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
