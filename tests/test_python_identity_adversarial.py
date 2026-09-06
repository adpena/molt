"""Malformed-receipt classes must fail at the public custody boundary."""

from __future__ import annotations

from copy import deepcopy
from typing import Any

import pytest

from molt.exact_json import canonical_json_sha256
from molt.python_capture import (
    PYTHON_CAPTURE_SCHEMA,
    _file_nodes,
    validate_python_capture,
)
from molt.python_environment_custody import validate_python_environment_identity
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_uv_lock_identity import validate_uv_lock_group_closure
from tests.python_environment_test_support import (
    lock_closure_manifest,
    realized_environment_manifest,
    runtime_identity_manifest,
)


_DELETE = object()


@pytest.fixture(scope="module")
def valid_receipts() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    runtime = runtime_identity_manifest()
    environment = realized_environment_manifest(runtime, [("ninja", "1.13.0")])
    lock = lock_closure_manifest(["ninja==1.13.0"], [("ninja", "1.13.0")])
    nodes = sorted(_file_nodes(environment).items())
    files = [
        {
            "path": f"/capture/file-{index:04d}",
            "size": node["size"],
            "sha256": node["sha256"],
        }
        for index, (_pointer, node) in enumerate(nodes)
    ]
    capture = {
        "schema": PYTHON_CAPTURE_SCHEMA,
        "identity": environment,
        "file_custody": files,
        "node_custody": [
            {"node": pointer, "file_index": index}
            for index, (pointer, _node) in enumerate(nodes)
        ],
        "inventory_profile": {
            "hash_workers": 1,
            "hashed_files": len(files),
            "hashed_bytes": sum(row["size"] for row in files),
            "hash_seconds": 0.0,
        },
    }
    assert validate_python_environment_identity(environment) == environment
    assert validate_uv_lock_group_closure(lock) == lock
    assert validate_python_capture(capture) == capture
    return environment, lock, capture


def _mutate(target: dict[str, Any], field: str, value: object) -> None:
    if value is _DELETE:
        del target[field]
    else:
        target[field] = deepcopy(value)


def _reseal_environment(payload: dict[str, Any]) -> None:
    tree = payload["tree"]
    if "entries" in tree:
        tree["manifest_sha256"] = canonical_json_sha256(tree["entries"])
    for distribution in payload["distributions"]:
        if "installed_files" in distribution:
            distribution["file_manifest_sha256"] = canonical_json_sha256(
                distribution["installed_files"]
            )
        if "entry_points" in distribution:
            distribution["entry_points_sha256"] = canonical_json_sha256(
                distribution["entry_points"]
            )
    payload["distribution_inventory_sha256"] = canonical_json_sha256(
        payload["distributions"]
    )
    payload["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in payload.items()
            if key != "environment_closure_sha256"
        }
    )


@pytest.mark.parametrize(
    "target,field,value",
    [
        pytest.param("environment", "operating_system", [], id="unhashable-os"),
        pytest.param("environment", "architecture", {}, id="unhashable-arch"),
        pytest.param("environment", "pointer_bits", 64.0, id="float-pointer-width"),
        pytest.param("environment", "py_debug", 0, id="integer-debug-flag"),
        pytest.param("environment", "gil_disabled", 0, id="integer-gil-flag"),
        pytest.param(
            "capabilities", "required_active_import_roles", [[]], id="unhashable-role"
        ),
        pytest.param("capabilities", "isolated", 1, id="integer-isolation-flag"),
        pytest.param(
            "capabilities", "runtime", _DELETE, id="missing-capability-runtime"
        ),
        pytest.param("tree_file", "kind", [], id="unhashable-entry-kind"),
        pytest.param("tree_file", "node", {}, id="unhashable-entry-node"),
        pytest.param("tree_file", "node", _DELETE, id="missing-entry-node"),
        pytest.param(
            "tree_file",
            "access",
            {"readable": 1, "writable": False, "executable": True},
            id="integer-access-flag",
        ),
        pytest.param("tree_node", "size", True, id="boolean-node-size"),
        pytest.param("tree_node", "sha256", _DELETE, id="missing-node-digest"),
        pytest.param("tree", "file_count", 3.0, id="float-file-count"),
        pytest.param("tree", "entries", _DELETE, id="missing-tree-entries"),
        pytest.param("selected", "node", [], id="unhashable-selected-node"),
        pytest.param("selected", "path", _DELETE, id="missing-selected-path"),
        pytest.param("active_import", "role", {}, id="unhashable-import-role"),
        pytest.param(
            "distribution", "installed_file_count", True, id="boolean-installed-count"
        ),
        pytest.param(
            "distribution", "installed_files", _DELETE, id="missing-installed-files"
        ),
        pytest.param(
            "distribution", "entry_points", _DELETE, id="missing-entry-points"
        ),
        pytest.param(
            "distribution", "record_sha256", "A" * 64, id="noncanonical-record-digest"
        ),
        pytest.param("installed_file", "node", [], id="unhashable-installed-node"),
    ],
)
def test_environment_resealed_malformed_receipts_raise_custody_error(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
    target: str,
    field: str,
    value: object,
) -> None:
    environment = deepcopy(valid_receipts[0])
    targets = {
        "environment": environment,
        "capabilities": environment["capabilities"],
        "tree": environment["tree"],
        "tree_file": next(
            row for row in environment["tree"]["entries"] if row["kind"] == "file"
        ),
        "tree_node": environment["tree"]["file_nodes"][0],
        "selected": environment["selected_executable"],
        "active_import": environment["active_import_roots"][0],
        "distribution": environment["distributions"][0],
        "installed_file": environment["distributions"][0]["installed_files"][0],
    }
    _mutate(targets[target], field, value)
    _reseal_environment(environment)
    with pytest.raises(PythonEnvironmentIdentityError):
        validate_python_environment_identity(environment)


@pytest.mark.parametrize(
    "target,field,value",
    [
        pytest.param("lock", "lock_version", True, id="boolean-lock-version"),
        pytest.param("lock", "lock_revision", 3.0, id="float-lock-revision"),
        pytest.param("lock", "requirements", [[]], id="nested-requirement-list"),
        pytest.param(
            "lock",
            "requirements",
            ['ninja; python_version ~= "not-a-version"'],
            id="invalid-marker-comparison",
        ),
        pytest.param(
            "lock", "requires_python", ">=invalid", id="invalid-python-specifier"
        ),
        pytest.param(
            "marker", "python_full_version", _DELETE, id="missing-marker-version"
        ),
        pytest.param("marker", "platform_machine", [], id="nonstring-marker-value"),
        pytest.param("package", "name", [], id="unhashable-package-name"),
        pytest.param("package", "version", _DELETE, id="missing-package-version"),
        pytest.param("package", "source", {}, id="missing-registry-source"),
        pytest.param("artifact", "filename", _DELETE, id="missing-wheel-filename"),
        pytest.param("artifact", "filename", {}, id="nonstring-wheel-filename"),
        pytest.param("artifact", "size", True, id="boolean-artifact-size"),
        pytest.param("artifact", "size", 1.0, id="float-artifact-size"),
        pytest.param("artifact", "sha256", [], id="nonstring-artifact-digest"),
    ],
)
def test_lock_resealed_malformed_receipts_raise_custody_error(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
    target: str,
    field: str,
    value: object,
) -> None:
    lock = deepcopy(valid_receipts[1])
    targets = {
        "lock": lock,
        "marker": lock["marker_environment"],
        "package": lock["packages"][0],
        "artifact": lock["packages"][0]["artifact"],
    }
    _mutate(targets[target], field, value)
    lock["closure_sha256"] = canonical_json_sha256(
        {key: value for key, value in lock.items() if key != "closure_sha256"}
    )
    with pytest.raises(PythonEnvironmentIdentityError):
        validate_uv_lock_group_closure(lock)


@pytest.mark.parametrize(
    "target,field,value",
    [
        pytest.param("capture", "identity", {}, id="empty-nested-identity"),
        pytest.param("capture", "identity", [], id="nonmapping-identity"),
        pytest.param("capture", "file_custody", {}, id="nonlist-custody"),
        pytest.param("capture", "inventory_profile", _DELETE, id="missing-profile"),
        pytest.param("file", "path", {}, id="nonstring-file-path"),
        pytest.param("file", "path", "relative/file", id="relative-file-path"),
        pytest.param("file", "path", "/capture/../file", id="noncanonical-file-path"),
        pytest.param("file", "size", True, id="boolean-file-size"),
        pytest.param("file", "size", 1.0, id="float-file-size"),
        pytest.param("file", "sha256", _DELETE, id="missing-file-digest"),
        pytest.param("file", "sha256", "F" * 64, id="uppercase-file-digest"),
        pytest.param("profile", "hash_workers", True, id="boolean-worker-count"),
        pytest.param("profile", "hash_workers", 0, id="zero-worker-count"),
        pytest.param("profile", "hashed_files", 1.0, id="float-hashed-count"),
        pytest.param("profile", "hashed_bytes", -1, id="negative-hashed-bytes"),
        pytest.param("profile", "hash_seconds", True, id="boolean-timing"),
        pytest.param("profile", "hash_seconds", float("nan"), id="nan-timing"),
        pytest.param("profile", "hash_seconds", float("inf"), id="infinite-timing"),
        pytest.param("profile", "hash_seconds", _DELETE, id="missing-timing"),
    ],
)
def test_capture_malformed_envelope_raises_custody_error(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
    target: str,
    field: str,
    value: object,
) -> None:
    capture = deepcopy(valid_receipts[2])
    targets = {
        "capture": capture,
        "file": capture["file_custody"][0],
        "profile": capture["inventory_profile"],
    }
    _mutate(targets[target], field, value)
    with pytest.raises(PythonEnvironmentIdentityError):
        validate_python_capture(capture)


@pytest.mark.parametrize("surface", ["environment", "lock", "capture"])
def test_nonfinite_identity_values_cannot_escape_public_error_contract(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
    surface: str,
) -> None:
    environment, lock, capture = deepcopy(valid_receipts)
    # NaN cannot be canonically resealed. The validators must normalize the
    # serialization failure too, not leak an unrelated ValueError to consumers.
    if surface == "lock":
        lock["packages"][0]["artifact"]["size"] = float("nan")
        validator, payload = validate_uv_lock_group_closure, lock
    else:
        selected = capture["identity"] if surface == "capture" else environment
        selected["pointer_bits"] = float("nan")
        validator, payload = (
            (validate_python_capture, capture)
            if surface == "capture"
            else (validate_python_environment_identity, environment)
        )
    with pytest.raises(PythonEnvironmentIdentityError):
        validator(payload)


@pytest.mark.parametrize("surface", ["environment", "lock"])
@pytest.mark.parametrize("digest", [None, [], True, "F" * 64, "0" * 64])
def test_public_identity_digest_rejection_uses_custody_error(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
    surface: str,
    digest: object,
) -> None:
    if surface == "environment":
        payload = deepcopy(valid_receipts[0])
        payload["environment_closure_sha256"] = digest
        validator = validate_python_environment_identity
    else:
        payload = deepcopy(valid_receipts[1])
        payload["closure_sha256"] = digest
        validator = validate_uv_lock_group_closure
    with pytest.raises(PythonEnvironmentIdentityError):
        validator(payload)


def test_capture_rejects_windows_case_aliases_on_every_host(
    valid_receipts: tuple[dict[str, Any], dict[str, Any], dict[str, Any]],
) -> None:
    capture = deepcopy(valid_receipts[2])
    capture["file_custody"][0]["path"] = "C:\\Capture\\file.bin"
    capture["file_custody"].append(
        {**capture["file_custody"][0], "path": "c:\\capture\\FILE.bin"}
    )
    capture["file_custody"].sort(key=lambda row: row["path"])
    with pytest.raises(PythonEnvironmentIdentityError, match="path alias"):
        validate_python_capture(capture)


@pytest.mark.parametrize("root", ["C:\\Workspace", "/workspace"])
def test_external_admission_path_grammar_is_independent_of_validation_host(
    valid_receipts, root
):
    environment = deepcopy(valid_receipts[0])
    environment["external_roots"] = [{"id": "external-root-0", "path": root}]
    _reseal_environment(environment)
    assert validate_python_environment_identity(environment) == environment


@pytest.mark.parametrize(
    "paths",
    [
        ["C:\\Workspace", "c:\\workspace"],
        ["C:\\Workspace", "C:\\Workspace\\nested"],
        ["/workspace", "/workspace/nested"],
        ["C:\\Workspace\\..\\elsewhere"],
        ["/workspace/../elsewhere"],
        ["C:\\Workspace."],
        ["C:\\Workspace "],
        ["C:\\Workspace.\\nested"],
        ["C:\\Workspace\\file:stream"],
        ["C:\\Workspace\\NUL"],
        ["C:\\NUL\\nested"],
        ["C:\\Workspace\\CON .txt"],
        ["C:\\Workspace\\CONIN$"],
        ["C:\\Workspace\\conout$.log"],
        ["C:\\Workspace\\COM¹.log"],
        ["C:\\Workspace\\wild*card"],
        ["\\\\?\\C:\\Workspace"],
        ["\\\\.\\C:\\Workspace"],
    ],
)
def test_external_admission_rejects_resealed_alias_or_overlapping_roots(
    valid_receipts, paths
):
    environment = deepcopy(valid_receipts[0])
    environment["external_roots"] = [
        {"id": f"external-root-{index}", "path": path}
        for index, path in enumerate(paths)
    ]
    _reseal_environment(environment)
    with pytest.raises(PythonEnvironmentIdentityError):
        validate_python_environment_identity(environment)
