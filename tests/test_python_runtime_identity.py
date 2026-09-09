"""Pure runtime-receipt validation; fixtures do not inspect the host Python."""

from __future__ import annotations

from typing import Any

import pytest

from molt import python_runtime_identity as runtime
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError


def _reseal(payload: dict[str, Any]) -> None:
    dependency = payload["native_dependency_closure"]
    dependency["closure_sha256"] = canonical_json_sha256(
        {
            key: dependency[key]
            for key in dependency
            if key not in {"status", "closure_sha256"}
        }
    )
    payload["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in payload.items()
            if key != "runtime_closure_sha256"
        }
    )


@pytest.mark.parametrize(
    "operating_system,old_policy",
    [
        ("windows", "pe-loaded-import-closure-v2"),
        ("linux", "elf-loaded-needed-closure-v2"),
        ("macos", "mach-o-loaded-dylib-closure-v3"),
    ],
)
def test_runtime_rejects_pre_loader_executable_custody(
    operating_system: str, old_policy: str
) -> None:
    payload = _runtime_payload(operating_system=operating_system)
    payload["capabilities"]["native_dependency_policy"] = old_policy
    payload["native_dependency_closure"]["policy"] = old_policy
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError):
        runtime.validate_python_runtime_identity(payload)


def _runtime_payload(
    *,
    operating_system: str = "linux",
    architecture: str = "x86_64",
    version: str = "3.12.1",
) -> dict[str, Any]:
    """The donor's fixture topology, with distinct executable/library/Unicode nodes."""
    windows = operating_system == "windows"
    policy = runtime._NATIVE_DEPENDENCY_POLICIES[operating_system]
    root_roles = [
        "base-dlls" if windows else "base-lib-dynload",
        "platstdlib",
        "stdlib",
    ]
    roles = ["base-executable", "runtime-library", "unicodedata"]
    names = [
        "python.exe" if windows else "python",
        "a-runtime.dll" if windows else "libpython.so",
        "unicodedata.pyd" if windows else "unicodedata.so",
    ]
    access = {"readable": True, "writable": False, "executable": True}
    entries = [
        {"path": "lib", "kind": "directory", "access": dict(access)},
        {
            "path": f"lib/{names[2]}",
            "kind": "file",
            "node": "file-node-2",
            "access": dict(access),
        },
    ]
    payload: dict[str, Any] = {
        "schema": runtime.PYTHON_RUNTIME_IDENTITY_SCHEMA,
        "implementation": "cpython",
        "version": version,
        "cache_tag": "cpython-" + "".join(version.split(".")[:2]),
        "soabi": "cpython-test-abi",
        "abi_flags": "",
        "multiarch": "test-arch",
        "py_debug": False,
        "gil_disabled": False,
        "operating_system": operating_system,
        "architecture": architecture,
        "pointer_bits": 64,
        "byteorder": "little",
        "capabilities": {
            "schema": runtime.PYTHON_RUNTIME_CAPABILITY_SCHEMA,
            "implementation_policy": "cpython>=3.12",
            "version_series": ".".join(version.split(".")[:2]),
            "operating_system": operating_system,
            "architecture": architecture,
            "linkage": "shared",
            "unicodedata_linkage": "dynamic",
            "scanner_policy": "no-follow-handle-two-snapshot-sha256-v1",
            "import_root_policy": "isolated-active-prefix-roots-v1",
            "native_dependency_policy": policy,
            "required_root_roles": root_roles,
            "required_explicit_roles": roles,
        },
        "file_nodes": [
            {
                "id": f"file-node-{index}",
                "size": index + 1,
                "sha256": str(index + 1) * 64,
            }
            for index in range(3)
        ],
        "native_dependency_closure": {
            "status": "closed",
            "policy": policy,
            "executable_component": "native-component-1",
            "root_components": [f"native-component-{index}" for index in range(3)],
            "observed_components": [f"native-component-{index}" for index in range(3)],
            "observed_contracts": [],
            "deferred_imports": [],
            "components": [
                {
                    "id": f"native-component-{component_index}",
                    "filename": names[node_index],
                    "node": f"file-node-{node_index}",
                    "roles": [roles[node_index]],
                }
                for component_index, node_index in enumerate((1, 0, 2))
            ],
            "contracts": [],
            "edges": [
                {"from": "native-component-1", "to": "native-component-0"},
                {"from": "native-component-2", "to": "native-component-0"},
            ],
        },
        "explicit_files": [
            {
                "role": role,
                "kind": "node-reference",
                "filename": names[index],
                "node": f"file-node-{index}",
            }
            if index < 2
            else {
                "role": role,
                "kind": "root-reference",
                "root": "runtime-root-0",
                "path": f"lib/{names[index]}",
                "node": f"file-node-{index}",
            }
            for index, role in enumerate(roles)
        ],
        "import_roots": [{"kind": "directory", "root": "runtime-root-0", "path": "."}],
        "runtime_root_roles": [
            {"role": role, "root": "runtime-root-0", "path": "."} for role in root_roles
        ],
        "runtime_roots": [
            {
                "id": "runtime-root-0",
                "file_count": 1,
                "node_ids": ["file-node-2"],
                "entries": entries,
                "manifest_sha256": canonical_json_sha256(entries),
            }
        ],
    }
    _reseal(payload)
    return payload


@pytest.mark.parametrize("operating_system", ["windows", "macos", "linux"])
@pytest.mark.parametrize("architecture", ["x86_64", "arm64"])
@pytest.mark.parametrize(
    "version", ["3.12.1", "3.13.0", "3.14.0", "3.15.0a1", "3.15.0b2", "3.15.0rc1"]
)
def test_runtime_receipt_platform_version_architecture_matrix(
    operating_system: str, architecture: str, version: str
) -> None:
    payload = _runtime_payload(
        operating_system=operating_system, architecture=architecture, version=version
    )
    assert runtime.validate_python_runtime_identity(payload) == payload
    assert runtime.runtime_explicit_file_content(payload, "unicodedata") == {
        "filename": "unicodedata.pyd"
        if operating_system == "windows"
        else "unicodedata.so",
        "size": 3,
        "sha256": "3" * 64,
    }


def test_macos_runtime_receipt_accepts_distinct_python_basename_components() -> None:
    payload = _runtime_payload(operating_system="macos")
    dependency = payload["native_dependency_closure"]
    dependency["components"][:2] = [
        {
            "id": "native-component-0",
            "filename": "Python",
            "node": "file-node-0",
            "roles": ["base-executable"],
        },
        {
            "id": "native-component-1",
            "filename": "Python",
            "node": "file-node-1",
            "roles": ["runtime-library"],
        },
    ]
    payload["explicit_files"][0]["filename"] = "Python"
    payload["explicit_files"][1]["filename"] = "Python"
    _reseal(payload)

    assert runtime.validate_python_runtime_identity(payload) == payload


@pytest.mark.parametrize("operating_system", ["windows", "linux"])
def test_receipt_distinguishes_configured_and_loaded_same_name_files(
    operating_system: str,
) -> None:
    payload = _runtime_payload(operating_system=operating_system)
    dependency = payload["native_dependency_closure"]
    dependency["components"][:2] = [
        {
            "id": "native-component-0",
            "filename": "Python",
            "node": "file-node-0",
            "roles": ["base-executable"],
        },
        {
            "id": "native-component-1",
            "filename": "Python",
            "node": "file-node-1",
            "roles": ["runtime-library"],
        },
    ]
    payload["explicit_files"][0]["filename"] = "Python"
    payload["explicit_files"][1]["filename"] = "Python"
    dependency["observed_components"] = ["native-component-1", "native-component-2"]
    dependency["executable_component"] = "native-component-1"
    dependency["edges"] = []
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload
    dependency["observed_components"].insert(0, "native-component-0")
    _reseal(payload)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="loader names are ambiguous"
    ):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize("value", [None, [], "native-component-99"])
def test_runtime_requires_an_observed_executable_component(value: object) -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["executable_component"] = value
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="executable.*observed"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize("surface", ["executable", "importer", "provider", "deferred"])
def test_runtime_never_uses_configured_only_root_as_loaded_image(surface: str) -> None:
    payload = _runtime_payload()
    dependency = payload["native_dependency_closure"]
    dependency["observed_components"].remove("native-component-1")
    dependency["executable_component"] = "native-component-0"
    dependency["edges"] = []
    if surface == "executable":
        dependency["executable_component"] = "native-component-1"
    elif surface == "importer":
        dependency["edges"] = [
            {"from": "native-component-1", "to": "native-component-0"}
        ]
    elif surface == "provider":
        dependency["edges"] = [
            {"from": "native-component-0", "to": "native-component-1"}
        ]
    else:
        dependency["deferred_imports"] = [
            {"from": "native-component-1", "name": "absent", "kind": "filter"}
        ]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "version",
    [
        "3.11.9",
        "03.12.0",
        "3.012.0",
        "3.12",
        "3.12.00",
        "3.12.0rc01",
        "3.12.0junk",
        312,
        [3, 12, 0],
    ],
)
def test_runtime_rejects_noncanonical_or_unsupported_versions(version: object) -> None:
    payload = _runtime_payload()
    payload["version"] = version
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "field,value",
    [
        ("operating_system", []),
        ("architecture", {}),
        ("pointer_bits", 64.0),
        ("py_debug", 0),
        ("gil_disabled", 1),
        ("soabi", ""),
        ("byteorder", "big"),
    ],
)
def test_runtime_rejects_malformed_platform_fields_without_type_error(
    field: str, value: object
) -> None:
    payload = _runtime_payload()
    payload[field] = value
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="platform/ABI"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "field,value",
    [
        ("linkage", []),
        ("unicodedata_linkage", {}),
        ("native_dependency_policy", "wrong-policy"),
        ("architecture", "wrong-arch"),
    ],
)
def test_runtime_capability_drift_is_not_hidden_by_resealing(
    field: str, value: object
) -> None:
    payload = _runtime_payload()
    payload["capabilities"][field] = value
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="capability vector"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize("node", [[], {}, "file-node-999"])
def test_runtime_rejects_malformed_native_node_references(node: object) -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["components"][0]["node"] = node
    _reseal(payload)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="native dependency component"
    ):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_native_role_must_reference_its_explicit_file_node() -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["components"][0]["node"] = "file-node-0"
    _reseal(payload)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="native dependency component"
    ):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "roles",
    [
        [[]],
        ["runtime-library", "runtime-library"],
        ["base-executable", "runtime-library"],
    ],
)
def test_runtime_rejects_malformed_or_duplicate_native_roles(roles: object) -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["components"][0]["roles"] = roles
    _reseal(payload)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="native dependency component"
    ):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_requires_dynamic_unicodedata_as_native_root() -> None:
    payload = _runtime_payload()
    dependency = payload["native_dependency_closure"]
    dependency["components"][2]["roles"] = []
    dependency["root_components"].pop()
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="native dependency roots"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_rejects_unreachable_native_component_with_valid_file_node() -> None:
    payload = _runtime_payload()
    dependency = payload["native_dependency_closure"]
    dependency["components"].append(
        {
            "id": "native-component-3",
            "filename": "z-orphan.so",
            "node": "file-node-0",
            "roles": [],
        }
    )
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="unreachable"):
        runtime.validate_python_runtime_identity(payload)
    # A graph edge alone cannot invent a loaded provider.
    dependency["edges"].insert(
        1, {"from": "native-component-1", "to": "native-component-3"}
    )
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="dependency edge"):
        runtime.validate_python_runtime_identity(payload)
    dependency["observed_components"].append("native-component-3")
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload


@pytest.mark.parametrize(
    "operating_system,contract",
    [
        ("windows", "windows-api-set:api-ms-win-core-file-l1-1-0.dll"),
        ("linux", "linux-loader-image:linux-vdso.so.1"),
        ("macos", "macos-dyld-cache-image:/usr/lib/libSystem.B.dylib"),
    ],
)
def test_runtime_contract_must_be_reachable_from_native_root(
    operating_system: str, contract: str
) -> None:
    payload = _runtime_payload(operating_system=operating_system)
    dependency = payload["native_dependency_closure"]
    dependency["contracts"] = [contract]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="unreachable"):
        runtime.validate_python_runtime_identity(payload)
    dependency["edges"].append({"from": "native-component-2", "to": contract})
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload


@pytest.mark.parametrize(
    "contract",
    [
        [],
        "windows-system-import:missing.dll",
        "linux-loader-image:linux-vdso.so.1",
        "windows-api-set:api-ms-missing.dll",
    ],
)
def test_runtime_rejects_malformed_cross_os_and_invented_contracts(
    contract: object,
) -> None:
    payload = _runtime_payload(operating_system="windows")
    payload["native_dependency_closure"]["contracts"] = [contract]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="native dependency"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "edge",
    [
        {"from": [], "to": "native-component-0"},
        {"from": "native-component-999", "to": "native-component-0"},
        {"from": "native-component-0", "to": "unknown-contract"},
    ],
)
def test_runtime_rejects_malformed_native_edges(edge: object) -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["edges"] = [edge]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="native dependency edge"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_rejects_duplicate_edges() -> None:
    payload = _runtime_payload()
    edges = payload["native_dependency_closure"]["edges"]
    edges.insert(0, dict(edges[0]))
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="native dependency edge"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize("field", ["observed_components", "observed_contracts"])
@pytest.mark.parametrize(
    "value", [None, [None], ["unknown"], ["native-component-0", "native-component-0"]]
)
def test_runtime_rejects_forged_observed_census(field: str, value: object) -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"][field] = value
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="observed census"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_observed_image_does_not_require_invented_import_edge() -> None:
    payload = _runtime_payload(operating_system="windows")
    dependency = payload["native_dependency_closure"]
    dependency["components"].append(
        {
            "id": "native-component-3",
            "filename": "z-optional.dll",
            "node": "file-node-0",
            "roles": [],
        }
    )
    dependency["observed_components"].append("native-component-3")
    dependency["deferred_imports"] = [
        {
            "from": "native-component-0",
            "name": "z-optional.dll",
            "kind": "delay",
        }
    ]
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload
    dependency["observed_components"].pop()
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="unreachable"):
        runtime.validate_python_runtime_identity(payload)


@pytest.mark.parametrize(
    "operating_system,kind",
    [("windows", "delay"), ("macos", "weak"), ("macos", "lazy")],
)
def test_runtime_records_unbound_optional_declarations(
    operating_system: str, kind: str
) -> None:
    payload = _runtime_payload(operating_system=operating_system)
    payload["native_dependency_closure"]["deferred_imports"] = [
        {
            "from": "native-component-0",
            "name": "unloaded-library",
            "kind": kind,
        }
    ]
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload


@pytest.mark.parametrize(
    "declaration",
    [
        {},
        {"from": [], "name": "x.dll", "kind": "delay"},
        {"from": "native-component-99", "name": "x.dll", "kind": "delay"},
        {"from": "native-component-0", "name": "X.dll", "kind": "delay"},
        {"from": "native-component-0", "name": "path/x.dll", "kind": "delay"},
        {"from": "native-component-0", "name": "x\0.dll", "kind": "delay"},
        {"from": "native-component-0", "name": "x.dll", "kind": "required"},
        {"from": "native-component-0", "name": "x.dll", "kind": "weak"},
        {
            "from": "native-component-0",
            "name": "x.dll",
            "kind": "delay",
            "to": "native-component-1",
        },
    ],
)
def test_runtime_rejects_resealed_malformed_or_falsely_bound_deferred_import(
    declaration: object,
) -> None:
    payload = _runtime_payload(operating_system="windows")
    payload["native_dependency_closure"]["deferred_imports"] = [declaration]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="deferred declaration"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_elf_needed_cannot_be_deferred() -> None:
    payload = _runtime_payload()
    payload["native_dependency_closure"]["deferred_imports"] = [
        {
            "from": "native-component-0",
            "name": "libc.so",
            "kind": "lazy",
        }
    ]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="deferred declaration"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_rejects_duplicate_deferred_imports() -> None:
    payload = _runtime_payload(operating_system="windows")
    declaration = {"from": "native-component-0", "name": "x.dll", "kind": "delay"}
    payload["native_dependency_closure"]["deferred_imports"] = [
        declaration,
        dict(declaration),
    ]
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="not canonical"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_rejects_orphaned_file_node_and_stale_digest() -> None:
    payload = _runtime_payload()
    payload["file_nodes"].append({"id": "file-node-3", "size": 1, "sha256": "4" * 64})
    with pytest.raises(PythonEnvironmentIdentityError, match="digest"):
        runtime.validate_python_runtime_identity(payload)
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError, match="unreferenced file nodes"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_rejects_stale_nested_native_digest_even_with_valid_outer_digest() -> (
    None
):
    payload = _runtime_payload()
    payload["native_dependency_closure"]["closure_sha256"] = "0" * 64
    payload["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in payload.items()
            if key != "runtime_closure_sha256"
        }
    )
    with pytest.raises(
        PythonEnvironmentIdentityError, match="native dependency closure"
    ):
        runtime.validate_python_runtime_identity(payload)
