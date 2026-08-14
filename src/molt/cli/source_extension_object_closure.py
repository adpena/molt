"""Canonical content identity for a source-extension object closure."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any, Mapping, cast

from molt.cli.source_extension_manifest_codec import (
    _manifest_dependencies,
    _manifest_sequence,
)
from molt.file_hashing import _sha256_file


class SourceExtensionObjectClosureError(ValueError):
    """An object closure is incomplete, non-canonical, or differs from its bytes."""


def source_extension_object_closure_digest(
    object_closure: Mapping[str, Any],
    *,
    manifest_dir: Path | None = None,
    manifest: Mapping[str, Any] | None = None,
) -> str:
    objects = object_closure.get("objects")
    runtime_symbols = object_closure.get("runtime_symbols")
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionObjectClosureError(
            "extension object_closure.objects is empty"
        )
    if not isinstance(runtime_symbols, list) or not all(
        isinstance(item, str) for item in runtime_symbols
    ):
        raise SourceExtensionObjectClosureError(
            "extension object_closure.runtime_symbols must be a string array"
        )
    digest_objects: list[dict[str, Any]] = []
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] is not an object"
            )
        item = cast(Mapping[str, Any], item)
        source = item.get("source")
        object_path = item.get("object")
        source_sha256 = item.get("source_sha256")
        object_sha256 = item.get("object_sha256")
        if not (
            isinstance(source, str)
            and source
            and isinstance(object_path, str)
            and object_path
            and isinstance(source_sha256, str)
            and source_sha256
            and isinstance(object_sha256, str)
            and object_sha256
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] lacks checksum custody"
            )
        source_path = Path(source)
        if not source_path.is_absolute() and manifest_dir is not None:
            source_path = manifest_dir / source_path
        if not source_path.is_file():
            raise SourceExtensionObjectClosureError(
                f"extension object_closure source is missing: {source_path}"
            )
        if _sha256_file(source_path) != source_sha256:
            raise SourceExtensionObjectClosureError(
                f"extension object_closure source checksum mismatch: {source_path}"
            )
        authority = (
            manifest if manifest is not None else {"object_closure": object_closure}
        )
        try:
            compile_command = _manifest_sequence(authority, item, "compile_command")
            symbol_command = _manifest_sequence(authority, item, "symbol_command")
            defined_symbols = (
                _manifest_sequence(authority, item, "defined_symbols") or []
            )
            undefined_symbols = (
                _manifest_sequence(authority, item, "undefined_symbols") or []
            )
        except ValueError as exc:
            raise SourceExtensionObjectClosureError(str(exc)) from exc
        if not (
            isinstance(defined_symbols, list)
            and all(isinstance(value, str) for value in defined_symbols)
            and isinstance(undefined_symbols, list)
            and all(isinstance(value, str) for value in undefined_symbols)
            and isinstance(compile_command, list)
            and bool(compile_command)
            and all(isinstance(value, str) and value for value in compile_command)
            and isinstance(symbol_command, list)
            and bool(symbol_command)
            and all(isinstance(value, str) and value for value in symbol_command)
        ):
            raise SourceExtensionObjectClosureError(
                f"extension object_closure.objects[{index}] has invalid symbols "
                "or tool command"
            )
        digest_object: dict[str, Any] = {
            "source": source,
            "object": object_path,
            "source_sha256": source_sha256,
            "object_sha256": object_sha256,
            "defined_symbols": defined_symbols,
            "undefined_symbols": undefined_symbols,
            "compile_command": compile_command,
            "symbol_command": symbol_command,
        }
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
            dependency_sha256 = raw_dependency.get("sha256")
            if not (
                isinstance(dependency_path_raw, str)
                and dependency_path_raw
                and isinstance(dependency_sha256, str)
                and dependency_sha256
            ):
                raise SourceExtensionObjectClosureError(
                    "extension object_closure dependency lacks path/checksum "
                    f"at objects[{index}].dependencies[{dependency_index}]"
                )
            dependency_path = Path(dependency_path_raw)
            if not dependency_path.is_absolute() and manifest_dir is not None:
                dependency_path = manifest_dir / dependency_path
            if not dependency_path.is_file():
                raise SourceExtensionObjectClosureError(
                    f"extension object_closure dependency is missing: {dependency_path}"
                )
            if _sha256_file(dependency_path) != dependency_sha256:
                raise SourceExtensionObjectClosureError(
                    "extension object_closure dependency checksum mismatch: "
                    f"{dependency_path}"
                )
            dependencies.append(
                {"path": dependency_path_raw, "sha256": dependency_sha256}
            )
        digest_object["dependencies"] = dependencies
        digest_objects.append(digest_object)
    digest_payload = {
        "schema_version": 1,
        "root_symbol": object_closure.get("root_symbol"),
        "objects": digest_objects,
        "runtime_symbols": runtime_symbols,
    }
    encoded = json.dumps(digest_payload, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()
