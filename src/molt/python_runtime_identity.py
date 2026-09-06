"""CPython runtime capability, root, import, and ABI identity authority."""

from __future__ import annotations

import os
import platform
import re
import sys
import sysconfig
import unicodedata
from collections.abc import Mapping
from pathlib import Path
from typing import cast

from molt.exact_json import canonical_json_bytes, canonical_json_sha256
from molt.python_file_node_custody import (
    PythonFileCaptureContext,
    _FileNodePool,
    _is_file_entry,
    _is_junction,
    _relative_path,
    _root_forest,
    _stable_tree_inventory,
    _validate_file_nodes,
    _validate_inventory_entries,
)
from molt.python_identity_common import (
    identity_validator,
    PythonEnvironmentIdentityError,
    _valid_relative_payload_path,
    _valid_sha256,
)
from molt.python_native_dependency_custody import (
    DEFERRED_DEPENDENCY_KINDS,
    _native_dependency_closure,
)
from molt.python_native_locations import _native_contract_valid

PYTHON_RUNTIME_IDENTITY_SCHEMA = "molt.python-runtime-closure.v4"
PYTHON_RUNTIME_CAPABILITY_SCHEMA = "molt.cpython-runtime-capabilities.v1"
_NATIVE_DEPENDENCY_POLICIES = {
    "windows": "pe-loaded-import-closure-v2",
    "macos": "mach-o-loaded-dylib-closure-v2",
    "linux": "elf-loaded-needed-closure-v2",
}
_RUNTIME_IDENTITY_FIELDS = frozenset(
    {
        "schema",
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
        "capabilities",
        "file_nodes",
        "native_dependency_closure",
        "explicit_files",
        "import_roots",
        "runtime_root_roles",
        "runtime_roots",
        "runtime_closure_sha256",
    }
)


def _platform_identity() -> dict[str, object]:
    if sys.implementation.name != "cpython" or sys.version_info < (3, 12):
        raise PythonEnvironmentIdentityError(
            "provisioned Python must be CPython 3.12 or newer"
        )
    operating_system = {
        "win32": "windows",
        "darwin": "macos",
        "linux": "linux",
    }.get(sys.platform)
    if operating_system is None:
        raise PythonEnvironmentIdentityError(
            f"unsupported provisioned-Python operating system: {sys.platform}"
        )
    raw_architecture = platform.machine().strip().casefold()
    architecture = {
        "amd64": "x86_64",
        "x86_64": "x86_64",
        "arm64": "arm64",
        "aarch64": "arm64",
    }.get(raw_architecture)
    if architecture is None:
        raise PythonEnvironmentIdentityError(
            f"unsupported provisioned-Python architecture: {raw_architecture or '<empty>'}"
        )

    def config_bool(name: str) -> bool:
        return str(sysconfig.get_config_var(name) or "0").strip().casefold() not in {
            "",
            "0",
            "false",
            "none",
        }

    soabi = str(sysconfig.get_config_var("SOABI") or "").strip()
    if not soabi:
        extension_suffix = str(sysconfig.get_config_var("EXT_SUFFIX") or "").strip()
        extension_kind = Path(extension_suffix).suffix
        if (
            not extension_suffix.startswith(".")
            or not extension_kind
            or len(extension_suffix) <= len(extension_kind) + 1
        ):
            raise PythonEnvironmentIdentityError(
                "provisioned Python exposes neither SOABI nor an ABI-tagged extension suffix"
            )
        soabi = extension_suffix[1 : -len(extension_kind)]

    return {
        "implementation": "cpython",
        "version": platform.python_version(),
        "cache_tag": str(sys.implementation.cache_tag or ""),
        "soabi": soabi,
        "abi_flags": str(getattr(sys, "abiflags", "")),
        "multiarch": str(getattr(sys.implementation, "_multiarch", "")),
        "py_debug": config_bool("Py_DEBUG"),
        "gil_disabled": config_bool("Py_GIL_DISABLED"),
        "operating_system": operating_system,
        "architecture": architecture,
        "pointer_bits": 64 if sys.maxsize > 2**32 else 32,
        "byteorder": sys.byteorder,
    }


def _runtime_library() -> Path | None:
    try:
        import ctypes

        if os.name == "nt":
            buffer = ctypes.create_unicode_buffer(32768)
            get_name = ctypes.windll.kernel32.GetModuleFileNameW  # type: ignore[attr-defined]
            get_name.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_uint32)
            get_name.restype = ctypes.c_uint32
            length = get_name(
                ctypes.c_void_p(ctypes.pythonapi._handle), buffer, len(buffer)
            )
            if 0 < length < len(buffer):
                return Path(buffer.value).resolve(strict=True)
            raise PythonEnvironmentIdentityError(
                "cannot identify the loaded CPython runtime library"
            )

        class _DlInfo(ctypes.Structure):
            _fields_ = [
                ("filename", ctypes.c_char_p),
                ("base", ctypes.c_void_p),
                ("symbol", ctypes.c_char_p),
                ("symbol_address", ctypes.c_void_p),
            ]

        process = ctypes.CDLL(None)
        dladdr = process.dladdr
        dladdr.argtypes = (ctypes.c_void_p, ctypes.POINTER(_DlInfo))
        dladdr.restype = ctypes.c_int
        info = _DlInfo()
        symbol = ctypes.cast(ctypes.pythonapi.Py_GetVersion, ctypes.c_void_p)
        if not dladdr(symbol, ctypes.byref(info)) or not info.filename:
            raise PythonEnvironmentIdentityError(
                "loader cannot identify the CPython runtime symbol owner"
            )
        candidate = Path(os.fsdecode(info.filename)).resolve(strict=True)
        base_executable = Path(
            getattr(sys, "_base_executable", None) or sys.executable
        ).resolve(strict=True)
        if not candidate.samefile(base_executable):
            return candidate
    except (AttributeError, OSError, TypeError, ValueError) as exc:
        raise PythonEnvironmentIdentityError(
            f"cannot inspect the loaded CPython runtime library: {exc}"
        ) from exc
    if (
        os.name == "nt"
        or sysconfig.get_config_var("Py_ENABLE_SHARED")
        or sysconfig.get_config_var("PYTHONFRAMEWORK")
    ):
        raise PythonEnvironmentIdentityError(
            "cannot identify the loaded CPython runtime library"
        )
    return None


def _runtime_linkage(runtime_library: Path | None) -> str:
    if sysconfig.get_config_var("PYTHONFRAMEWORK"):
        return "framework"
    if runtime_library is not None:
        return "shared"
    if sysconfig.get_config_var("Py_ENABLE_SHARED"):
        raise PythonEnvironmentIdentityError(
            "CPython reports shared linkage but its loaded runtime library is unavailable"
        )
    return "static"


def _required_runtime_roles(
    platform_identity: Mapping[str, object],
    *,
    linkage: str,
    unicodedata_dynamic: bool,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    operating_system = str(platform_identity["operating_system"])
    root_roles = ["platstdlib", "stdlib"]
    if operating_system == "windows":
        root_roles.append("base-dlls")
    else:
        root_roles.append("base-lib-dynload")
    explicit_roles = ["base-executable"]
    if linkage in {"shared", "framework"}:
        explicit_roles.append("runtime-library")
    if unicodedata_dynamic:
        explicit_roles.append("unicodedata")
    return tuple(sorted(root_roles)), tuple(sorted(explicit_roles))


def _runtime_capabilities(
    platform_identity: Mapping[str, object],
    *,
    linkage: str,
    unicodedata_dynamic: bool,
) -> dict[str, object]:
    root_roles, explicit_roles = _required_runtime_roles(
        platform_identity,
        linkage=linkage,
        unicodedata_dynamic=unicodedata_dynamic,
    )
    operating_system = str(platform_identity["operating_system"])
    dependency_policy = _NATIVE_DEPENDENCY_POLICIES[operating_system]
    version = str(platform_identity["version"])
    return {
        "schema": PYTHON_RUNTIME_CAPABILITY_SCHEMA,
        "implementation_policy": "cpython>=3.12",
        "version_series": ".".join(version.split(".")[:2]),
        "operating_system": operating_system,
        "architecture": platform_identity["architecture"],
        "linkage": linkage,
        "unicodedata_linkage": "dynamic" if unicodedata_dynamic else "built-in",
        "scanner_policy": "no-follow-handle-two-snapshot-sha256-v1",
        "import_root_policy": "isolated-active-prefix-roots-v1",
        "native_dependency_policy": dependency_policy,
        "required_root_roles": list(root_roles),
        "required_explicit_roles": list(explicit_roles),
    }


def _base_runtime_paths() -> dict[str, Path]:
    base = str(Path(sys.base_prefix).resolve(strict=True))
    paths = sysconfig.get_paths(
        vars={
            "base": base,
            "platbase": base,
            "installed_base": base,
            "installed_platbase": base,
        }
    )
    result = {
        "stdlib": Path(paths["stdlib"]),
        "platstdlib": Path(paths["platstdlib"]),
    }
    if sys.platform == "win32":
        result["base-dlls"] = Path(base) / "DLLs"
    else:
        # CPython owns the configured ABI/platform layout, including lib64 and
        # free-threaded pythonX.Yt directories. Do not reconstruct it from version.
        dynload = sysconfig.get_config_var("DESTSHARED")
        if not isinstance(dynload, str) or not dynload:
            raise PythonEnvironmentIdentityError(
                "CPython does not expose its configured dynamic-extension root"
            )
        result["base-lib-dynload"] = Path(dynload)
    return result


def _runtime_import_candidates(
    base_prefix: Path,
) -> tuple[list[tuple[str, Path, int]], list[tuple[str, Path | None, int]]]:
    directories: list[tuple[str, Path, int]] = []
    archives: list[tuple[str, Path | None, int]] = []
    seen_directories: set[str] = set()
    seen_archives: set[str] = set()
    expected_archive = re.compile(
        rf"python{sys.version_info.major}{sys.version_info.minor}\.zip$", re.I
    )
    for position, raw in enumerate(sys.path):
        if not raw:
            continue
        lexical = Path(os.path.abspath(raw))
        if lexical.is_dir():
            resolved = lexical.resolve(strict=True)
            if not resolved.is_relative_to(base_prefix):
                continue
            key = os.path.normcase(str(resolved))
            if key not in seen_directories:
                seen_directories.add(key)
                directories.append((f"import-directory-{position}", resolved, position))
            continue
        if lexical.is_file() and lexical.suffix.casefold() in {".zip", ".pyz"}:
            resolved = lexical.resolve(strict=True)
            if not resolved.is_relative_to(base_prefix):
                continue
            key = os.path.normcase(str(resolved))
            if key not in seen_archives:
                seen_archives.add(key)
                archives.append((lexical.name, resolved, position))
            continue
        if not lexical.exists() and expected_archive.fullmatch(lexical.name):
            try:
                parent = lexical.parent.resolve(strict=True)
            except OSError:
                continue
            if not parent.is_relative_to(base_prefix):
                continue
            key = lexical.name.casefold()
            if key not in seen_archives:
                seen_archives.add(key)
                archives.append((lexical.name, None, position))
            continue
        raise PythonEnvironmentIdentityError(
            f"base CPython import root is neither a directory nor a supported archive: {lexical}"
        )
    return directories, archives


def _capture_runtime_with_context(
    *, capture_context: PythonFileCaptureContext | None = None
) -> tuple[
    dict[str, object],
    tuple[tuple[str, Path], ...],
    _FileNodePool,
    dict[str, Path],
]:
    platform_payload = _platform_identity()
    base_prefix = Path(sys.base_prefix).resolve(strict=True)
    base_executable = Path(
        getattr(sys, "_base_executable", None) or sys.executable
    ).resolve(strict=True)
    runtime_library = _runtime_library()
    linkage = _runtime_linkage(runtime_library)
    unicode_file = getattr(unicodedata, "__file__", None)
    unicode_path = Path(unicode_file).resolve(strict=True) if unicode_file else None
    unicodedata_dynamic = unicode_path is not None
    capabilities = _runtime_capabilities(
        platform_payload,
        linkage=linkage,
        unicodedata_dynamic=unicodedata_dynamic,
    )
    required_root_roles = cast(list[str], capabilities["required_root_roles"])
    required_root_role_set = set(required_root_roles)
    base_paths = _base_runtime_paths()
    root_candidates: list[tuple[str, Path]] = []
    for role in required_root_roles:
        path = base_paths[role]
        if not path.is_dir() or path.is_symlink() or _is_junction(path):
            raise PythonEnvironmentIdentityError(
                f"CPython capability requires missing runtime root role {role}: {path}"
            )
        root_candidates.append((role, path.resolve(strict=True)))
    import_directories, import_archives = _runtime_import_candidates(base_prefix)
    root_candidates.extend((role, path) for role, path, _position in import_directories)
    roots, all_role_references = _root_forest(
        [(role, path.resolve(strict=True)) for role, path in root_candidates],
        root_prefix="runtime-root",
    )
    runtime_root_roles = [
        row for row in all_role_references if str(row["role"]) in required_root_role_set
    ]
    import_directory_references = {
        str(row["role"]): row
        for row in all_role_references
        if str(row["role"]).startswith("import-directory-")
    }
    pool = _FileNodePool(capture_context=capture_context)
    explicit_paths = {"base-executable": base_executable}
    if runtime_library is not None:
        explicit_paths["runtime-library"] = runtime_library
    if unicode_path is not None:
        explicit_paths["unicodedata"] = unicode_path
    native_dependency_closure = _native_dependency_closure(
        explicit_paths,
        operating_system=str(platform_payload["operating_system"]),
        architecture=str(platform_payload["architecture"]),
        policy=str(capabilities["native_dependency_policy"]),
        pool=pool,
    )
    runtime_roots: list[dict[str, object]] = []
    for root_id, path in roots:
        inventory, _files, _metadata = _stable_tree_inventory(
            path,
            root_id=root_id,
            label="Python runtime",
            pool=pool,
            pruned_components=frozenset({"site-packages", "dist-packages"}),
        )
        runtime_roots.append(inventory)
    root_entries = {
        root_id: {
            str(entry["path"]): entry
            for entry in cast(list[Mapping[str, object]], inventory["entries"])
        }
        for (root_id, _path), inventory in zip(roots, runtime_roots)
    }
    explicit: list[dict[str, object]] = []
    for role, resolved in explicit_paths.items():
        reference: dict[str, object] | None = None
        for root_id, root_path in roots:
            if not resolved.is_relative_to(root_path):
                continue
            relative = _relative_path(
                resolved, root_path, label="runtime explicit file"
            )
            row = root_entries[root_id].get(relative)
            if row is not None and _is_file_entry(row):
                reference = {
                    "role": role,
                    "kind": "root-reference",
                    "root": root_id,
                    "path": relative,
                    "node": row["node"],
                }
                break
        if reference is None:
            metadata = resolved.lstat()
            node = pool.bind(resolved, metadata, label="Python runtime explicit file")
            reference = {
                "role": role,
                "kind": "node-reference",
                "filename": resolved.name,
                "node": node,
            }
        explicit.append(reference)
    positioned_import_roots: list[tuple[int, dict[str, object]]] = []
    for role, _path, position in import_directories:
        reference = import_directory_references[role]
        positioned_import_roots.append(
            (
                position,
                {
                    "kind": "directory",
                    "root": reference["root"],
                    "path": reference["path"],
                },
            )
        )
    for filename, archive, position in import_archives:
        if archive is None:
            archive_row = cast(
                dict[str, object],
                {"kind": "absent-archive", "filename": filename},
            )
        else:
            node = pool.bind(
                archive,
                archive.lstat(),
                label="Python runtime import archive",
            )
            archive_row = cast(
                dict[str, object],
                {"kind": "archive", "filename": filename, "node": node},
            )
        positioned_import_roots.append((position, archive_row))
    import_roots = [row for _position, row in sorted(positioned_import_roots)]
    material = {
        "schema": PYTHON_RUNTIME_IDENTITY_SCHEMA,
        **platform_payload,
        "capabilities": capabilities,
        "file_nodes": pool.nodes,
        "native_dependency_closure": native_dependency_closure,
        "explicit_files": sorted(explicit, key=lambda row: str(row["role"])),
        "import_roots": import_roots,
        "runtime_root_roles": runtime_root_roles,
        "runtime_roots": runtime_roots,
    }
    payload = {**material, "runtime_closure_sha256": canonical_json_sha256(material)}
    return payload, tuple(roots), pool, explicit_paths


def capture_current_python_runtime(
    *, capture_context: PythonFileCaptureContext | None = None
) -> dict[str, object]:
    """Capture CPython's files, import roots, and loaded native ABI closure."""

    payload, _roots, _pool, _explicit = _capture_runtime_with_context(
        capture_context=capture_context
    )
    _pool.capture_context.verify()
    return payload


@identity_validator("Python runtime closure")
def validate_python_runtime_identity(payload: object) -> dict[str, object]:
    if (
        not isinstance(payload, dict)
        or set(payload) != _RUNTIME_IDENTITY_FIELDS
        or payload.get("schema") != PYTHON_RUNTIME_IDENTITY_SCHEMA
    ):
        raise PythonEnvironmentIdentityError("Python runtime closure shape is invalid")
    payload = cast(dict[str, object], payload)
    material = dict(payload)
    digest = material.pop("runtime_closure_sha256", None)
    if not _valid_sha256(digest) or digest != canonical_json_sha256(material):
        raise PythonEnvironmentIdentityError("Python runtime closure digest is invalid")
    version_text = payload.get("version")
    version_match = (
        re.fullmatch(
            r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:(?:a|b|rc)(?:0|[1-9][0-9]*))?",
            version_text,
        )
        if isinstance(version_text, str)
        else None
    )
    if version_match is None:
        raise PythonEnvironmentIdentityError("Python runtime version is invalid")
    version = tuple(int(item) for item in version_match.groups())
    if (
        payload.get("implementation") != "cpython"
        or len(version) != 3
        or version[:2] < (3, 12)
        or payload.get("operating_system") not in ("windows", "macos", "linux")
        or payload.get("architecture") not in ("x86_64", "arm64")
        or type(payload.get("pointer_bits")) is not int
        or payload.get("pointer_bits") != 64
        or payload.get("byteorder") != "little"
        or type(payload.get("py_debug")) is not bool
        or type(payload.get("gil_disabled")) is not bool
        or not isinstance(payload.get("cache_tag"), str)
        or not payload.get("cache_tag")
        or not isinstance(payload.get("soabi"), str)
        or not payload.get("soabi")
        or not isinstance(payload.get("abi_flags"), str)
        or not isinstance(payload.get("multiarch"), str)
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime platform/ABI identity is invalid"
        )
    capabilities = payload.get("capabilities")
    capability_fields = {
        "schema",
        "implementation_policy",
        "version_series",
        "operating_system",
        "architecture",
        "linkage",
        "unicodedata_linkage",
        "scanner_policy",
        "import_root_policy",
        "native_dependency_policy",
        "required_root_roles",
        "required_explicit_roles",
    }
    if not isinstance(capabilities, Mapping) or set(capabilities) != capability_fields:
        raise PythonEnvironmentIdentityError(
            "Python runtime capability vector is invalid"
        )
    capabilities = cast(Mapping[str, object], capabilities)
    linkage = capabilities.get("linkage")
    unicode_linkage = capabilities.get("unicodedata_linkage")
    expected_policy = _NATIVE_DEPENDENCY_POLICIES[str(payload["operating_system"])]
    expected_root_roles, expected_explicit_roles = _required_runtime_roles(
        payload,
        linkage=str(linkage),
        unicodedata_dynamic=unicode_linkage == "dynamic",
    )
    if (
        capabilities.get("schema") != PYTHON_RUNTIME_CAPABILITY_SCHEMA
        or capabilities.get("implementation_policy") != "cpython>=3.12"
        or capabilities.get("version_series") != ".".join(map(str, version[:2]))
        or capabilities.get("operating_system") != payload.get("operating_system")
        or capabilities.get("architecture") != payload.get("architecture")
        or linkage not in ("static", "shared", "framework")
        or unicode_linkage not in ("built-in", "dynamic")
        or capabilities.get("scanner_policy")
        != "no-follow-handle-two-snapshot-sha256-v1"
        or capabilities.get("import_root_policy") != "isolated-active-prefix-roots-v1"
        or capabilities.get("native_dependency_policy") != expected_policy
        or capabilities.get("required_root_roles") != list(expected_root_roles)
        or capabilities.get("required_explicit_roles") != list(expected_explicit_roles)
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime capability vector is invalid"
        )
    _nodes, nodes_by_id = _validate_file_nodes(
        payload.get("file_nodes"), label="Python runtime"
    )
    roots = payload.get("runtime_roots")
    if not isinstance(roots, list) or not roots:
        raise PythonEnvironmentIdentityError("Python runtime root closure is invalid")
    root_entries: dict[str, dict[str, Mapping[str, object]]] = {}
    referenced_nodes: set[str] = set()
    for index, root in enumerate(roots):
        if not isinstance(root, Mapping) or set(root) != {
            "id",
            "file_count",
            "node_ids",
            "entries",
            "manifest_sha256",
        }:
            raise PythonEnvironmentIdentityError(
                "Python runtime root closure is invalid"
            )
        entries, _paths, root_nodes = _validate_inventory_entries(
            root.get("entries"), label="Python runtime", nodes=nodes_by_id
        )
        root_id = root.get("id")
        expected_node_ids = sorted(
            root_nodes, key=lambda value: int(value.removeprefix("file-node-"))
        )
        if (
            root_id != f"runtime-root-{index}"
            or type(root.get("file_count")) is not int
            or root.get("file_count") != sum(_is_file_entry(entry) for entry in entries)
            or root.get("node_ids") != expected_node_ids
            or root.get("manifest_sha256") != canonical_json_sha256(root.get("entries"))
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime root closure is invalid"
            )
        root_entries[str(root_id)] = {str(row["path"]): row for row in entries}
        referenced_nodes.update(root_nodes)
    raw_root_roles = payload.get("runtime_root_roles")
    if not isinstance(raw_root_roles, list):
        raise PythonEnvironmentIdentityError(
            "Python runtime root-role closure is invalid"
        )
    root_roles: set[str] = set()
    referenced_roots: set[str] = set()
    for raw in raw_root_roles:
        if not isinstance(raw, Mapping) or set(raw) != {"role", "root", "path"}:
            raise PythonEnvironmentIdentityError(
                "Python runtime root-role closure is invalid"
            )
        role = str(raw.get("role"))
        root_id = str(raw.get("root"))
        path = raw.get("path")
        if (
            role in root_roles
            or role not in expected_root_roles
            or root_id not in root_entries
            or (
                path != "."
                and (
                    not _valid_relative_payload_path(path)
                    or root_entries[root_id].get(str(path), {}).get("kind")
                    != "directory"
                )
            )
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime root-role closure is invalid"
            )
        root_roles.add(role)
        referenced_roots.add(root_id)
    if root_roles != set(expected_root_roles) or raw_root_roles != sorted(
        raw_root_roles, key=lambda row: str(row["role"])
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime root-role closure is not canonical"
        )
    import_roots = payload.get("import_roots")
    if not isinstance(import_roots, list) or not import_roots:
        raise PythonEnvironmentIdentityError(
            "Python runtime import-root closure is invalid"
        )
    seen_import_roots: set[bytes] = set()
    expected_archive = f"python{version[0]}{version[1]}.zip".casefold()
    for row in import_roots:
        if not isinstance(row, Mapping):
            raise PythonEnvironmentIdentityError(
                "Python runtime import-root closure is invalid"
            )
        kind = row.get("kind")
        if kind == "directory":
            root_id = str(row.get("root"))
            path = row.get("path")
            valid = (
                set(row) == {"kind", "root", "path"}
                and root_id in root_entries
                and (
                    path == "."
                    or (
                        _valid_relative_payload_path(path)
                        and root_entries[root_id].get(str(path), {}).get("kind")
                        == "directory"
                    )
                )
            )
            referenced_roots.add(root_id)
        elif kind == "archive":
            filename = row.get("filename")
            node = row.get("node")
            valid = (
                set(row) == {"kind", "filename", "node"}
                and isinstance(filename, str)
                and filename
                and not any(separator in filename for separator in ("/", "\\"))
                and isinstance(node, str)
                and node in nodes_by_id
            )
            if valid:
                referenced_nodes.add(str(node))
        elif kind == "absent-archive":
            filename = row.get("filename")
            valid = (
                set(row) == {"kind", "filename"}
                and isinstance(filename, str)
                and filename.casefold() == expected_archive
            )
        else:
            valid = False
        encoded = canonical_json_bytes(dict(row))
        if not valid or encoded in seen_import_roots:
            raise PythonEnvironmentIdentityError(
                "Python runtime import-root closure is invalid"
            )
        seen_import_roots.add(encoded)
    if referenced_roots != set(root_entries):
        raise PythonEnvironmentIdentityError(
            "Python runtime root closure contains an unreferenced root"
        )
    explicit = payload.get("explicit_files")
    if not isinstance(explicit, list):
        raise PythonEnvironmentIdentityError(
            "Python runtime explicit-file closure is invalid"
        )
    explicit_roles: set[str] = set()
    for raw_row in explicit:
        if not isinstance(raw_row, Mapping):
            raise PythonEnvironmentIdentityError(
                "Python runtime explicit-file closure is invalid"
            )
        row = cast(Mapping[str, object], raw_row)
        role = str(row.get("role"))
        kind = row.get("kind")
        if role in explicit_roles or role not in expected_explicit_roles:
            raise PythonEnvironmentIdentityError(
                "Python runtime explicit-file closure is invalid"
            )
        if kind == "node-reference":
            filename = row.get("filename")
            node = row.get("node")
            valid = (
                set(row) == {"role", "kind", "filename", "node"}
                and isinstance(filename, str)
                and filename
                and not any(separator in filename for separator in ("/", "\\"))
                and isinstance(node, str)
                and node in nodes_by_id
            )
        elif kind == "root-reference":
            root_id = str(row.get("root"))
            target = root_entries.get(root_id, {}).get(str(row.get("path")))
            valid = (
                set(row) == {"role", "kind", "root", "path", "node"}
                and target is not None
                and _is_file_entry(target)
                and target.get("node") == row.get("node")
            )
        else:
            valid = False
        if not valid:
            raise PythonEnvironmentIdentityError(
                "Python runtime explicit-file reference is invalid"
            )
        explicit_roles.add(role)
        referenced_nodes.add(str(row.get("node")))
    if explicit_roles != set(expected_explicit_roles) or explicit != sorted(
        explicit, key=lambda row: str(row["role"])
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime explicit-file closure is not canonical"
        )
    dependency = payload.get("native_dependency_closure")
    if not isinstance(dependency, Mapping) or set(dependency) != {
        "status",
        "policy",
        "root_components",
        "observed_components",
        "observed_contracts",
        "components",
        "contracts",
        "edges",
        "deferred_imports",
        "closure_sha256",
    }:
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency closure is invalid"
        )
    dependency = cast(Mapping[str, object], dependency)
    dependency_material = {
        key: dependency[key]
        for key in dependency
        if key not in {"status", "closure_sha256"}
    }
    components = dependency.get("components")
    contracts = dependency.get("contracts")
    edges = dependency.get("edges")
    if (
        dependency.get("status") != "closed"
        or dependency.get("policy") != capabilities["native_dependency_policy"]
        or dependency.get("closure_sha256")
        != canonical_json_sha256(dependency_material)
        or not isinstance(components, list)
        or not components
        or not isinstance(contracts, list)
        or not all(isinstance(contract, str) for contract in contracts)
        or contracts != sorted(set(contracts))
        or not isinstance(edges, list)
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency closure is invalid"
        )
    component_ids: set[str] = set()
    component_roles: set[str] = set()
    filenames: set[str] = set()
    role_nodes = {
        str(row["role"]): row["node"]
        for row in cast(list[Mapping[str, object]], explicit)
    }
    expected_roots: list[str] = []
    prior_filename = ""
    for index, raw_component in enumerate(components):
        if not isinstance(raw_component, Mapping):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency component is invalid"
            )
        component = cast(Mapping[str, object], raw_component)
        filename = component.get("filename")
        roles = component.get("roles")
        component_id = component.get("id")
        filename_key = (
            filename.casefold()
            if isinstance(filename, str) and payload["operating_system"] == "windows"
            else filename
        )
        if (
            set(component) != {"id", "filename", "node", "roles"}
            or component_id != f"native-component-{index}"
            or not isinstance(filename, str)
            or not filename
            or any(separator in filename for separator in ("/", "\\", "\0"))
            or str(filename_key) < prior_filename
            or filename_key in filenames
            or not isinstance(component.get("node"), str)
            or component.get("node") not in nodes_by_id
            or not isinstance(roles, list)
            or not all(isinstance(role, str) for role in roles)
            or roles != sorted(set(roles))
            or any(role not in expected_explicit_roles for role in roles)
            or any(role in component_roles for role in roles)
            or any(role_nodes[str(role)] != component["node"] for role in roles)
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency component is invalid"
            )
        prior_filename = str(filename_key)
        filenames.add(str(filename_key))
        component_ids.add(str(component_id))
        component_roles.update(cast(list[str], roles))
        referenced_nodes.add(str(component.get("node")))
        if roles:
            expected_roots.append(str(component_id))
    native_required_roles = set(expected_explicit_roles)
    if component_roles != native_required_roles or dependency.get(
        "root_components"
    ) != sorted(expected_roots):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency roots are invalid"
        )
    if any(
        not _native_contract_valid(value, str(payload["operating_system"]))
        for value in contracts
    ):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency contract is invalid"
        )
    valid_contracts = set(contracts)
    observed_components = dependency.get("observed_components")
    observed_contracts = dependency.get("observed_contracts")
    ordered_component_ids = [
        f"native-component-{index}" for index in range(len(components))
    ]
    for observed, valid, ordered in (
        (observed_components, component_ids, ordered_component_ids),
        (observed_contracts, valid_contracts, contracts),
    ):
        if not isinstance(observed, list) or not all(
            isinstance(value, str) and value in valid for value in observed
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency observed census is invalid"
            )
        observed_set = set(observed)
        if observed != [value for value in ordered if value in observed_set]:
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency observed census is not canonical"
            )
    deferred = dependency.get("deferred_imports")
    if not isinstance(deferred, list):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency deferred declarations are invalid"
        )
    deferred_keys: list[tuple[int, str, str]] = []
    operating_system = str(payload["operating_system"])
    for declaration in deferred:
        if not isinstance(declaration, Mapping) or set(declaration) != {
            "from",
            "name",
            "kind",
        }:
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency deferred declaration is invalid"
            )
        source, name, kind = (declaration.get(key) for key in ("from", "name", "kind"))
        if (
            not isinstance(source, str)
            or source not in component_ids
            or not isinstance(name, str)
            or not name
            or "\0" in name
            or not isinstance(kind, str)
            or kind not in DEFERRED_DEPENDENCY_KINDS[operating_system]
            or (
                operating_system == "windows"
                and (
                    name != name.casefold()
                    or "/" in name
                    or "\\" in name
                    or not name.isascii()
                )
            )
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency deferred declaration is invalid"
            )
        deferred_keys.append(
            (int(source.removeprefix("native-component-")), name, kind)
        )
    if deferred_keys != sorted(set(deferred_keys)):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency deferred declarations are not canonical"
        )
    edge_pairs: list[tuple[str, str]] = []
    seen_edge_pairs: set[tuple[str, str]] = set()
    for edge in edges:
        if not isinstance(edge, Mapping) or set(edge) != {"from", "to"}:
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency edge is invalid"
            )
        if not isinstance(edge.get("from"), str) or not isinstance(edge.get("to"), str):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency edge is invalid"
            )
        typed_edge = cast(Mapping[str, object], edge)
        pair = (str(typed_edge["from"]), str(typed_edge["to"]))
        if (
            pair[0] not in component_ids
            or (pair[1] not in component_ids and pair[1] not in valid_contracts)
            or pair in seen_edge_pairs
        ):
            raise PythonEnvironmentIdentityError(
                "Python runtime native dependency edge is invalid"
            )
        edge_pairs.append(pair)
        seen_edge_pairs.add(pair)

    def edge_key(pair: tuple[str, str]) -> tuple[int, int, str]:
        source_index = int(pair[0].removeprefix("native-component-"))
        if pair[1] in component_ids:
            return (
                source_index,
                int(pair[1].removeprefix("native-component-")),
                "",
            )
        return (source_index, len(component_ids), pair[1])

    if edge_pairs != sorted(edge_pairs, key=edge_key):
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency edges are not canonical"
        )
    adjacency: dict[str, set[str]] = {}
    for source, target in edge_pairs:
        adjacency.setdefault(source, set()).add(target)
    reachable = (
        set(expected_roots)
        | set(cast(list[str], observed_components))
        | set(cast(list[str], observed_contracts))
    )
    pending = list(reachable)
    while pending:
        for target in adjacency.get(pending.pop(), set()):
            if target not in reachable:
                reachable.add(target)
                pending.append(target)
    if reachable != component_ids | valid_contracts:
        raise PythonEnvironmentIdentityError(
            "Python runtime native dependency closure contains unreachable components/contracts"
        )
    if set(nodes_by_id) != referenced_nodes:
        raise PythonEnvironmentIdentityError(
            "Python runtime has unreferenced file nodes"
        )
    return payload


def runtime_explicit_file_content(
    runtime: Mapping[str, object], role: str
) -> Mapping[str, object] | None:
    explicit = cast(list[Mapping[str, object]], runtime["explicit_files"])
    selected = next((row for row in explicit if row.get("role") == role), None)
    if selected is None:
        return None
    nodes = cast(list[Mapping[str, object]], runtime["file_nodes"])
    target = next((row for row in nodes if row.get("id") == selected.get("node")), None)
    if target is None:
        return None
    return {
        "filename": (
            selected.get("filename")
            if selected.get("kind") == "node-reference"
            else Path(str(selected["path"])).name
        ),
        "size": target["size"],
        "sha256": target["sha256"],
    }
