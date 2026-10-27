"""Pure runtime-receipt validation; fixtures do not inspect the host Python."""

from __future__ import annotations

import json
import os
import sys
from types import FunctionType, SimpleNamespace
from typing import Any
from pathlib import Path

import pytest

from molt import python_runtime_identity as runtime
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from tests.process_guard_common import run_custody_subject_process


@pytest.mark.parametrize(
    "version,expected",
    [
        ("3.12.15 (main) [MSC v.1944 64 bit (AMD64)]", "x86_64"),
        ("3.13.0 (main) [MSC v.1944 64 bit (ARM64)]", "arm64"),
        ("3.14.0 (main) [MSC v.1944 32 bit (Intel)]", None),
        ("3.14.0 (main) [MSC v.1944 32 bit (ARM)]", None),
    ],
)
def test_windows_runtime_architecture_uses_cpython_build_without_host_query(
    monkeypatch: pytest.MonkeyPatch, version: str, expected: str | None
) -> None:
    # Execute this interpreter's real CPython sysconfig selection against the
    # independently specified Windows build banner; do not mirror its parser.
    observed_sys = SimpleNamespace(**vars(runtime.sys))
    observed_sys.platform = "win32"
    observed_sys.version = version
    actual_selector = runtime.sysconfig.get_platform
    globals_ = dict(actual_selector.__globals__)
    globals_.update(
        sys=observed_sys,
        os=SimpleNamespace(
            name="nt",
            environ={
                "PROCESSOR_ARCHITECTURE": "wrong-host",
                "PROCESSOR_ARCHITEW6432": "wrong-host",
                "_PYTHON_HOST_PLATFORM": "wrong-cross-target",
            },
        ),
    )
    selector = FunctionType(actual_selector.__code__, globals_)

    def forbidden_machine() -> str:
        pytest.fail("runtime identity queried the host through platform.machine/WMI")

    monkeypatch.setattr(runtime, "sys", observed_sys)
    monkeypatch.setattr(
        runtime,
        "sysconfig",
        SimpleNamespace(
            get_platform=selector,
            get_config_var=lambda name: {
                "SOABI": "cpython-fixture-windows",
                "Py_DEBUG": 0,
                "Py_GIL_DISABLED": 0,
            }.get(name),
        ),
    )
    monkeypatch.setattr(
        runtime,
        "platform",
        SimpleNamespace(
            machine=forbidden_machine,
            python_version=lambda: version.split()[0],
        ),
    )
    if expected is None:
        with pytest.raises(
            PythonEnvironmentIdentityError,
            match="unsupported provisioned-Python architecture",
        ):
            runtime._platform_identity()
    else:
        identity = runtime._platform_identity()
        assert identity["operating_system"] == "windows"
        assert identity["architecture"] == expected


@pytest.mark.parametrize(
    "component", ["site-packages", "dist-packages", "Site-Packages", "DIST-PACKAGES"]
)
def test_runtime_import_candidates_exclude_environment_distribution_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, component: str
) -> None:
    stdlib = tmp_path / "lib"
    distributions = stdlib / component
    nested = distributions / "editable"
    nested.mkdir(parents=True)
    archive = distributions / "extra.zip"
    archive.write_bytes(b"fixture; contents not read")
    absent = distributions / "python314.zip"
    monkeypatch.setattr(
        runtime.sys,
        "path",
        [str(stdlib), str(distributions), str(nested), str(archive), str(absent)],
    )
    directories, archives = runtime._runtime_import_candidates(
        tmp_path, _runtime_payload(version="3.14.4")
    )
    assert directories == [("import-directory-0", stdlib.resolve(), 0)]
    assert archives == []


@pytest.mark.parametrize("version", ["3.14.4", "3.15.0"])
@pytest.mark.parametrize(
    "operating_system,py_debug,gil_disabled,archive_names",
    [
        ("linux", False, False, ("python314.zip", "python315.zip")),
        ("linux", True, False, ("python314.zip", "python315.zip")),
        ("linux", False, True, ("python314t.zip", "python315t.zip")),
        ("linux", True, True, ("python314t.zip", "python315t.zip")),
        ("macos", False, False, ("python314.zip", "python315.zip")),
        ("macos", True, False, ("python314.zip", "python315.zip")),
        ("macos", False, True, ("python314t.zip", "python315t.zip")),
        ("macos", True, True, ("python314t.zip", "python315t.zip")),
        ("windows", False, False, ("python314.zip", "python315.zip")),
        ("windows", True, False, ("python314_d.zip", "python315_d.zip")),
        ("windows", False, True, ("python314.zip", "python315.zip")),
        ("windows", True, True, ("python314_d.zip", "python315_d.zip")),
    ],
)
def test_absent_stdlib_archive_capture_and_receipt_follow_cpython_abi(
    tmp_path,
    monkeypatch,
    version,
    operating_system,
    py_debug,
    gil_disabled,
    archive_names,
):
    # Literal CPython getpath landmarks, independent of Molt's spelling helper.
    filename = archive_names[0 if version == "3.14.4" else 1]
    payload = _runtime_payload(operating_system=operating_system, version=version)
    payload.update(py_debug=py_debug, gil_disabled=gil_disabled)
    payload["import_roots"].append({"kind": "absent-archive", "filename": filename})
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload

    # No host version/ABI fields are available: capture must use its already
    # selected metadata, as replay validation does for the received identity.
    selected_sys = SimpleNamespace(path=[str(tmp_path / filename)])
    monkeypatch.setattr(runtime, "sys", selected_sys)
    assert runtime._runtime_import_candidates(tmp_path, payload) == (
        [],
        [(filename, None, 0)],
    )
    wrong_names = (
        ("python314.zip", "python314t.zip", "python314_d.zip", "python314t_d.zip")
        if version == "3.14.4"
        else ("python315.zip", "python315t.zip", "python315_d.zip", "python315t_d.zip")
    )
    for wrong_name in (*wrong_names, "python999.zip", "unrelated.zip"):
        if wrong_name == filename:
            continue
        selected_sys.path = [str(tmp_path / wrong_name)]
        with pytest.raises(PythonEnvironmentIdentityError, match="import root"):
            runtime._runtime_import_candidates(tmp_path, payload)
        payload["import_roots"][-1]["filename"] = wrong_name
        _reseal(payload)
        with pytest.raises(PythonEnvironmentIdentityError, match="import-root"):
            runtime.validate_python_runtime_identity(payload)


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


def _add_external_runtime_symlink(
    payload: dict[str, Any], node: str, path: str
) -> None:
    root = payload["runtime_roots"][0]
    access = {"readable": True, "writable": False, "executable": True}
    existing_paths = {row["path"] for row in root["entries"]}
    parts = path.split("/")
    for index in range(1, len(parts)):
        parent = "/".join(parts[:index])
        if parent not in existing_paths:
            root["entries"].append(
                {"path": parent, "kind": "directory", "access": dict(access)}
            )
            existing_paths.add(parent)
    root["entries"].append(
        {
            "path": path,
            "kind": "symlink",
            "target_owner": "file-node",
            "node": node,
            "access": access,
        }
    )
    root["entries"].sort(key=lambda row: (row["path"].casefold(), row["path"]))
    root["file_count"] += 1
    if node in {row["id"] for row in payload["file_nodes"]}:
        root["node_ids"] = sorted(
            set(root["node_ids"]) | {node},
            key=lambda value: int(value.removeprefix("file-node-")),
        )
    root["manifest_sha256"] = canonical_json_sha256(root["entries"])
    _reseal(payload)


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


def test_platform_identity_makes_no_native_load_or_process_requests() -> None:
    """Attribute native imports, library loads and spawns to the identity call.

    A process image census also observes external DLL injection and cannot
    establish which loads this Python call requested. The Windows architecture
    selector tests separately forbid the cached platform.machine/WMI path.
    """

    program = (
        "import json, sys\n"
        "from molt import python_runtime_identity as runtime\n"
        "from importlib.machinery import EXTENSION_SUFFIXES\n"
        "native_suffixes = tuple(suffix.casefold() for suffix in EXTENSION_SUFFIXES)\n"
        "requests = []\n"
        "observing = True\n"
        "def audit(event, args):\n"
        "    if not observing:\n"
        "        return\n"
        "    if event == 'import':\n"
        "        filename = args[1]\n"
        "        if isinstance(filename, str) and filename.casefold().endswith(native_suffixes):\n"
        "            requests.append([event, args[0], filename])\n"
        "    elif event in {'ctypes.dlopen', 'subprocess.Popen'}:\n"
        "        requests.append([event, str(args[0])])\n"
        "sys.addaudithook(audit)\n"
        "try:\n"
        "    runtime._platform_identity()\n"
        "finally:\n"
        "    observing = False\n"
        "print(json.dumps(requests))\n"
    )
    from molt.python_environment_identity import (
        PythonIdentityVerifier,
        python_identity_verifier_arguments,
    )

    with PythonIdentityVerifier() as verifier:
        completed = verifier.run(
            [
                sys.executable,
                *python_identity_verifier_arguments(verifier.prefix),
                "-c",
                f"import sys; sys.path[:0] = [{str(Path(runtime.__file__).parents[1])!r}]\n{program}",
            ],
            memory_guard_prefix="MOLT_PYTEST",
            timeout=60.0,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        assert not list(verifier.prefix.iterdir())

    assert completed.returncode == 0, completed.stderr
    assert json.loads(completed.stdout) == []


def _selection(paths: list[str], **native: object) -> dict[str, object]:
    return {
        "schema": runtime.PYTHON_RUNTIME_SELECTION_SCHEMA,
        "platform": {"operating_system": "windows"},
        "import_paths": [{"lexical": "C:/py/Lib"}],
        "native": {"executable": "C:/py/python.exe", "paths": paths, **native},
    }


def test_startup_selections_agree_when_images_only_load_or_unload() -> None:
    """HF-169: a census gains kernel.appcore.dll on first use and loses the
    WMI providers COM frees; neither changes how the loader selected."""

    before = _selection(["C:/py/python312.dll", "C:/Windows/System32/wbemprox.dll"])
    after = _selection(
        ["C:/py/python312.dll", "C:/Windows/System32/kernel.appcore.dll"],
        aliases={"kernel.appcore.dll": "C:/Windows/System32/kernel.appcore.dll"},
    )

    assert (
        runtime.startup_selection_difference(before, after, admit_new_images=True) == []
    )


def test_fresh_launch_selection_refuses_an_image_the_admitted_process_lacks() -> None:
    admitted = _selection(["C:/py/python312.dll", "C:/Windows/System32/x.dll"])
    fresh = _selection(["C:/py/python312.dll", "C:/py/injected.dll"])

    assert runtime.startup_selection_difference(
        admitted, fresh, admit_new_images=False
    ) == ["native image injected.dll gained: C:/py/injected.dll"]
    assert (
        runtime.startup_selection_difference(admitted, fresh, admit_new_images=True)
        == []
    )


def test_startup_selections_disagree_when_a_loaded_name_resolves_elsewhere() -> None:
    before = _selection(["C:/py/python312.dll", "C:/py/VCRUNTIME140.dll"])
    after = _selection(["C:/py/python312.dll", "C:/evil/vcruntime140.dll"])
    after["import_paths"] = [{"lexical": "C:/other"}]
    after["native"]["executable"] = "C:/py2/python.exe"  # type: ignore[index]

    assert runtime.startup_selection_difference(
        before, after, admit_new_images=True
    ) == [
        "import_paths",
        "native executable C:/py/python.exe -> C:/py2/python.exe",
        "native image vcruntime140.dll: C:/py/VCRUNTIME140.dll -> "
        "C:/evil/vcruntime140.dll",
    ]


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


def test_macos_framework_runtime_receipt_references_attested_runtime_library() -> None:
    payload = _runtime_payload(operating_system="macos")
    _add_external_runtime_symlink(
        payload,
        "file-node-1",
        "lib/python3.12/config-3.12-darwin/libpython3.12.a",
    )

    validated = runtime.validate_python_runtime_identity(payload)
    row = next(
        entry
        for entry in validated["runtime_roots"][0]["entries"]
        if entry["path"] == "lib/python3.12/config-3.12-darwin/libpython3.12.a"
    )
    assert row["target_owner"] == "file-node"
    assert row["node"] == "file-node-1"
    assert runtime.runtime_explicit_file_content(validated, "runtime-library") == {
        "filename": "libpython.so",
        "size": 2,
        "sha256": "2" * 64,
    }


@pytest.mark.parametrize("node", ["file-node-0", "file-node-1", "file-node-2"])
def test_runtime_receipt_external_alias_reuses_existing_file_node(
    node: str,
) -> None:
    payload = _runtime_payload()
    _add_external_runtime_symlink(payload, node, f"links/{node}")

    assert runtime.validate_python_runtime_identity(payload) == payload


@pytest.mark.parametrize(
    "role", ["framework/Python", "base-dlls", "runtime-library-alias"]
)
def test_runtime_receipt_rejects_role_spelling_as_external_node(role: str) -> None:
    payload = _runtime_payload(operating_system="macos")
    _add_external_runtime_symlink(payload, role, "lib/python3.12/config/link")

    with pytest.raises(PythonEnvironmentIdentityError, match="symlink entry"):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_external_alias_retains_independent_explicit_file_attestation() -> None:
    payload = _runtime_payload(operating_system="macos")
    _add_external_runtime_symlink(
        payload,
        "file-node-1",
        "lib/python3.12/config-3.12-darwin/libpython3.12.a",
    )
    # An alias node cannot substitute for the independently attested loaded role.
    payload["file_nodes"].append({"id": "file-node-3", "size": 4, "sha256": "4" * 64})
    payload["explicit_files"][1]["node"] = "file-node-3"
    _reseal(payload)

    with pytest.raises(
        PythonEnvironmentIdentityError, match="native dependency component"
    ):
        runtime.validate_python_runtime_identity(payload)


def test_runtime_external_alias_does_not_infer_role_from_node_identity() -> None:
    payload = _runtime_payload(operating_system="macos")
    _add_external_runtime_symlink(
        payload,
        "file-node-1",
        "lib/python3.12/config-3.12-darwin/libpython3.12.a",
    )
    payload["explicit_files"][1]["node"] = "file-node-0"
    payload["native_dependency_closure"]["components"][0]["node"] = "file-node-0"
    _reseal(payload)

    # The alias owns node 1 independently; loaded-role coincidence is not its owner.
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


def test_runtime_deferred_import_order_is_component_numeric_then_name() -> None:
    payload = _runtime_payload(operating_system="macos")
    dependency = payload["native_dependency_closure"]
    dependency["deferred_imports"] = [
        {
            "from": "native-component-0",
            "name": "a-future.dylib",
            "kind": "lazy",
        },
        {
            "from": "native-component-0",
            "name": "a-future.dylib",
            "kind": "weak",
        },
        {
            "from": "native-component-0",
            "name": "z-future.dylib",
            "kind": "weak",
        },
        {
            "from": "native-component-1",
            "name": "a-future.dylib",
            "kind": "weak",
        },
    ]
    _reseal(payload)
    assert runtime.validate_python_runtime_identity(payload) == payload

    dependency["deferred_imports"].reverse()
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


def test_mapped_file_lookup_names_the_file_backing_an_address() -> None:
    maps = (
        "55d4c0a00000-55d4c0a2c000 r--p 00000000 08:01 393 /opt/py/bin/python3.12\n"
        "55d4c0a2c000-55d4c0d10000 r-xp 0002c000 08:01 393 /opt/py/bin/python3.12\n"
        "55d4c1f3e000-55d4c2005000 rw-p 00000000 00:00 0 [heap]\n"
        "7f1e2a000000-7f1e2a021000 r-xp 00000000 08:01 77 /opt/a b/libx.so\n"
    )
    assert runtime._mapped_file_at(0x55D4C0A2D000, maps) == Path(
        "/opt/py/bin/python3.12"
    )
    assert runtime._mapped_file_at(0x7F1E2A000010, maps) == Path("/opt/a b/libx.so")
    assert runtime._mapped_file_at(0x55D4C1F3E100, maps) is None
    assert runtime._mapped_file_at(0x10, maps) is None


def test_runtime_library_does_not_depend_on_the_invocation_name(tmp_path) -> None:
    # A PATH launch names the interpreter "python3" in argv[0]; glibc reports
    # a statically linked runtime by that name, which the cwd cannot resolve.
    probe = (
        "import json, sys; from molt import python_runtime_identity as r; "
        "library = r._runtime_library(); "
        "print(json.dumps({'library': None if library is None else str(library), "
        "'prefix': sys.prefix, 'executable': sys.executable}))"
    )
    # CPython Modules/getpath.py resolves a bare argv[0] against PATH on
    # POSIX. Bind that selector to this installation as well as execve's image;
    # otherwise standalone Python can select a different prefix before import.
    result = run_custody_subject_process(
        [Path(sys.executable).name, "-c", probe],
        executable=sys.executable,
        env={**os.environ, "PATH": str(Path(sys.executable).parent)},
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    observed = json.loads(result.stdout)
    assert Path(observed["prefix"]).resolve() == Path(sys.prefix).resolve()
    assert Path(observed["executable"]).resolve() == Path(sys.executable).resolve()
    library = runtime._runtime_library()
    expected = None if library is None else str(library)
    assert observed["library"] == expected


@pytest.mark.parametrize("enable_shared", ["0", "1"])
def test_runtime_linkage_follows_the_loaded_image_not_build_configuration(
    monkeypatch: pytest.MonkeyPatch, enable_shared: str
) -> None:
    # python-build-standalone configures --enable-shared (shipping a libpython
    # for embedders) yet links its own executable statically: the runtime
    # symbol lives in the executable, so the linkage is static.
    config = {"Py_ENABLE_SHARED": enable_shared, "PYTHONFRAMEWORK": ""}
    monkeypatch.setattr(runtime.sysconfig, "get_config_var", config.get)
    assert runtime._runtime_linkage(None) == "static"
    assert runtime._runtime_linkage(Path("/opt/py/lib/libpython3.12.so.1.0")) == (
        "shared"
    )


def test_framework_linkage_stays_framework(monkeypatch: pytest.MonkeyPatch) -> None:
    config = {"Py_ENABLE_SHARED": "0", "PYTHONFRAMEWORK": "Python"}
    monkeypatch.setattr(runtime.sysconfig, "get_config_var", config.get)
    assert runtime._runtime_linkage(None) == "framework"


@pytest.fixture
def external_alias_installation(tmp_path, monkeypatch):
    """Finite static ELF runtime with independently known external input bytes."""
    from molt import python_native_dependency_custody as native
    from molt import python_native_locations as locations
    from molt.python_file_node_custody import PythonFileCaptureContext
    from tests.native_artifact_fixtures import elf_header

    prefix = tmp_path / "installation"
    stdlib = prefix / "lib" / "python3.14"
    config_dir = stdlib / "config-3.14-aarch64-linux-gnu"
    config_dir.mkdir(parents=True)
    dynload = stdlib / "lib-dynload"
    dynload.mkdir()
    executable = prefix / "bin" / "python3.14"
    executable.parent.mkdir()
    executable.write_bytes(elf_header(machine=183, kind=2))
    library_dir = tmp_path / "multiarch"
    library_dir.mkdir()
    library = library_dir / "libpython3.14.so.1.0"
    library_bytes = b"independent configured embedding library fixture"
    library.write_bytes(library_bytes)
    alias = config_dir / "libpython3.14.so"
    try:
        alias.symlink_to(library)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    config = {
        "LIBDIR": str(library_dir),
        "INSTSONAME": library.name,
        "LDLIBRARY": alias.name,
        "LIBPL": str(config_dir),
        "Py_ENABLE_SHARED": 1,
    }
    proxy = SimpleNamespace(**vars(runtime.sys))
    proxy.executable = str(executable)
    proxy._base_executable = str(executable)
    proxy.prefix = proxy.base_prefix = str(prefix)
    proxy.path = [str(stdlib)]
    monkeypatch.setattr(runtime, "sys", proxy)
    monkeypatch.setattr(
        runtime, "sysconfig", SimpleNamespace(get_config_var=config.get)
    )
    monkeypatch.setattr(runtime, "unicodedata", SimpleNamespace())
    monkeypatch.setattr(runtime, "_runtime_library", lambda: None)
    platform = _runtime_payload(architecture="arm64", version="3.14.4")
    fields = (
        "implementation",
        "version",
        "cache_tag",
        "soabi",
        "abi_flags",
        "multiarch",
        "py_debug",
        "gil_disabled",
        "operating_system",
        "architecture",
        "pointer_bits",
        "byteorder",
    )
    monkeypatch.setattr(
        runtime, "_platform_identity", lambda: {key: platform[key] for key in fields}
    )
    monkeypatch.setattr(
        runtime,
        "_base_runtime_paths",
        lambda: {
            "stdlib": stdlib,
            "platstdlib": stdlib,
            "base-lib-dynload": dynload,
        },
    )
    snapshot = locations.LoadedNativeModuleSnapshot(
        executable=executable,
        paths=(executable,),
        aliases={},
        contracts=(),
        macho_identities={},
    )
    monkeypatch.setattr(
        locations, "_loaded_native_module_snapshot", lambda _os: snapshot
    )
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)
    with PythonFileCaptureContext() as context:
        yield SimpleNamespace(
            context=context,
            stdlib=stdlib,
            executable=executable,
            library=library,
            alias=alias,
            config=config,
            library_bytes=library_bytes,
            directory=library_dir,
        )


def test_runtime_external_alias_retarget_during_snapshot_precedes_content_capture(
    external_alias_installation, monkeypatch
):
    from molt import python_file_node_custody as files

    fixture = external_alias_installation
    other = fixture.directory / "other-provider"
    other.write_bytes(b"unadmitted provider")
    snapshot = files._external_alias_chain_snapshot

    def retarget(path, *, label):
        captured = snapshot(path, label=label)
        if path == fixture.alias:
            path.unlink()
            path.symlink_to(other)
        return captured

    monkeypatch.setattr(files, "_external_alias_chain_snapshot", retarget)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="changed during traversal"
    ):
        runtime.capture_current_python_runtime(capture_context=fixture.context)
    # Only the admitted executable was read, never either external provider.
    assert fixture.context.inventory_profile()["hashed_files"] == 1
    assert fixture.context.inventory_profile()["hashed_bytes"] == (
        fixture.executable.stat().st_size
    )


def test_runtime_external_aliases_own_exact_nodes_without_loaded_roles(
    external_alias_installation,
):
    import hashlib

    fixture = external_alias_installation
    site = fixture.directory / "sitecustomize.py"
    site_bytes = b"known distro customization input\n"
    site.write_bytes(site_bytes)
    (fixture.stdlib / "sitecustomize.py").symlink_to(site)
    (fixture.stdlib / "library-alias").symlink_to(fixture.library)
    payload = runtime.capture_current_python_runtime(capture_context=fixture.context)
    runtime.validate_python_runtime_identity(payload)
    assert payload["capabilities"]["linkage"] == "static"
    assert payload["capabilities"]["required_explicit_roles"] == ["base-executable"]
    closure = payload["native_dependency_closure"]
    assert len(closure["components"]) == 1
    assert closure["components"][0]["roles"] == ["base-executable"]
    assert closure["root_components"] == closure["observed_components"]
    assert closure["edges"] == []
    roots = payload["runtime_roots"]
    rows = {row["path"]: row for root in roots for row in root["entries"]}
    librow = rows["config-3.14-aarch64-linux-gnu/libpython3.14.so"]
    assert librow["target_owner"] == "file-node"
    assert rows["library-alias"]["node"] == librow["node"]
    nodes = {node["id"]: node for node in payload["file_nodes"]}
    for row, data in (
        (librow, fixture.library_bytes),
        (rows["sitecustomize.py"], site_bytes),
    ):
        assert nodes[row["node"]]["sha256"] == hashlib.sha256(data).hexdigest()
        assert nodes[row["node"]]["size"] == len(data)
    assert len(nodes) == 3  # Executable plus two distinct external file objects.
    assert fixture.context.inventory_profile()["hashed_files"] == 3
    assert fixture.context.inventory_profile()["hashed_bytes"] == (
        fixture.executable.stat().st_size + len(fixture.library_bytes) + len(site_bytes)
    )
    fixture.context.verify()


@pytest.mark.parametrize(
    "mutation", ["bytes", "alias", "terminal", "intermediate", "parent"]
)
def test_runtime_external_alias_retains_content_and_resolution(
    external_alias_installation,
    mutation,
):
    fixture = external_alias_installation
    intermediate = fixture.directory / "selected.so"
    intermediate.symlink_to(fixture.library)
    fixture.alias.unlink()
    fixture.alias.symlink_to(intermediate)
    if mutation == "parent":
        selected_directory = fixture.directory / "selected-directory"
        selected_directory.symlink_to(fixture.directory, target_is_directory=True)
        fixture.alias.unlink()
        fixture.alias.symlink_to(selected_directory / fixture.library.name)
    runtime.capture_current_python_runtime(capture_context=fixture.context)
    if mutation == "bytes":
        fixture.library.write_bytes(b"changed input bytes")
    else:
        other = fixture.directory / "replacement"
        if mutation == "parent":
            other.mkdir()
            (other / fixture.library.name).write_bytes(fixture.library_bytes)
            selected = selected_directory
        else:
            other.write_bytes(fixture.library_bytes)
            selected = {
                "alias": fixture.alias,
                "terminal": fixture.library,
                "intermediate": intermediate,
            }[mutation]
        selected.unlink()
        selected.symlink_to(other, target_is_directory=mutation == "parent")
    with pytest.raises((PythonEnvironmentIdentityError, ValueError)):
        fixture.context.verify()


def test_unreached_configured_library_is_not_an_input(external_alias_installation):
    fixture = external_alias_installation
    fixture.alias.unlink()
    payload = runtime.capture_current_python_runtime(capture_context=fixture.context)
    runtime.validate_python_runtime_identity(payload)
    assert len(payload["file_nodes"]) == 1
    assert fixture.context.inventory_profile()["hashed_files"] == 1
    fixture.library.write_bytes(b"unreached configured library is irrelevant")
    fixture.config["INSTSONAME"] = "other-unreached-library.so"
    fixture.context.verify()


@pytest.mark.parametrize("target", ["directory", "broken", "cycle", "fifo"])
def test_runtime_external_alias_rejects_nonregular_targets(
    external_alias_installation, target
):
    import os

    fixture = external_alias_installation
    fixture.library.unlink()
    if target == "directory":
        fixture.library.mkdir()
    elif target == "cycle":
        fixture.library.symlink_to(fixture.alias)
    elif target == "fifo":
        if not hasattr(os, "mkfifo"):
            pytest.skip("FIFO creation unavailable")
        os.mkfifo(fixture.library)
    with pytest.raises((PythonEnvironmentIdentityError, OSError, RuntimeError)):
        runtime.capture_current_python_runtime(capture_context=fixture.context)


@pytest.mark.parametrize(
    "fault",
    [
        "missing-node",
        "unknown-node",
        "root-node-omitted",
        "extra-field",
        "old-role",
        "orphan-node",
    ],
)
def test_runtime_external_alias_receipt_requires_closed_content_nodes(
    external_alias_installation, fault
):
    fixture = external_alias_installation
    payload = runtime.capture_current_python_runtime(capture_context=fixture.context)
    root = payload["runtime_roots"][0]
    alias = next(row for row in root["entries"] if row["kind"] == "symlink")
    if fault == "missing-node":
        payload["file_nodes"].pop()
    elif fault == "unknown-node":
        alias["node"] = "file-node-999"
    elif fault == "root-node-omitted":
        root["node_ids"] = []
    elif fault == "extra-field":
        alias["target"] = "/unowned/absolute/path"
    elif fault == "old-role":
        del alias["node"]
        alias.update(target_owner="base-runtime", target_role="base-executable")
    else:
        payload["file_nodes"].append(
            {
                "id": f"file-node-{len(payload['file_nodes'])}",
                "size": 3,
                "sha256": "1" * 64,
            }
        )
    root["manifest_sha256"] = canonical_json_sha256(root["entries"])
    _reseal(payload)
    with pytest.raises(PythonEnvironmentIdentityError):
        runtime.validate_python_runtime_identity(payload)


def test_external_file_nodes_are_not_admitted_in_environment_inventory(
    external_alias_installation,
):
    from molt import python_file_node_custody as files

    fixture = external_alias_installation
    payload = runtime.capture_current_python_runtime(capture_context=fixture.context)
    nodes = {row["id"]: row for row in payload["file_nodes"]}
    with pytest.raises(PythonEnvironmentIdentityError, match="symlink entry"):
        files._validate_inventory_entries(
            payload["runtime_roots"][0]["entries"], label="environment", nodes=nodes
        )
    with pytest.raises(PythonEnvironmentIdentityError, match="symlink escapes custody"):
        files._stable_tree_inventory(
            fixture.stdlib,
            root_id="environment",
            label="environment",
            pool=files._FileNodePool(),
        )


@pytest.mark.parametrize("indirection", ["file", "parent"])
def test_runtime_external_intermediate_restore_is_detected(
    external_alias_installation, indirection
):
    fixture = external_alias_installation
    link = fixture.directory / "selected"
    original = fixture.library if indirection == "file" else fixture.directory
    link.symlink_to(original, target_is_directory=indirection == "parent")
    fixture.alias.unlink()
    fixture.alias.symlink_to(
        link if indirection == "file" else link / fixture.library.name
    )
    runtime.capture_current_python_runtime(capture_context=fixture.context)
    changed = fixture.directory / "other"
    if indirection == "file":
        changed.write_bytes(b"transient different provider")
    else:
        changed.mkdir()
        (changed / fixture.library.name).write_bytes(b"transient different provider")
    link.unlink()
    link.symlink_to(changed, target_is_directory=indirection == "parent")
    assert fixture.alias.read_bytes() == b"transient different provider"
    link.unlink()
    link.symlink_to(original, target_is_directory=indirection == "parent")
    assert fixture.alias.resolve() == fixture.library.resolve()
    with pytest.raises(
        PythonEnvironmentIdentityError, match="external alias topology changed"
    ):
        fixture.context.verify()


def test_runtime_external_chain_expansion_is_bounded(external_alias_installation):
    from molt import python_file_node_custody as files

    fixture = external_alias_installation
    target = fixture.library
    for number in range(257):
        link = fixture.directory / f"link-{number}"
        link.symlink_to(target)
        target = link
    with pytest.raises(PythonEnvironmentIdentityError, match="link expansion limit"):
        files._external_alias_chain_snapshot(target, label="fixture")


def test_runtime_external_relative_alias_chain_is_owned(external_alias_installation):
    import os

    fixture = external_alias_installation
    intermediate = fixture.directory / "selected.so"
    intermediate.symlink_to(fixture.library.name)
    fixture.alias.unlink()
    fixture.alias.symlink_to(os.path.relpath(intermediate, fixture.alias.parent))
    payload = runtime.capture_current_python_runtime(capture_context=fixture.context)
    runtime.validate_python_runtime_identity(payload)
    assert len(payload["file_nodes"]) == 2
    fixture.context.verify()
