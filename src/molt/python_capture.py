"""Nonsemantic host custody around one portable Python identity capture."""

from __future__ import annotations

from collections.abc import Mapping
import math
from pathlib import PurePath
from typing import cast

from molt.python_file_node_custody import PythonFileCaptureContext
from molt.python_identity_common import (
    PythonEnvironmentIdentityError,
    canonical_absolute_path,
    _valid_sha256,
    identity_validator,
)

PYTHON_CAPTURE_SCHEMA = "molt.python-capture.v2"


def _file_nodes(value: object, pointer: str = "") -> dict[str, Mapping[str, object]]:
    """Address semantic nodes explicitly; identical content does not imply aliasing."""
    nodes: dict[str, Mapping[str, object]] = {}
    if isinstance(value, Mapping):
        for key, item in value.items():
            child = pointer + "/" + str(key).replace("~", "~0").replace("/", "~1")
            if key == "file_nodes" and isinstance(item, list):
                for index, row in enumerate(item):
                    if isinstance(row, Mapping):
                        nodes[f"{child}/{index}"] = cast(Mapping[str, object], row)
            else:
                nodes.update(_file_nodes(item, child))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            nodes.update(_file_nodes(item, f"{pointer}/{index}"))
    return nodes


@identity_validator("Python capture")
def validate_python_capture(payload: object) -> dict[str, object]:
    """Validate the complete capture envelope without consulting host files."""

    if (
        not isinstance(payload, dict)
        or set(payload)
        != {"schema", "identity", "file_custody", "node_custody", "inventory_profile"}
        or payload.get("schema") != PYTHON_CAPTURE_SCHEMA
    ):
        raise PythonEnvironmentIdentityError("Python capture envelope shape is invalid")
    identity = payload.get("identity")
    if not isinstance(identity, dict):
        raise PythonEnvironmentIdentityError("Python capture identity is invalid")
    from molt.python_runtime_identity import (
        PYTHON_RUNTIME_IDENTITY_SCHEMA,
        validate_python_runtime_identity,
    )

    if identity.get("schema") == PYTHON_RUNTIME_IDENTITY_SCHEMA:
        validate_python_runtime_identity(identity)
    else:
        from molt.python_environment_custody import validate_python_environment_identity

        validate_python_environment_identity(identity)
    files = payload.get("file_custody")
    if not isinstance(files, list):
        raise PythonEnvironmentIdentityError("Python capture has no file custody")
    paths: list[str] = []
    path_identities: set[PurePath] = set()
    contents: list[tuple[int, str]] = []
    for row in files:
        if not isinstance(row, dict) or set(row) != {"path", "size", "sha256"}:
            raise PythonEnvironmentIdentityError("Python capture file row is invalid")
        path, size, digest = row.get("path"), row.get("size"), row.get("sha256")
        if not isinstance(path, str) or "\x00" in path:
            raise PythonEnvironmentIdentityError("Python capture path is invalid")
        parsed = canonical_absolute_path(path)
        if parsed in path_identities:
            raise PythonEnvironmentIdentityError("Python capture repeats a path alias")
        path_identities.add(parsed)
        if type(size) is not int or size < 0 or not _valid_sha256(digest):
            raise PythonEnvironmentIdentityError(
                "Python capture file content is invalid"
            )
        paths.append(path)
        contents.append((size, str(digest)))
    if paths != sorted(set(paths)):
        raise PythonEnvironmentIdentityError(
            "Python capture paths are not sorted and unique"
        )
    nodes = _file_nodes(identity)
    bindings = payload.get("node_custody")
    if not isinstance(bindings, list):
        raise PythonEnvironmentIdentityError("Python capture has no file-node custody")
    observed: list[str] = []
    pool_files: dict[str, set[int]] = {}
    for binding in bindings:
        if not isinstance(binding, dict) or set(binding) != {"node", "file_index"}:
            raise PythonEnvironmentIdentityError(
                "Python capture node binding is invalid"
            )
        pointer, index = binding.get("node"), binding.get("file_index")
        if (
            not isinstance(pointer, str)
            or type(index) is not int
            or not 0 <= index < len(contents)
        ):
            raise PythonEnvironmentIdentityError(
                "Python capture node binding is invalid"
            )
        node = nodes.get(pointer)
        if node is None or contents[index] != (node["size"], node["sha256"]):
            raise PythonEnvironmentIdentityError(
                f"Python capture file-node custody differs from its content: {pointer}"
            )
        pool = pointer.rsplit("/", 1)[0]
        used = pool_files.setdefault(pool, set())
        if index in used:
            raise PythonEnvironmentIdentityError(
                f"Python capture aliases distinct nodes within one pool: {pointer}"
            )
        used.add(index)
        observed.append(pointer)
    if observed != sorted(nodes):
        raise PythonEnvironmentIdentityError(
            "Python capture file-node custody is not exact, sorted and unique"
        )
    profile = payload.get("inventory_profile")
    if not isinstance(profile, dict) or set(profile) != {
        "hash_workers",
        "hashed_files",
        "hashed_bytes",
        "hash_seconds",
    }:
        raise PythonEnvironmentIdentityError("Python capture profile is invalid")
    workers = profile.get("hash_workers")
    if type(workers) is not int or not 1 <= workers <= 32:
        raise PythonEnvironmentIdentityError("Python capture worker count is invalid")
    for key in ("hashed_files", "hashed_bytes"):
        value = profile.get(key)
        if type(value) is not int or value < 0:
            raise PythonEnvironmentIdentityError(f"Python capture {key} is invalid")
    seconds = profile.get("hash_seconds")
    if (
        not isinstance(seconds, (int, float))
        or isinstance(seconds, bool)
        or (isinstance(seconds, float) and not math.isfinite(seconds))
        or seconds < 0
    ):
        raise PythonEnvironmentIdentityError("Python capture timing is invalid")
    return cast(dict[str, object], payload)


def python_capture_payload(
    identity: dict[str, object], context: PythonFileCaptureContext
) -> dict[str, object]:
    files = context.file_custody()
    indexes = {str(row["path"]): index for index, row in enumerate(files)}
    return validate_python_capture(
        {
            "schema": PYTHON_CAPTURE_SCHEMA,
            "identity": identity,
            "file_custody": files,
            "node_custody": [
                {"node": pointer, "file_index": indexes[str(context.node_path(node))]}
                for pointer, node in sorted(_file_nodes(identity).items())
            ],
            "inventory_profile": context.inventory_profile(),
        }
    )
