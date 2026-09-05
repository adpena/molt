"""Provisioned Python environment capture and v6 schema validation."""

from __future__ import annotations

import base64
import configparser
import csv
import hmac
import importlib.machinery
import importlib.util
import io
import marshal
import os
import re
import struct
import sys
import unicodedata
from collections.abc import Mapping, Sequence
from email.parser import Parser
from pathlib import Path, PurePath
from typing import cast

from molt.exact_json import canonical_json_sha256
from molt.python_environment_location import (
    _active_environment_prefix,
    _environment_sysconfig_path,
    _site_roots,
    editable_direct_url_path,
)
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
from molt.python_external_custody import (
    capture_external_import_custody,
    validate_active_import_finders,
    validate_external_import_custody,
)
from molt.python_identity_common import (
    canonical_absolute_path,
    identity_validator,
    PythonEnvironmentIdentityError,
    _canonicalize_name,
    _valid_relative_payload_path,
    _valid_sha256,
)
from molt.python_runtime_identity import (
    _capture_runtime_with_context,
    _platform_identity,
    runtime_explicit_file_content,
    validate_python_runtime_identity,
)

PYTHON_ENVIRONMENT_IDENTITY_SCHEMA = "molt.python-environment-closure.v6"
PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA = "molt.python-environment-capabilities.v2"
_EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
_ENVIRONMENT_IDENTITY_FIELDS = frozenset(
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
        "runtime",
        "selected_executable",
        "pyvenv_config",
        "scripts_root",
        "site_roots",
        "active_import_roots",
        "external_roots",
        "external_import_custody",
        "tree",
        "distributions",
        "distribution_inventory_sha256",
        "site_bootstrap_files",
        "console_scripts",
        "native_modules",
        "environment_closure_sha256",
    }
)
_DISTRIBUTION_IDENTITY_FIELDS = frozenset(
    {
        "name",
        "version",
        "record_sha256",
        "direct_url_sha256",
        "installer_sha256",
        "installed_file_count",
        "file_manifest_sha256",
        "installed_files",
        "entry_points",
        "entry_points_sha256",
        "console_scripts",
        "external_source",
    }
)


class _CaseSensitiveConfigParser(configparser.ConfigParser):
    def optionxform(self, optionstr: str) -> str:
        return optionstr


def virtualenv_site_bootstrap_relative_paths(root: Path) -> tuple[str, ...]:
    """Discover the one coherent virtualenv import-hook pair, when present.

    ``uv`` and ``virtualenv`` create these files outside wheel ``RECORD``
    ownership.  They are admissible only as a complete pair and remain sealed
    as ordinary file nodes by :func:`capture_current_python_environment`.
    """

    canonical_root = Path(root).resolve(strict=True)
    site_roots = _site_roots(canonical_root)
    candidates: list[tuple[str, str]] = []
    for site_root in site_roots:
        relative_root = _relative_path(
            site_root, canonical_root, label="virtualenv bootstrap site root"
        )
        pth = site_root / "_virtualenv.pth"
        module = site_root / "_virtualenv.py"
        present = (pth.is_file(), module.is_file())
        if present == (False, False):
            continue
        if present != (True, True) or pth.is_symlink() or module.is_symlink():
            raise PythonEnvironmentIdentityError(
                "virtualenv site bootstrap custody requires a regular "
                "_virtualenv.pth/_virtualenv.py pair"
            )
        candidates.append(
            (
                f"{relative_root}/_virtualenv.pth",
                f"{relative_root}/_virtualenv.py",
            )
        )
    if len(candidates) > 1:
        raise PythonEnvironmentIdentityError(
            "virtualenv site bootstrap custody is ambiguous across site roots"
        )
    return tuple(sorted(candidates[0])) if candidates else ()


def _canonical_external_roots(
    values: Sequence[Path], environment_root: Path
) -> tuple[tuple[str, Path], ...]:
    roots: list[Path] = []
    for raw in values:
        lexical = Path(os.path.abspath(raw))
        if not lexical.is_dir() or lexical.is_symlink() or _is_junction(lexical):
            raise PythonEnvironmentIdentityError(
                f"external Python import root is not a real directory: {lexical}"
            )
        resolved = lexical.resolve(strict=True)
        if resolved == environment_root or resolved.is_relative_to(environment_root):
            raise PythonEnvironmentIdentityError(
                f"external Python import root overlaps environment custody: {resolved}"
            )
        roots.append(resolved)
    roots = sorted(
        {os.path.normcase(str(root)): root for root in roots}.values(),
        key=lambda root: (os.path.normcase(str(root)), str(root)),
    )
    forest, _references = _root_forest(
        [(f"admission-{index:08d}", root) for index, root in enumerate(roots)],
        root_prefix="external-root",
    )
    return tuple(forest)


def _external_reference(
    path: Path,
    external_roots: Sequence[tuple[str, Path]],
    *,
    label: str,
) -> dict[str, str]:
    matches: list[tuple[int, str, Path, Path]] = []
    for root_id, root in external_roots:
        try:
            relative = path.relative_to(root)
        except ValueError:
            continue
        matches.append((len(root.parts), root_id, root, relative))
    if not matches:
        raise PythonEnvironmentIdentityError(
            f"{label} is outside admitted external Python roots: {path}"
        )
    _depth, root_id, _root, relative = max(matches)
    return {
        "root": root_id,
        "path": "."
        if not relative.parts
        else unicodedata.normalize("NFC", relative.as_posix()),
    }


def _editable_direct_url_source(
    data: bytes,
    external_roots: Sequence[tuple[str, Path]],
    *,
    distribution: str,
) -> dict[str, str]:
    root = editable_direct_url_path(data, distribution=distribution)
    return _external_reference(
        root,
        external_roots,
        label=f"editable distribution {distribution!r}",
    )


def _scan_environment_tree(
    root: Path,
    *,
    base_executable: Path,
    excluded: frozenset[str],
    capture_context: PythonFileCaptureContext,
) -> tuple[dict[str, object], set[str], dict[str, os.stat_result], _FileNodePool]:
    pool = _FileNodePool(capture_context=capture_context)
    inventory, files, metadata = _stable_tree_inventory(
        root,
        root_id="environment-root",
        label="Python environment",
        pool=pool,
        excluded=excluded,
        external_symlink_role=(base_executable, "base-executable"),
    )
    return inventory, files, metadata, pool


def _declared_digest(value: str, sha256: str, *, label: str) -> str | None:
    if not value:
        return None
    try:
        algorithm, expected = value.split("=", 1)
    except ValueError as exc:
        raise PythonEnvironmentIdentityError(
            f"invalid RECORD hash for {label}"
        ) from exc
    if algorithm != "sha256":
        raise PythonEnvironmentIdentityError(
            f"unsupported RECORD hash for {label}: {algorithm}"
        )
    digest = bytes.fromhex(sha256)
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    if not hmac.compare_digest(expected.rstrip("="), encoded):
        raise PythonEnvironmentIdentityError(
            f"installed distribution RECORD mismatch: {label}"
        )
    return f"{algorithm}={expected.rstrip('=')}"


def _declared_sha256_matches(value: object, sha256: object) -> bool:
    if value is None:
        return True
    if not isinstance(value, str) or not _valid_sha256(sha256):
        return False
    expected = (
        base64.urlsafe_b64encode(bytes.fromhex(str(sha256))).decode("ascii").rstrip("=")
    )
    return hmac.compare_digest(value, f"sha256={expected}")


def _console_script_filenames(script_name: str) -> tuple[str, ...]:
    if (
        not script_name
        or script_name != unicodedata.normalize("NFC", script_name)
        or any(separator in script_name for separator in ("/", "\\"))
        or script_name in {".", ".."}
    ):
        raise PythonEnvironmentIdentityError(
            f"invalid console entry-point name: {script_name!r}"
        )
    return (
        script_name,
        f"{script_name}.exe",
        f"{script_name}-script.py",
        f"{script_name}.cmd",
        f"{script_name}.bat",
    )


def _record_environment_path(site_root: str, raw: str, *, label: str) -> str:
    if not raw or "\\" in raw or "\x00" in raw or raw.startswith("/"):
        raise PythonEnvironmentIdentityError(
            f"invalid RECORD path for {label}: {raw!r}"
        )
    parts = site_root.split("/")
    floor = 0
    for part in raw.split("/"):
        if part in {"", "."}:
            raise PythonEnvironmentIdentityError(
                f"invalid RECORD path for {label}: {raw!r}"
            )
        if part == "..":
            if len(parts) == floor:
                raise PythonEnvironmentIdentityError(
                    f"RECORD path escapes environment custody for {label}: {raw!r}"
                )
            parts.pop()
        else:
            parts.append(unicodedata.normalize("NFC", part))
    value = "/".join(parts)
    if not _valid_relative_payload_path(value):
        raise PythonEnvironmentIdentityError(
            f"invalid RECORD path for {label}: {raw!r}"
        )
    return value


def _installed_distributions(
    site_roots: Sequence[str],
    scripts_root: str,
    tree_by_path: Mapping[str, Mapping[str, object]],
    nodes_by_id: Mapping[str, Mapping[str, object]],
    pool: _FileNodePool,
    external_roots: Sequence[tuple[str, Path]] = (),
) -> tuple[list[dict[str, object]], set[str], dict[str, list[str]]]:
    rows: list[dict[str, object]] = []
    owned_files: set[str] = set()
    console_owners: dict[str, list[str]] = {}
    file_owners: dict[str, str] = {}
    names: set[str] = set()
    metadata_roots: list[tuple[str, str]] = []
    for site_root in site_roots:
        prefix = f"{site_root}/"
        for path, row in tree_by_path.items():
            suffix = path.removeprefix(prefix)
            if (
                path.startswith(prefix)
                and "/" not in suffix
                and suffix.casefold().endswith(".dist-info")
                and row.get("kind") == "directory"
            ):
                metadata_roots.append((site_root, path))
    metadata_roots.sort(key=lambda row: (row[1].casefold(), row[1]))

    def file_node(path: str, *, label: str) -> tuple[str, Mapping[str, object]]:
        row = tree_by_path.get(path)
        node_id = row.get("node") if row is not None else None
        node = nodes_by_id.get(str(node_id))
        if row is None or not _is_file_entry(row) or node is None:
            raise PythonEnvironmentIdentityError(
                f"{label} is absent from the stable tree: {path}"
            )
        return str(node_id), node

    def read_bound(path: str, *, label: str) -> bytes:
        node_id, _node = file_node(path, label=label)
        return pool.read_bound(node_id, label=label)

    for site_root, metadata_root in metadata_roots:
        metadata_path = f"{metadata_root}/METADATA"
        record_path = f"{metadata_root}/RECORD"
        try:
            metadata_text = read_bound(
                metadata_path, label="distribution METADATA"
            ).decode("utf-8")
            record_data = read_bound(record_path, label="distribution RECORD")
        except UnicodeDecodeError as exc:
            raise PythonEnvironmentIdentityError(
                f"installed distribution metadata is not UTF-8: {metadata_root}"
            ) from exc
        message = Parser().parsestr(metadata_text)
        raw_name = message.get("Name")
        if not isinstance(raw_name, str) or not raw_name.strip():
            raise PythonEnvironmentIdentityError(
                "installed distribution has no Name metadata"
            )
        name = _canonicalize_name(raw_name)
        if name in names:
            raise PythonEnvironmentIdentityError(
                f"environment contains duplicate distribution {name!r}"
            )
        names.add(name)
        version = message.get("Version")
        if not isinstance(version, str) or not version.strip():
            raise PythonEnvironmentIdentityError(
                f"installed distribution {name!r} has no Version metadata"
            )
        try:
            record_text = record_data.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise PythonEnvironmentIdentityError(
                f"installed distribution {name!r} RECORD is not UTF-8"
            ) from exc
        try:
            record_rows = list(csv.reader(io.StringIO(record_text, newline="")))
        except csv.Error as exc:
            raise PythonEnvironmentIdentityError(
                f"installed distribution {name!r} RECORD is invalid"
            ) from exc
        if not record_rows:
            raise PythonEnvironmentIdentityError(
                f"installed distribution {name!r} has no RECORD file inventory"
            )
        installed: list[dict[str, object]] = []
        installed_paths: set[str] = set()
        for raw_record_row in record_rows:
            if len(raw_record_row) != 3:
                raise PythonEnvironmentIdentityError(
                    f"installed distribution {name!r} RECORD row is invalid"
                )
            raw_path, declared_hash, declared_size = raw_record_row
            relative = _record_environment_path(
                site_root, raw_path, label=f"{name}:{raw_path}"
            )
            if relative in installed_paths:
                raise PythonEnvironmentIdentityError(
                    f"installed distribution {name!r} repeats RECORD path {relative}"
                )
            installed_paths.add(relative)
            node_id, node = file_node(relative, label=f"{name} RECORD entry")
            size = node.get("size")
            if type(size) is not int or (
                declared_size
                and (not declared_size.isdecimal() or int(declared_size) != size)
            ):
                raise PythonEnvironmentIdentityError(
                    f"installed distribution RECORD size mismatch: {name}:{raw_path}"
                )
            installed.append(
                {
                    "path": relative,
                    "node": node_id,
                    "declared": _declared_digest(
                        declared_hash,
                        str(node["sha256"]),
                        label=f"{name}:{raw_path}",
                    ),
                }
            )
            previous_owner = file_owners.get(relative)
            if previous_owner is not None:
                raise PythonEnvironmentIdentityError(
                    "installed distributions claim the same file: "
                    f"{previous_owner}, {name}: {relative}"
                )
            file_owners[relative] = name
            owned_files.add(relative)
        installed.sort(key=lambda row: (str(row["path"]).casefold(), str(row["path"])))
        entry_points_path = f"{metadata_root}/entry_points.txt"
        entry_points: list[dict[str, str]] = []
        if entry_points_path in tree_by_path:
            try:
                entry_points_text = read_bound(
                    entry_points_path, label=f"{name} entry_points.txt"
                ).decode("utf-8")
                parser = _CaseSensitiveConfigParser(
                    interpolation=None,
                    delimiters=("=",),
                    strict=True,
                )
                parser.read_string(entry_points_text)
            except (UnicodeDecodeError, configparser.Error) as exc:
                raise PythonEnvironmentIdentityError(
                    f"installed distribution {name!r} has invalid entry_points.txt"
                ) from exc
            for group in parser.sections():
                for entry_name, entry_value in parser.items(group):
                    entry_points.append(
                        {"group": group, "name": entry_name, "value": entry_value}
                    )
        entry_points.sort(
            key=lambda value: (value["group"], value["name"], value["value"])
        )
        console_scripts: dict[str, list[str]] = {}
        for entry_point in entry_points:
            if entry_point["group"] != "console_scripts":
                continue
            script_name = str(entry_point["name"])
            candidates = tuple(
                candidate
                for candidate in (
                    f"{scripts_root}/{filename}"
                    for filename in _console_script_filenames(script_name)
                )
                if _is_file_entry(tree_by_path.get(candidate, {"kind": "absent"}))
            )
            if not candidates:
                raise PythonEnvironmentIdentityError(
                    f"installed console entry point has no launcher: {name}:{script_name}"
                )
            paths = sorted(candidates)
            previous = console_owners.get(script_name)
            if previous is not None:
                raise PythonEnvironmentIdentityError(
                    f"multiple distributions own console entry point {script_name!r}"
                )
            console_owners[script_name] = paths
            console_scripts[script_name] = paths
        file_manifest = canonical_json_sha256(installed)
        direct_url_path = f"{metadata_root}/direct_url.json"
        external_source: dict[str, str] | None = None
        direct_url_sha256 = _EMPTY_SHA256
        if direct_url_path in tree_by_path:
            direct_url_node_id, direct_url_node = file_node(
                direct_url_path, label=f"{name} direct_url.json"
            )
            external_source = _editable_direct_url_source(
                pool.read_bound(direct_url_node_id, label=f"{name} direct_url.json"),
                external_roots,
                distribution=name,
            )
            direct_url_sha256 = str(direct_url_node["sha256"])
        _record_node_id, record_node = file_node(record_path, label=f"{name} RECORD")
        installer_path = f"{metadata_root}/INSTALLER"
        installer_node = (
            file_node(installer_path, label=f"{name} INSTALLER")[1]
            if installer_path in tree_by_path
            else None
        )
        rows.append(
            {
                "name": name,
                "version": version.strip(),
                "record_sha256": record_node["sha256"],
                "direct_url_sha256": direct_url_sha256,
                "installer_sha256": (
                    installer_node["sha256"]
                    if installer_node is not None
                    else _EMPTY_SHA256
                ),
                "installed_file_count": len(installed),
                "file_manifest_sha256": file_manifest,
                "installed_files": installed,
                "entry_points": entry_points,
                "entry_points_sha256": canonical_json_sha256(entry_points),
                "console_scripts": console_scripts,
                "external_source": external_source,
            }
        )
    rows.sort(key=lambda row: (str(row["name"]), str(row["version"])))
    return rows, owned_files, console_owners


def _bytecode_owner(
    relative: str,
    root: Path,
    owned: set[str],
    tree_by_path: Mapping[str, Mapping[str, object]],
    metadata_by_path: Mapping[str, os.stat_result],
    pool: _FileNodePool,
    *,
    pytest_version: str | None,
) -> str | None:
    path = root / Path(relative)
    if path.suffix.casefold() != ".pyc":
        return None
    pytest_rewrite = False
    cache_tag = re.escape(str(sys.implementation.cache_tag))
    cache_match = (
        re.fullmatch(
            rf"(?P<stem>.+)\.{cache_tag}(?P<variant>(?:\.opt-[0-9]+)?|(?:-pytest-[A-Za-z0-9.]+))\.pyc",
            path.name,
        )
        if path.parent.name == "__pycache__"
        else None
    )
    if cache_match is not None:
        source = path.parent.parent / f"{cache_match.group('stem')}.py"
        variant = cache_match.group("variant")
        if variant.startswith("-pytest-"):
            pytest_rewrite = True
            if pytest_version is None or variant != f"-pytest-{pytest_version}":
                raise PythonEnvironmentIdentityError(
                    f"pytest bytecode cache has no matching tool identity: {path}"
                )
    else:
        try:
            source = Path(importlib.util.source_from_cache(str(path)))
        except (ValueError, NotImplementedError):
            source = path.with_suffix(".py")
    try:
        source_relative = _relative_path(source, root, label="bytecode source")
    except PythonEnvironmentIdentityError:
        return None
    if source_relative not in owned:
        return None
    source_row = tree_by_path.get(source_relative)
    bytecode_row = tree_by_path.get(relative)
    if (
        source_row is None
        or bytecode_row is None
        or not _is_file_entry(source_row)
        or not _is_file_entry(bytecode_row)
    ):
        return None
    source_node = source_row.get("node")
    bytecode_node = bytecode_row.get("node")
    if not isinstance(source_node, str) or not isinstance(bytecode_node, str):
        return None
    source_data = pool.read_bound(source_node, label="owned bytecode source")
    bytecode = pool.read_bound(bytecode_node, label="generated bytecode")
    if len(bytecode) < 16 or bytecode[:4] != importlib.util.MAGIC_NUMBER:
        raise PythonEnvironmentIdentityError(
            f"generated bytecode has an invalid CPython header: {path}"
        )
    flags = struct.unpack_from("<I", bytecode, 4)[0]
    if flags & ~0b11:
        raise PythonEnvironmentIdentityError(
            f"generated bytecode uses unsupported header flags: {path}"
        )
    if flags & 0b1:
        if bytecode[8:16] != importlib.util.source_hash(source_data):
            raise PythonEnvironmentIdentityError(
                f"generated bytecode source hash is invalid: {path}"
            )
    else:
        timestamp, source_size = struct.unpack_from("<II", bytecode, 8)
        source_stat = metadata_by_path.get(source_relative)
        if source_stat is None:
            return None
        if timestamp != int(source_stat.st_mtime) & 0xFFFFFFFF or source_size != (
            len(source_data) & 0xFFFFFFFF
        ):
            raise PythonEnvironmentIdentityError(
                f"generated bytecode source metadata is invalid: {path}"
            )
    optimization_match = re.search(r"\.opt-(\d+)\.pyc$", path.name)
    optimization = int(optimization_match.group(1)) if optimization_match else 0
    if optimization not in {0, 1, 2}:
        raise PythonEnvironmentIdentityError(
            f"generated bytecode optimization level is unsupported: {path}"
        )
    if pytest_rewrite:
        return source_relative
    expected_code = compile(
        source_data,
        str(source),
        "exec",
        dont_inherit=True,
        optimize=optimization,
    )
    if bytecode[16:] != marshal.dumps(expected_code):
        raise PythonEnvironmentIdentityError(
            f"generated bytecode does not match its owned source: {path}"
        )
    return source_relative


def _active_environment_import_roots(
    environment_root: Path,
    site_roots: Sequence[Path],
    runtime: Mapping[str, object],
    runtime_roots: Sequence[tuple[str, Path]],
    runtime_pool: _FileNodePool,
    external_roots: Sequence[tuple[str, Path]] = (),
) -> tuple[list[dict[str, object]], dict[str, object]]:
    flags = sys.flags
    isolation = {
        "isolated": bool(flags.isolated),
        "ignore_environment": bool(flags.ignore_environment),
        "no_user_site": bool(flags.no_user_site),
        "safe_path": bool(getattr(flags, "safe_path", False)),
    }
    if not all(isolation.values()):
        raise PythonEnvironmentIdentityError(
            "Python environment identity requires -I isolated interpreter operation"
        )
    runtime_imports = cast(list[Mapping[str, object]], runtime["import_roots"])
    site_by_path = {
        os.path.normcase(str(path)): f"site-root-{index}"
        for index, path in enumerate(site_roots)
    }
    probe_directory = Path(__file__).resolve(strict=True).parent
    rows: list[dict[str, object]] = []
    observed_roles: set[str] = set()
    environment_root_active = False
    external_import_count = 0

    def append(role: str, row: Mapping[str, object]) -> None:
        if role in observed_roles:
            raise PythonEnvironmentIdentityError(
                f"active Python import root role is duplicated: {role}"
            )
        observed_roles.add(role)
        rows.append({"role": role, **row})

    for raw in sys.path:
        if not raw:
            raise PythonEnvironmentIdentityError(
                "isolated Python import roots contain an ambient current-directory entry"
            )
        lexical = Path(os.path.abspath(raw))
        if lexical.is_dir():
            resolved = lexical.resolve(strict=True)
            if resolved == probe_directory:
                # The probe is a self-contained script, not an admitted package root.
                continue
            if resolved == environment_root:
                environment_root_active = True
                append(
                    "environment-root",
                    {"owner": "environment", "kind": "directory", "path": "."},
                )
                continue
            site_role = site_by_path.get(os.path.normcase(str(resolved)))
            if site_role is not None:
                relative = _relative_path(
                    resolved, environment_root, label="active environment import root"
                )
                append(
                    site_role,
                    {"owner": "environment", "kind": "directory", "path": relative},
                )
                continue
            runtime_row: dict[str, object] | None = None
            for root_id, root_path in runtime_roots:
                if resolved.is_relative_to(root_path):
                    relative = (
                        "."
                        if resolved == root_path
                        else _relative_path(
                            resolved, root_path, label="active runtime import root"
                        )
                    )
                    candidate: dict[str, object] = {
                        "kind": "directory",
                        "root": root_id,
                        "path": relative,
                    }
                    if candidate in runtime_imports:
                        runtime_row = candidate
                        break
            if runtime_row is not None:
                append(
                    f"runtime-import-{runtime_imports.index(runtime_row)}",
                    {"owner": "runtime", **runtime_row},
                )
                continue
            try:
                external = _external_reference(
                    resolved,
                    external_roots,
                    label="active external Python import root",
                )
            except PythonEnvironmentIdentityError:
                pass
            else:
                append(
                    f"external-import-{external_import_count}",
                    {"owner": "external", "kind": "directory", **external},
                )
                external_import_count += 1
                continue
            raise PythonEnvironmentIdentityError(
                f"isolated Python import roots leak outside runtime/environment custody: {lexical}"
            )
        if lexical.is_file() and lexical.suffix.casefold() in {".zip", ".pyz"}:
            resolved = lexical.resolve(strict=True)
            node = runtime_pool.node_for_metadata(resolved.lstat())
            candidate = {"kind": "archive", "filename": lexical.name, "node": node}
            if node is not None and candidate in runtime_imports:
                append(
                    f"runtime-import-{runtime_imports.index(candidate)}",
                    {"owner": "runtime", **candidate},
                )
                continue
            raise PythonEnvironmentIdentityError(
                f"isolated Python import archive is outside runtime custody: {lexical}"
            )
        expected_archive = f"python{sys.version_info.major}{sys.version_info.minor}.zip"
        if (
            not lexical.exists()
            and lexical.name.casefold() == expected_archive.casefold()
        ):
            candidate = {"kind": "absent-archive", "filename": lexical.name}
            if candidate in runtime_imports:
                append(
                    f"runtime-import-{runtime_imports.index(candidate)}",
                    {"owner": "runtime", **candidate},
                )
                continue
        raise PythonEnvironmentIdentityError(
            f"isolated Python import root is unsupported or absent: {lexical}"
        )
    required_roles = {
        *(f"runtime-import-{index}" for index in range(len(runtime_imports))),
        *(f"site-root-{index}" for index in range(len(site_roots))),
    }
    required_roles.update(
        f"external-import-{index}" for index in range(external_import_count)
    )
    if environment_root_active:
        required_roles.add("environment-root")
    if observed_roles != required_roles:
        missing = ", ".join(sorted(required_roles - observed_roles))
        extra = ", ".join(sorted(observed_roles - required_roles))
        raise PythonEnvironmentIdentityError(
            "isolated Python import-root closure differs from required roles"
            + (f"; missing={missing}" if missing else "")
            + (f"; extra={extra}" if extra else "")
        )
    capabilities = {
        "schema": PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA,
        "runtime": runtime["capabilities"],
        **isolation,
        "environment_root_active": environment_root_active,
        "required_active_import_roles": sorted(required_roles),
    }
    return rows, capabilities


def capture_current_python_environment(
    root: Path,
    *,
    excluded_relative_paths: Sequence[str] = (),
    admitted_site_bootstrap_paths: Sequence[str] = (),
    admitted_external_roots: Sequence[Path] = (),
    capture_context: PythonFileCaptureContext | None = None,
) -> dict[str, object]:
    """Capture an exact immutable environment using the active interpreter."""

    validate_active_import_finders(bootstrap_pending=True)
    if capture_context is None:
        capture_context = PythonFileCaptureContext()

    lexical_root = Path(os.path.abspath(root))
    if (
        not lexical_root.is_dir()
        or lexical_root.is_symlink()
        or _is_junction(lexical_root)
    ):
        raise PythonEnvironmentIdentityError(
            f"environment root is not a real directory: {lexical_root}"
        )
    canonical_root = lexical_root.resolve(strict=True)
    external_roots = _canonical_external_roots(admitted_external_roots, canonical_root)
    active_root = _active_environment_prefix()
    if active_root != canonical_root:
        raise PythonEnvironmentIdentityError(
            f"active interpreter prefix {active_root} differs from environment {canonical_root}"
        )
    base_executable = Path(
        getattr(sys, "_base_executable", None) or sys.executable
    ).resolve(strict=True)
    runtime, runtime_roots, runtime_pool, _runtime_explicit_paths = (
        _capture_runtime_with_context(capture_context=capture_context)
    )
    scripts_path = _environment_sysconfig_path("scripts", canonical_root)
    if scripts_path is None:
        raise PythonEnvironmentIdentityError("environment has no scripts path")
    scripts_root = Path(scripts_path).absolute()
    _relative_path(scripts_root, canonical_root, label="scripts root")
    if (
        not scripts_root.is_dir()
        or scripts_root.is_symlink()
        or _is_junction(scripts_root)
    ):
        raise PythonEnvironmentIdentityError(
            f"environment scripts root is not a real directory: {scripts_root}"
        )
    site_roots = _site_roots(canonical_root)
    excluded = frozenset(
        unicodedata.normalize("NFC", Path(value).as_posix())
        for value in excluded_relative_paths
    )
    tree, tree_files, tree_metadata, tree_pool = _scan_environment_tree(
        canonical_root,
        base_executable=base_executable,
        excluded=excluded,
        capture_context=capture_context,
    )
    tree_entries = cast(list[Mapping[str, object]], tree["entries"])
    tree_by_path = {str(row["path"]): row for row in tree_entries}
    nodes = tree_pool.nodes
    nodes_by_id = {str(row["id"]): row for row in nodes}
    site_relatives = tuple(
        sorted(
            (
                _relative_path(path, canonical_root, label="site root")
                for path in site_roots
            ),
            key=lambda value: (value.casefold(), value),
        )
    )
    scripts_relative = _relative_path(
        scripts_root, canonical_root, label="scripts root"
    )
    distributions, owned_files, console_scripts = _installed_distributions(
        site_relatives,
        scripts_relative,
        tree_by_path,
        nodes_by_id,
        tree_pool,
        external_roots,
    )
    pytest_version = next(
        (
            str(distribution["version"])
            for distribution in distributions
            if distribution.get("name") == "pytest"
        ),
        None,
    )
    site_bootstrap_files: list[dict[str, str]] = []
    admitted_bootstrap = sorted(
        {
            unicodedata.normalize("NFC", Path(value).as_posix())
            for value in admitted_site_bootstrap_paths
        },
        key=lambda value: (value.casefold(), value),
    )
    for relative in admitted_bootstrap:
        if not _valid_relative_payload_path(relative) or not any(
            relative == site or relative.startswith(f"{site}/")
            for site in site_relatives
        ):
            raise PythonEnvironmentIdentityError(
                f"admitted site bootstrap path is invalid: {relative!r}"
            )
        row = tree_by_path.get(relative)
        if row is None or row.get("kind") not in {"file", "hardlink"}:
            raise PythonEnvironmentIdentityError(
                f"admitted site bootstrap file is absent: {relative}"
            )
        node = row.get("node")
        if not isinstance(node, str) or node not in nodes_by_id:
            raise PythonEnvironmentIdentityError(
                f"admitted site bootstrap file has no stable node: {relative}"
            )
        if relative in owned_files:
            raise PythonEnvironmentIdentityError(
                f"site bootstrap file overlaps installed distribution custody: {relative}"
            )
        owned_files.add(relative)
        site_bootstrap_files.append({"path": relative, "node": node})
    unowned: list[str] = []
    for relative in sorted(tree_files):
        if relative in owned_files:
            continue
        if any(
            relative == site or relative.startswith(f"{site}/")
            for site in site_relatives
        ):
            if (
                _bytecode_owner(
                    relative,
                    canonical_root,
                    owned_files,
                    tree_by_path,
                    tree_metadata,
                    tree_pool,
                    pytest_version=pytest_version,
                )
                is None
            ):
                unowned.append(relative)
    if unowned:
        raise PythonEnvironmentIdentityError(
            "environment site-packages contains unowned files: "
            + ", ".join(unowned[:8])
        )
    pyvenv = canonical_root / "pyvenv.cfg"
    if not pyvenv.is_file() or pyvenv.is_symlink():
        raise PythonEnvironmentIdentityError(
            f"environment has no regular pyvenv.cfg: {pyvenv}"
        )
    selected = Path(os.path.abspath(sys.executable))
    selected_relative = _relative_path(
        selected, canonical_root, label="selected interpreter"
    )
    selected_row = tree_by_path.get(selected_relative)
    if selected_row is None or not _is_file_entry(selected_row):
        raise PythonEnvironmentIdentityError(
            "selected interpreter is absent from the stable environment tree"
        )
    if selected_row.get("target_owner") == "base-runtime":
        selected_identity = {
            "path": selected_relative,
            "kind": "runtime-role-reference",
            "role": "base-executable",
        }
    else:
        selected_identity = {
            "path": selected_relative,
            "kind": "tree-reference",
            "node": selected_row["node"],
        }
    pyvenv_row = tree_by_path.get("pyvenv.cfg")
    if pyvenv_row is None or pyvenv_row.get("kind") not in {"file", "hardlink"}:
        raise PythonEnvironmentIdentityError(
            "environment pyvenv.cfg is absent from the stable file-node closure"
        )
    active_import_roots, capabilities = _active_environment_import_roots(
        canonical_root,
        site_roots,
        runtime,
        runtime_roots,
        runtime_pool,
        external_roots,
    )
    external_import_custody = capture_external_import_custody(
        environment_root=canonical_root,
        external_roots=external_roots,
        active_import_roots=active_import_roots,
        distributions=distributions,
        tree_entries=tree_by_path,
        tree_nodes=nodes_by_id,
        tree_pool=tree_pool,
        site_roots=site_relatives,
        bootstrap_paths=admitted_bootstrap,
        capture_context=capture_context,
    )
    native_modules = [
        {
            "path": str(row["path"]),
            "node": row["node"],
        }
        for row in tree_entries
        if row.get("kind") in {"file", "hardlink", "symlink"}
        and "node" in row
        and str(row.get("path", "")).endswith(
            tuple(importlib.machinery.EXTENSION_SUFFIXES)
        )
    ]
    material = {
        "schema": PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
        **_platform_identity(),
        "capabilities": capabilities,
        "runtime": runtime,
        "selected_executable": selected_identity,
        "pyvenv_config": {"path": "pyvenv.cfg", "node": pyvenv_row["node"]},
        "scripts_root": scripts_relative,
        "site_roots": list(site_relatives),
        "active_import_roots": active_import_roots,
        "external_roots": [
            {"id": root_id, "path": str(path)} for root_id, path in external_roots
        ],
        "external_import_custody": external_import_custody,
        "tree": {**tree, "file_nodes": nodes},
        "distributions": distributions,
        "distribution_inventory_sha256": canonical_json_sha256(distributions),
        "site_bootstrap_files": site_bootstrap_files,
        "console_scripts": console_scripts,
        "native_modules": native_modules,
    }
    capture_context.verify()
    return {
        **material,
        "environment_closure_sha256": canonical_json_sha256(material),
    }


def python_environment_executable_files(
    payload: object,
    root: Path,
    *,
    base_executable: Path,
) -> list[dict[str, object]]:
    """Project every environment-owned process launcher from one closure."""

    environment = validate_python_environment_identity(payload)
    canonical_root = Path(root).resolve(strict=True)
    if not canonical_root.is_dir():
        raise PythonEnvironmentIdentityError(
            f"Python environment root is not a directory: {canonical_root}"
        )
    tree = cast(Mapping[str, object], environment["tree"])
    entries = cast(list[Mapping[str, object]], tree["entries"])
    nodes = cast(list[Mapping[str, object]], tree["file_nodes"])
    entries_by_path = {str(row["path"]): row for row in entries}
    nodes_by_id = {str(row["id"]): row for row in nodes}
    runtime = cast(Mapping[str, object], environment["runtime"])
    runtime_base = runtime_explicit_file_content(runtime, "base-executable")
    if runtime_base is None:
        raise PythonEnvironmentIdentityError(
            "Python environment runtime has no base executable content"
        )
    projected: list[dict[str, object]] = []

    def append(role: str, relative: str) -> None:
        entry = entries_by_path.get(relative)
        node = nodes_by_id.get(str(entry.get("node"))) if entry is not None else None
        if entry is None or not _is_file_entry(entry) or node is None:
            raise PythonEnvironmentIdentityError(
                f"Python environment launcher is absent from its file closure: {relative}"
            )
        projected.append(
            {
                "role": role,
                "path": str(canonical_root / Path(relative)),
                "size": node["size"],
                "sha256": node["sha256"],
            }
        )

    selected = cast(Mapping[str, object], environment["selected_executable"])
    selected_relative = str(selected["path"])
    if selected.get("kind") == "tree-reference":
        append("selected-interpreter", selected_relative)
    else:
        projected.append(
            {
                "role": "selected-interpreter",
                "path": str(canonical_root / Path(selected_relative)),
                "size": runtime_base["size"],
                "sha256": runtime_base["sha256"],
            }
        )
    projected.append(
        {
            "role": "base-interpreter",
            "path": str(Path(base_executable).resolve(strict=True)),
            "size": runtime_base["size"],
            "sha256": runtime_base["sha256"],
        }
    )
    console_scripts = cast(Mapping[str, list[str]], environment["console_scripts"])
    projected_paths: set[str] = {str(row["path"]) for row in projected}
    for name in sorted(console_scripts):
        for relative in console_scripts[name]:
            append(f"console-script:{name}", relative)
            projected_paths.add(str(canonical_root / Path(relative)))
    operating_system = str(environment["operating_system"])
    distributions = cast(list[Mapping[str, object]], environment["distributions"])
    for distribution in distributions:
        name = str(distribution["name"])
        installed = cast(list[Mapping[str, object]], distribution["installed_files"])
        for row in installed:
            relative = str(row["path"])
            absolute = str(canonical_root / Path(relative))
            if absolute in projected_paths:
                continue
            entry = entries_by_path[relative]
            access = entry.get("access")
            native_launcher = (
                Path(relative).suffix.casefold() in {".exe", ".com"}
                if operating_system == "windows"
                else isinstance(access, Mapping)
                and access.get("executable") is True
                and Path(relative).suffix.casefold()
                not in {".py", ".pyc", ".sh", ".bash", ".zsh", ".fish"}
            )
            if native_launcher:
                append(f"distribution-executable:{name}", relative)
                projected_paths.add(absolute)
    return projected


@identity_validator("Python environment closure")
def validate_python_environment_identity(payload: object) -> dict[str, object]:
    if (
        not isinstance(payload, dict)
        or set(payload) != _ENVIRONMENT_IDENTITY_FIELDS
        or payload.get("schema") != PYTHON_ENVIRONMENT_IDENTITY_SCHEMA
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment closure shape is invalid"
        )
    payload = cast(dict[str, object], payload)
    material = dict(payload)
    digest = material.pop("environment_closure_sha256", None)
    if not _valid_sha256(digest) or digest != canonical_json_sha256(material):
        raise PythonEnvironmentIdentityError(
            "Python environment closure digest is invalid"
        )
    runtime = validate_python_runtime_identity(payload.get("runtime"))
    if (
        payload.get("implementation") != "cpython"
        or payload.get("operating_system") not in {"windows", "macos", "linux"}
        or payload.get("architecture") not in {"x86_64", "arm64"}
        or not isinstance(payload.get("distributions"), list)
        or not isinstance(payload.get("tree"), dict)
        or not isinstance(payload.get("native_modules"), list)
        or not _valid_sha256(payload.get("distribution_inventory_sha256"))
    ):
        raise PythonEnvironmentIdentityError("Python environment closure is incomplete")
    for field in (
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
    ):
        if type(payload.get(field)) is not type(runtime.get(field)) or payload.get(
            field
        ) != runtime.get(field):
            raise PythonEnvironmentIdentityError(
                "Python environment ABI differs from its runtime closure"
            )
    capabilities = payload.get("capabilities")
    if not isinstance(capabilities, Mapping) or set(capabilities) != {
        "schema",
        "runtime",
        "isolated",
        "ignore_environment",
        "no_user_site",
        "safe_path",
        "environment_root_active",
        "required_active_import_roles",
    }:
        raise PythonEnvironmentIdentityError(
            "Python environment capability vector is invalid"
        )
    capabilities = cast(Mapping[str, object], capabilities)
    required_import_roles = capabilities.get("required_active_import_roles")
    if (
        capabilities.get("schema") != PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA
        or capabilities.get("runtime") != runtime.get("capabilities")
        or any(
            capabilities.get(field) is not True
            for field in ("isolated", "ignore_environment", "no_user_site", "safe_path")
        )
        or type(capabilities.get("environment_root_active")) is not bool
        or not isinstance(required_import_roles, list)
        or required_import_roles != sorted(set(required_import_roles))
        or not all(isinstance(role, str) and role for role in required_import_roles)
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment capability vector is invalid"
        )
    tree = payload["tree"]
    assert isinstance(tree, dict)
    if set(tree) != {
        "id",
        "file_count",
        "node_ids",
        "file_nodes",
        "entries",
        "manifest_sha256",
    }:
        raise PythonEnvironmentIdentityError(
            "Python environment tree closure is invalid"
        )
    _tree_nodes, nodes_by_id = _validate_file_nodes(
        tree.get("file_nodes"), label="Python environment"
    )
    tree_entries, _tree_paths, tree_node_ids = _validate_inventory_entries(
        tree.get("entries"),
        label="Python environment",
        nodes=nodes_by_id,
        allow_base_runtime=True,
    )
    tree_by_path = {str(row["path"]): row for row in tree_entries}
    raw_selected = payload.get("selected_executable")
    selected = (
        cast(Mapping[str, object], raw_selected)
        if isinstance(raw_selected, Mapping)
        else None
    )
    selected_tree_row = next(
        (
            row
            for row in tree_entries
            if selected is not None and row.get("path") == selected.get("path")
        ),
        None,
    )
    if (
        tree.get("id") != "environment-root"
        or type(tree.get("file_count")) is not int
        or tree.get("file_count")
        != sum(_is_file_entry(entry) for entry in tree_entries)
        or tree.get("node_ids")
        != sorted(
            tree_node_ids, key=lambda value: int(value.removeprefix("file-node-"))
        )
        or set(nodes_by_id) != tree_node_ids
        or tree.get("manifest_sha256") != canonical_json_sha256(tree.get("entries"))
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment tree closure is invalid"
        )
    pyvenv = payload.get("pyvenv_config")
    selected_kind = selected.get("kind") if selected is not None else None
    if selected is not None and selected_kind == "tree-reference":
        selected_valid = (
            set(selected) == {"path", "kind", "node"}
            and selected_tree_row is not None
            and selected_tree_row.get("node") == selected.get("node")
        )
    elif selected is not None and selected_kind == "runtime-role-reference":
        selected_valid = (
            set(selected) == {"path", "kind", "role"}
            and selected.get("role") == "base-executable"
            and selected_tree_row is not None
            and selected_tree_row.get("kind") == "symlink"
            and selected_tree_row.get("target_owner") == "base-runtime"
            and selected_tree_row.get("target_role") == "base-executable"
        )
    else:
        selected_valid = False
    if (
        selected is None
        or not _valid_relative_payload_path(selected.get("path"))
        or not selected_valid
        or not isinstance(pyvenv, Mapping)
        or set(pyvenv) != {"path", "node"}
        or pyvenv.get("path") != "pyvenv.cfg"
        or tree_by_path.get("pyvenv.cfg", {}).get("kind") not in {"file", "hardlink"}
        or tree_by_path.get("pyvenv.cfg", {}).get("node") != pyvenv.get("node")
    ):
        raise PythonEnvironmentIdentityError("Python environment foundation is invalid")
    scripts_root = payload.get("scripts_root")
    site_roots = payload.get("site_roots")
    directory_paths = {
        str(row["path"]) for row in tree_entries if row.get("kind") == "directory"
    }
    if (
        not _valid_relative_payload_path(scripts_root)
        or scripts_root not in directory_paths
        or not isinstance(site_roots, list)
        or not site_roots
        or not all(
            _valid_relative_payload_path(path) and path in directory_paths
            for path in site_roots
        )
        or len(site_roots) != len(set(str(path).casefold() for path in site_roots))
        or site_roots
        != sorted(site_roots, key=lambda path: (str(path).casefold(), str(path)))
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment scripts/site-root closure is invalid"
        )
    external_roots = payload.get("external_roots")
    if not isinstance(external_roots, list):
        raise PythonEnvironmentIdentityError(
            "Python environment external-root closure is invalid"
        )
    external_root_paths: dict[str, str] = {}
    external_path_identities: set[PurePath] = set()
    for index, row in enumerate(external_roots):
        if not isinstance(row, Mapping) or set(row) != {"id", "path"}:
            raise PythonEnvironmentIdentityError(
                "Python environment external-root closure is invalid"
            )
        root_id = row.get("id")
        path = row.get("path")
        path_identity = canonical_absolute_path(path)
        if (
            root_id != f"external-root-{index}"
            or not isinstance(path, str)
            or any(
                path_identity.is_relative_to(prior)
                or prior.is_relative_to(path_identity)
                for prior in external_path_identities
            )
        ):
            raise PythonEnvironmentIdentityError(
                "Python environment external-root closure is invalid"
            )
        external_root_paths[str(root_id)] = path
        external_path_identities.add(path_identity)

    def external_order(row: Mapping[str, object]) -> tuple[str, str]:
        path = canonical_absolute_path(row["path"])
        raw = str(path)
        return (raw.lower() if path.drive else raw, raw)

    if external_roots != sorted(
        cast(list[Mapping[str, object]], external_roots),
        key=external_order,
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment external-root closure is not canonical"
        )
    active_import_roots = payload.get("active_import_roots")
    if not isinstance(active_import_roots, list):
        raise PythonEnvironmentIdentityError(
            "Python environment active import-root closure is invalid"
        )
    active_roles: set[str] = set()
    external_roles: list[str] = []
    runtime_imports = cast(list[Mapping[str, object]], runtime["import_roots"])
    for row in active_import_roots:
        if not isinstance(row, Mapping):
            raise PythonEnvironmentIdentityError(
                "Python environment active import-root closure is invalid"
            )
        role = row.get("role")
        owner = row.get("owner")
        valid = False
        if (
            isinstance(role, str)
            and role.startswith("runtime-import-")
            and owner == "runtime"
        ):
            try:
                index = int(role.removeprefix("runtime-import-"))
            except ValueError:
                index = -1
            valid = 0 <= index < len(runtime_imports) and {
                key: value for key, value in row.items() if key not in {"role", "owner"}
            } == dict(runtime_imports[index])
        elif role == "environment-root" and owner == "environment":
            valid = (
                capabilities.get("environment_root_active") is True
                and set(row) == {"role", "owner", "kind", "path"}
                and row.get("kind") == "directory"
                and row.get("path") == "."
            )
        elif (
            isinstance(role, str)
            and role.startswith("site-root-")
            and owner == "environment"
        ):
            try:
                index = int(role.removeprefix("site-root-"))
            except ValueError:
                index = -1
            valid = (
                0 <= index < len(site_roots)
                and set(row) == {"role", "owner", "kind", "path"}
                and row.get("kind") == "directory"
                and row.get("path") == site_roots[index]
            )
        elif (
            isinstance(role, str)
            and role.startswith("external-import-")
            and owner == "external"
        ):
            try:
                index = int(role.removeprefix("external-import-"))
            except ValueError:
                index = -1
            root_id = row.get("root")
            path = row.get("path")
            valid = (
                index == len(external_roles)
                and set(row) == {"role", "owner", "kind", "root", "path"}
                and row.get("kind") == "directory"
                and isinstance(root_id, str)
                and root_id in external_root_paths
                and isinstance(path, str)
                and (path == "." or _valid_relative_payload_path(path))
            )
            if valid:
                external_roles.append(role)
        if not valid or str(role) in active_roles:
            raise PythonEnvironmentIdentityError(
                "Python environment active import-root closure is invalid"
            )
        active_roles.add(str(role))
    expected_required_roles = {
        *(f"runtime-import-{index}" for index in range(len(runtime_imports))),
        *(f"site-root-{index}" for index in range(len(site_roots))),
    }
    expected_required_roles.update(external_roles)
    if capabilities.get("environment_root_active") is True:
        expected_required_roles.add("environment-root")
    if active_roles != expected_required_roles or required_import_roles != sorted(
        expected_required_roles
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment active import-root roles are incomplete"
        )
    distributions = payload["distributions"]
    assert isinstance(distributions, list)
    if payload.get("distribution_inventory_sha256") != canonical_json_sha256(
        distributions
    ):
        raise PythonEnvironmentIdentityError(
            "Python distribution inventory digest is invalid"
        )
    distribution_names: set[str] = set()
    installed_owners: dict[str, str] = {}
    expected_console_scripts: dict[str, list[str]] = {}
    for row in tree_entries:
        if row.get("kind") != "symlink" or row.get("target_owner") != "base-runtime":
            continue
        if row.get("target_role") != "base-executable":
            raise PythonEnvironmentIdentityError(
                "Python environment base-runtime symlink is invalid"
            )
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            raise PythonEnvironmentIdentityError(
                "Python distribution identity is invalid"
            )
        distribution = cast(Mapping[str, object], distribution)
        name = distribution.get("name")
        version_value = distribution.get("version")
        installed = distribution.get("installed_files")
        entry_points = distribution.get("entry_points")
        if (
            set(distribution) != _DISTRIBUTION_IDENTITY_FIELDS
            or not isinstance(name, str)
            or not name
            or name != _canonicalize_name(name)
            or name in distribution_names
            or not isinstance(version_value, str)
            or not version_value
            or not isinstance(installed, list)
            or not isinstance(entry_points, list)
            or type(distribution.get("installed_file_count")) is not int
            or distribution.get("installed_file_count") != len(installed)
            or distribution.get("file_manifest_sha256")
            != canonical_json_sha256(installed)
            or distribution.get("entry_points_sha256")
            != canonical_json_sha256(entry_points)
            or any(
                not _valid_sha256(distribution.get(field))
                for field in ("record_sha256", "direct_url_sha256", "installer_sha256")
            )
        ):
            raise PythonEnvironmentIdentityError(
                "Python distribution identity is invalid"
            )
        distribution_names.add(name)
        external_source = distribution.get("external_source")
        if external_source is None:
            if distribution.get("direct_url_sha256") != _EMPTY_SHA256:
                raise PythonEnvironmentIdentityError(
                    "Python distribution direct-url identity is invalid"
                )
        elif (
            not isinstance(external_source, Mapping)
            or set(external_source) != {"root", "path", "import_roles"}
            or external_source.get("root") not in external_root_paths
            or not isinstance(external_source.get("path"), str)
            or (
                external_source.get("path") != "."
                and not _valid_relative_payload_path(external_source.get("path"))
            )
            or distribution.get("direct_url_sha256") == _EMPTY_SHA256
        ):
            raise PythonEnvironmentIdentityError(
                "Python distribution external-source identity is invalid"
            )
        installed_paths: set[str] = set()
        direct_url_nodes: list[Mapping[str, object]] = []
        for row in installed:
            if not isinstance(row, Mapping):
                raise PythonEnvironmentIdentityError(
                    "Python distribution installed-file identity is invalid"
                )
            row = cast(Mapping[str, object], row)
            tree_row = tree_by_path.get(str(row.get("path")))
            node_id = row.get("node")
            node = nodes_by_id.get(str(node_id))
            if (
                set(row) != {"path", "node", "declared"}
                or not _valid_relative_payload_path(row.get("path"))
                or row.get("path") in installed_paths
                or node is None
                or tree_row is None
                or not _is_file_entry(tree_row)
                or tree_row.get("node") != node_id
                or not _declared_sha256_matches(row.get("declared"), node.get("sha256"))
            ):
                raise PythonEnvironmentIdentityError(
                    "Python distribution installed-file identity is invalid"
                )
            installed_path = str(row["path"])
            previous_owner = installed_owners.get(installed_path)
            if previous_owner is not None:
                raise PythonEnvironmentIdentityError(
                    "Python distributions claim the same installed file: "
                    f"{previous_owner}, {name}: {installed_path}"
                )
            installed_owners[installed_path] = name
            installed_paths.add(installed_path)
            if installed_path.casefold().endswith("/direct_url.json"):
                direct_url_nodes.append(node)
        if external_source is None:
            if direct_url_nodes:
                raise PythonEnvironmentIdentityError(
                    "Python distribution has an undeclared direct-url source"
                )
        elif len(direct_url_nodes) != 1 or direct_url_nodes[0].get(
            "sha256"
        ) != distribution.get("direct_url_sha256"):
            raise PythonEnvironmentIdentityError(
                "Python distribution direct-url file identity is invalid"
            )
        if installed != sorted(
            installed,
            key=lambda row: (str(row["path"]).casefold(), str(row["path"])),
        ):
            raise PythonEnvironmentIdentityError(
                "Python distribution installed-file identity is not canonical"
            )
        console_entry_points: set[str] = set()
        for entry_point in entry_points:
            if not isinstance(entry_point, Mapping):
                raise PythonEnvironmentIdentityError(
                    "Python distribution entry-point identity is invalid"
                )
            entry_point = cast(Mapping[str, object], entry_point)
            if set(entry_point) != {"group", "name", "value"} or not all(
                isinstance(entry_point.get(field), str) and entry_point.get(field)
                for field in ("group", "name", "value")
            ):
                raise PythonEnvironmentIdentityError(
                    "Python distribution entry-point identity is invalid"
                )
            if entry_point["group"] == "console_scripts":
                script_name = str(entry_point["name"])
                _console_script_filenames(script_name)
                if script_name in console_entry_points:
                    raise PythonEnvironmentIdentityError(
                        "Python distribution repeats a console entry point"
                    )
                console_entry_points.add(script_name)
        if entry_points != sorted(
            entry_points,
            key=lambda item: (
                str(item["group"]),
                str(item["name"]),
                str(item["value"]),
            ),
        ):
            raise PythonEnvironmentIdentityError(
                "Python distribution entry-point identity is not canonical"
            )
        console_scripts = distribution.get("console_scripts")
        if (
            not isinstance(console_scripts, Mapping)
            or set(console_scripts) != console_entry_points
        ):
            raise PythonEnvironmentIdentityError(
                "Python distribution console-script identity is invalid"
            )
        console_scripts = cast(Mapping[str, object], console_scripts)
        for script in sorted(console_entry_points):
            paths = console_scripts[script]
            derived_paths = [
                f"{scripts_root}/{filename}"
                for filename in _console_script_filenames(script)
                if _is_file_entry(
                    tree_by_path.get(f"{scripts_root}/{filename}", {"kind": "absent"})
                )
            ]
            if (
                script in expected_console_scripts
                or not isinstance(paths, list)
                or not paths
                or paths != derived_paths
            ):
                raise PythonEnvironmentIdentityError(
                    "Python distribution console-script identity is invalid"
                )
            expected_console_scripts[script] = derived_paths
    if distributions != sorted(
        distributions,
        key=lambda row: (str(row["name"]), str(row["version"])),
    ):
        raise PythonEnvironmentIdentityError(
            "Python distribution inventory is not canonical"
        )
    site_bootstrap_files = payload["site_bootstrap_files"]
    if not isinstance(site_bootstrap_files, list):
        raise PythonEnvironmentIdentityError(
            "Python environment site bootstrap closure is invalid"
        )
    bootstrap_paths: list[str] = []
    for item in site_bootstrap_files:
        if not isinstance(item, Mapping) or set(item) != {"path", "node"}:
            raise PythonEnvironmentIdentityError(
                "Python environment site bootstrap closure is invalid"
            )
        path = item.get("path")
        node = item.get("node")
        if (
            not isinstance(path, str)
            or not _valid_relative_payload_path(path)
            or path in bootstrap_paths
            or path in installed_owners
            or not any(
                path == site or path.startswith(f"{site}/") for site in site_roots
            )
            or not isinstance(node, str)
            or node not in nodes_by_id
        ):
            raise PythonEnvironmentIdentityError(
                "Python environment site bootstrap closure is invalid"
            )
        entry = tree_by_path.get(path)
        if (
            entry is None
            or entry.get("kind") not in {"file", "hardlink"}
            or entry.get("node") != node
        ):
            raise PythonEnvironmentIdentityError(
                "Python environment site bootstrap closure is invalid"
            )
        bootstrap_paths.append(path)
    if bootstrap_paths != sorted(
        bootstrap_paths, key=lambda value: (value.casefold(), value)
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment site bootstrap closure is not canonical"
        )
    validate_external_import_custody(
        payload.get("external_import_custody"),
        external_roots=external_root_paths,
        active_import_roots=cast(Sequence[Mapping[str, object]], active_import_roots),
        distributions=cast(Sequence[Mapping[str, object]], distributions),
        tree_entries=tree_by_path,
        tree_nodes=nodes_by_id,
        site_roots=cast(Sequence[str], site_roots),
        bootstrap_paths=bootstrap_paths,
    )
    if payload.get("console_scripts") != expected_console_scripts:
        raise PythonEnvironmentIdentityError(
            "Python environment console-script closure is invalid"
        )
    expected_native = [
        {
            "path": str(row["path"]),
            "node": row["node"],
        }
        for row in tree_entries
        if row.get("kind") in {"file", "hardlink", "symlink"}
        and "node" in row
        and str(row.get("path", "")).endswith(
            tuple(importlib.machinery.EXTENSION_SUFFIXES)
        )
    ]
    if payload.get("native_modules") != expected_native:
        raise PythonEnvironmentIdentityError(
            "Python environment native-module closure is invalid"
        )
    return payload
