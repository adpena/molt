"""Installed compiler identity, independent of guest optimization policy.

The signed bundle owns this manifest. Its source inventory is a projection of
one Git commit; hashes detect local damage, not authenticate an unsigned bundle.
It also declares the exact prebuilt runtime cells (native staticlib/link
closure with the archive-derived callable projection code generation consumes,
and WASM pairs) under ``runtime/``. Installed compilation selects only
those cells and never builds Molt or its Rust runtime. The same bundle tree is
installed by release archives, package managers and platform wheels.
"""

from __future__ import annotations

from dataclasses import dataclass
from concurrent.futures import ThreadPoolExecutor
from collections.abc import Mapping, Sequence
import os
from pathlib import Path
import re
from typing import Any

from molt.exact_json import canonical_json_sha256, read_exact
from molt.file_publication import is_link_like, resolve_owned_path
from molt.portable_paths import (
    portable_path_component,
    portable_path_identity,
    portable_relative_path,
)
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from molt.source_root import MANIFEST_NAME, installed_distribution_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_content_identity,
    stable_regular_file_identity,
)
from molt.verified_subset import current_host_coordinate

MANIFEST_SCHEMA = "molt.release-compiler-source.v5"
PRODUCTION_COMPILER_PROFILE = "release"
PRODUCTION_COMPILER_FEATURES = (
    "llvm",
    "luau-backend",
    "native-backend",
    "rust-backend",
    "wasm-backend",
)
MAX_SOURCE_FILES = 25_000
MAX_SOURCE_BYTES = 2 * 1024 * 1024 * 1024


def _source_mode_matches_git(actual: int, recorded: int) -> bool:
    """Whether a projected file's permissions agree with its Git mode.

    Git records only the executable bit. A checkout or an installer applies the
    user's umask to the rest (pip writes ``0o666 & ~umask``, so umask 002
    gives 664 for a recorded 644), and the content digest already binds the
    bytes. A file must keep the owner's executable bit as recorded and carry
    no setuid, setgid or sticky bit.
    """

    actual &= 0o7777
    return not actual & 0o7000 and bool(actual & 0o100) == bool(recorded & 0o100)


def verify_source_inventory(
    root: Path,
    files: Sequence[Mapping[str, Any]],
    *,
    manifest_name: str | None = None,
) -> Path:
    """Verify the same exact source projection before packaging and after install.

    Callers admit the inventory schema first. The optional root manifest is
    metadata outside its own inventory, never a blanket directory exclusion.
    """
    root = resolve_owned_path(root)
    expected = {entry["path"]: entry for entry in files}
    directories = {
        parent.as_posix()
        for name in expected
        for parent in Path(name).parents
        if parent != Path(".")
    }
    actual_files: set[str] = set()
    actual_dirs: set[str] = set()

    def scan_error(error: OSError) -> None:
        raise error

    for current, dirs, names in os.walk(root, followlinks=False, onerror=scan_error):
        for name in dirs + names:
            path = Path(current) / name
            if is_link_like(path):
                raise ValueError(f"Compiler source contains a link: {path}")
        actual_dirs.update(
            (Path(current) / name).relative_to(root).as_posix() for name in dirs
        )
        for name in names:
            path = Path(current) / name
            relative = path.relative_to(root).as_posix()
            if not path.is_file():
                raise ValueError(f"Compiler source is not a regular file: {relative}")
            actual_files.add(relative)
    allowed = set(expected)
    if manifest_name is not None:
        allowed.add(manifest_name)
    if actual_files != allowed or actual_dirs != directories:
        raise ValueError("Compiler source file/directory closure is not exact")
    # Windows records no POSIX permission bits to compare.
    check_modes = os.name != "nt"

    def verify_file(name: str) -> None:
        entry = expected[name]
        path = root / name
        identity = stable_regular_file_content_identity(path, label="compiler source")
        if any(identity[key] != entry[key] for key in ("size", "sha256")):
            raise ValueError(f"Compiler source content changed: {name}")
        if check_modes:
            mode = path.stat().st_mode
            if not _source_mode_matches_git(mode, entry["mode"]):
                raise ValueError(
                    f"Compiler source mode changed: {name} (found "
                    f"{mode & 0o7777:o}, recorded {entry['mode'] & 0o777:o})"
                )

    with ThreadPoolExecutor(
        max_workers=min(16, max(1, len(expected))),
        thread_name_prefix="compiler-source-verify",
    ) as executor:
        tuple(executor.map(verify_file, expected, chunksize=32))
    return root


def _digest(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def validate_compiler_record(record: object) -> dict[str, Any]:
    if (
        not isinstance(record, dict)
        or set(record)
        != {"path", "sha256", "size", "profile", "features", "platform", "arch"}
        or not _digest(record.get("sha256"))
        or type(record.get("size")) is not int
        or record["size"] <= 0
        or record.get("profile") != PRODUCTION_COMPILER_PROFILE
        or record.get("features") != list(PRODUCTION_COMPILER_FEATURES)
        or not isinstance(record.get("platform"), str)
        or not isinstance(record.get("arch"), str)
        or (record["platform"], record["arch"]) not in RUST_TARGET_BY_COORDINATE
    ):
        raise ValueError("invalid production compiler identity")
    expected_name = (
        "molt-backend.exe" if record["platform"] == "windows" else "molt-backend"
    )
    if record["path"] != f"bin/{expected_name}":
        raise ValueError("invalid production compiler executable path")
    return record


def validate_launcher_record(record: object) -> dict[str, Any]:
    if (
        not isinstance(record, dict)
        or set(record) != {"path", "sha256", "size", "platform", "arch"}
        or not _digest(record.get("sha256"))
        or type(record.get("size")) is not int
        or record["size"] <= 0
        or not isinstance(record.get("platform"), str)
        or not isinstance(record.get("arch"), str)
        or (record.get("platform"), record.get("arch")) not in RUST_TARGET_BY_COORDINATE
        or record["path"]
        != ("bin/molt.exe" if record["platform"] == "windows" else "bin/molt")
    ):
        raise ValueError("invalid production launcher identity")
    return record


RUNTIME_INVENTORY_SCHEMA = "molt.installed-runtime.v2"
RUNTIME_CELL_SCHEMA = "molt.installed-runtime-cell.v2"
RUNTIME_ROOT = "runtime"
# Complete top-level bundle projection shared by its producer and installers.
COMPILER_BUNDLE_DIRECTORIES = ("bin", "source", RUNTIME_ROOT, "share")
NATIVE_RUNTIME_CELL = "native-staticlib"
WASM_RUNTIME_CELL = "wasm-pair"
# The archive-derived ``molt_*`` callable projection native code generation
# consumes. It is content-addressed to its archive and shipped, so installed
# compilation never discovers or runs a symbol reader.
NATIVE_CALLABLE_PROJECTION_ROLE = "runtime_callable_symbols"
MAX_RUNTIME_CELLS = 512
# kind -> (key fields, required roles, optional roles). Build identity is not
# restated here: it lives in each cell's canonical receipt (native-link
# manifest / WASM generation), whose bytes the file records pin.
_RUNTIME_CELL_POLICY: dict[
    str, tuple[frozenset[str], frozenset[str], frozenset[str]]
] = {
    NATIVE_RUNTIME_CELL: (
        frozenset(
            {"target_triple", "cargo_profile", "stdlib_profile", "runtime_features"}
        ),
        frozenset(
            {
                "runtime_archive",
                "native_link_manifest",
                NATIVE_CALLABLE_PROJECTION_ROLE,
            }
        ),
        frozenset({"native_link_custody_archive"}),
    ),
    WASM_RUNTIME_CELL: (
        frozenset(
            {
                "target_triple",
                "cargo_profile",
                "stdlib_profile",
                "runtime_features",
                "simd",
                "freestanding",
            }
        ),
        frozenset(
            {"wasm_generation_manifest", "wasm_shared_member", "wasm_reloc_member"}
        ),
        frozenset(),
    ),
}


def runtime_cell_id(
    kind: str, key: Mapping[str, Any], files: Sequence[Mapping[str, Any]]
) -> str:
    """Content address of one cell; it names the cell's bundle/retention directory."""
    return canonical_json_sha256(
        {
            "schema": RUNTIME_CELL_SCHEMA,
            "kind": kind,
            "key": dict(key),
            "files": [dict(entry) for entry in files],
        }
    )


def _validate_runtime_key(kind: str, key: object) -> None:
    if not isinstance(key, dict) or set(key) != _RUNTIME_CELL_POLICY[kind][0]:
        raise ValueError("invalid installed runtime cell key")
    for name in ("target_triple", "cargo_profile", "stdlib_profile"):
        value = key[name]
        if (
            not isinstance(value, str)
            or re.fullmatch(r"[A-Za-z0-9_.-]+", value) is None
        ):
            raise ValueError("invalid installed runtime cell key")
    features = key["runtime_features"]
    if (
        not isinstance(features, list)
        or not all(isinstance(item, str) and item for item in features)
        or features != sorted(set(features))
    ):
        raise ValueError("invalid installed runtime cell features")
    if kind == WASM_RUNTIME_CELL and (
        key["target_triple"] != "wasm32-wasip1"
        or type(key["simd"]) is not bool
        or type(key["freestanding"]) is not bool
    ):
        raise ValueError("invalid installed WASM runtime cell key")


def validate_runtime_inventory(
    record: object, *, platform: str, arch: str
) -> dict[str, Any]:
    """Admit the exact typed runtime cell inventory of one release target."""
    if (
        not isinstance(record, dict)
        or set(record) != {"schema", "platform", "arch", "source", "cells"}
        or record["schema"] != RUNTIME_INVENTORY_SCHEMA
        or (record["platform"], record["arch"]) != (platform, arch)
        or not isinstance(record["cells"], list)
        or len(record["cells"]) > MAX_RUNTIME_CELLS
    ):
        raise ValueError("invalid installed runtime inventory")
    source = record["source"]
    length = (
        {"sha1": 40, "sha256": 64}.get(source.get("object_format"))
        if isinstance(source, dict)
        else None
    )
    if (
        not isinstance(source, dict)
        or set(source) != {"object_format", "commit", "tree"}
        or length is None
        or any(
            not isinstance(source[key], str)
            or re.fullmatch(f"[0-9a-f]{{{length}}}", source[key]) is None
            for key in ("commit", "tree")
        )
    ):
        raise ValueError("invalid installed runtime source identity")
    last = ""
    keys: set[str] = set()
    for cell in record["cells"]:
        if (
            not isinstance(cell, dict)
            or set(cell) != {"id", "kind", "key", "files"}
            or cell["kind"] not in _RUNTIME_CELL_POLICY
        ):
            raise ValueError("invalid installed runtime cell")
        _fields, required, optional = _RUNTIME_CELL_POLICY[cell["kind"]]
        _validate_runtime_key(cell["kind"], cell["key"])
        files = cell["files"]
        if not isinstance(files, list):
            raise ValueError("invalid installed runtime cell files")
        roles: list[str] = []
        names: list[str] = []
        for entry in files:
            if (
                not isinstance(entry, dict)
                or set(entry) != {"role", "name", "sha256", "size"}
                or not isinstance(entry["role"], str)
                or not isinstance(entry["name"], str)
                or not _digest(entry["sha256"])
                or type(entry["size"]) is not int
                or entry["size"] <= 0
                or portable_path_component(entry["name"]) != entry["name"]
            ):
                raise ValueError("invalid installed runtime cell file")
            roles.append(entry["role"])
            names.append(entry["name"])
        if (
            len(set(roles)) != len(roles)
            or not required <= set(roles) <= required | optional
            or names != sorted(set(names))
            or len({portable_path_identity(name) for name in names}) != len(names)
        ):
            raise ValueError("installed runtime cell roles or names are not exact")
        if not _digest(cell["id"]) or cell["id"] != runtime_cell_id(
            cell["kind"], cell["key"], files
        ):
            raise ValueError("installed runtime cell id is not its content address")
        if cell["id"] <= last:
            raise ValueError("installed runtime cells are not canonically ordered")
        selector = canonical_json_sha256({"kind": cell["kind"], "key": cell["key"]})
        if selector in keys:
            raise ValueError("installed runtime inventory has duplicate cell keys")
        keys.add(selector)
        last = cell["id"]
    return record


def verify_runtime_member(
    cell_root: Path, cell: Mapping[str, Any], role: str
) -> StableRegularFileIdentity:
    """Admit one runtime cell member by content before any consumer reads it."""
    entry = next((item for item in cell["files"] if item["role"] == role), None)
    if entry is None:
        raise ValueError(f"Runtime cell has no {role}")
    path = resolve_owned_path(cell_root / entry["name"])
    identity = stable_regular_file_identity(path, label="installed runtime")
    if (identity.sha256, identity.size) != (entry["sha256"], entry["size"]):
        raise ValueError("Runtime cell member differs from its release manifest")
    return identity


def verify_runtime_tree(
    root: Path,
    inventory: Mapping[str, Any],
    *,
    extra_names: frozenset[str] = frozenset(),
) -> None:
    """Verify the exact runtime cell closure and member bytes under ``root``."""
    expected = {
        f"{cell['id']}/{entry['name']}": entry
        for cell in inventory["cells"]
        for entry in cell["files"]
    }
    if not root.exists() and not is_link_like(root):
        if expected or extra_names:
            raise ValueError("Installed runtime artifacts are missing; reinstall Molt")
        return
    root = resolve_owned_path(root)
    actual_files: set[str] = set()
    actual_dirs: set[str] = set()

    def scan_error(error: OSError) -> None:
        raise error

    for current, dirs, names in os.walk(root, followlinks=False, onerror=scan_error):
        for name in dirs + names:
            if is_link_like(Path(current) / name):
                raise ValueError(f"Installed runtime contains a link: {name}")
        actual_dirs.update(
            (Path(current) / name).relative_to(root).as_posix() for name in dirs
        )
        for name in names:
            path = Path(current) / name
            if not path.is_file():
                raise ValueError(f"Installed runtime member is not a file: {name}")
            actual_files.add(path.relative_to(root).as_posix())
    if actual_files != set(expected) | set(extra_names) or actual_dirs != {
        cell["id"] for cell in inventory["cells"]
    }:
        raise ValueError("Installed runtime file/directory closure is not exact")
    for name, entry in expected.items():
        identity = stable_regular_file_content_identity(
            root / name, label="installed runtime"
        )
        if (identity["sha256"], identity["size"]) != (entry["sha256"], entry["size"]):
            raise ValueError(f"Installed runtime content changed: {name}")


@dataclass(frozen=True)
class InstalledCompiler:
    source_root: Path
    source_sha: str
    record: dict[str, Any]
    launcher: dict[str, Any]
    files: tuple[dict[str, Any], ...]
    runtime: dict[str, Any]

    @property
    def binary(self) -> Path:
        return self.source_root.parent / self.record["path"]

    def verify_launcher(self) -> dict[str, str | int]:
        if current_host_coordinate() != (
            self.launcher["platform"],
            self.launcher["arch"],
        ):
            raise ValueError("Installed launcher does not match this host")
        identity = stable_regular_file_content_identity(
            resolve_owned_path(self.source_root.parent / self.launcher["path"]),
            label="installed launcher",
        )
        if any(identity[key] != self.launcher[key] for key in ("sha256", "size")):
            raise ValueError("Installed launcher differs from its release manifest")
        return identity

    def verify_binary(
        self, features: tuple[str, ...], cargo_profile: str
    ) -> dict[str, str | int]:
        if cargo_profile != self.record["profile"]:
            raise ValueError(
                "Installed Molt uses its production compiler; host compiler profile "
                "overrides require a source checkout. Use --profile for your program."
            )
        missing = set(features).difference(self.record["features"])
        if missing:
            raise ValueError(
                "Installed compiler does not provide: " + ", ".join(sorted(missing))
            )
        if current_host_coordinate() != (self.record["platform"], self.record["arch"]):
            raise ValueError(
                "Installed compiler does not match this host OS/architecture"
            )
        identity = stable_regular_file_content_identity(
            resolve_owned_path(self.binary), label="installed compiler"
        )
        if any(identity[key] != self.record[key] for key in ("sha256", "size")):
            raise ValueError(
                "Installed compiler content differs from its release manifest"
            )
        return identity

    def verify_sources(self) -> None:
        self.verify_launcher()
        verify_source_inventory(
            self.source_root, self.files, manifest_name=MANIFEST_NAME
        )

    @property
    def runtime_root(self) -> Path:
        return self.source_root.parent / RUNTIME_ROOT

    def verify_runtime_file(self, cell: Mapping[str, Any], role: str) -> Path:
        """Admit one shipped cell member by content before any consumer reads it."""
        return verify_runtime_member(self.runtime_root / cell["id"], cell, role).path

    def verify_runtime(self) -> None:
        verify_runtime_tree(self.runtime_root, self.runtime)

    def verify_executing_package(self) -> None:
        """Bind a separately installed ``molt`` package (pip) to the signed source.

        A bundle executes its own verified ``source/src``. A platform wheel's
        package lives in site-packages; every file it contains must be the
        release source file of the same path. Bytecode caches are not source.
        """
        package = Path(__file__).resolve().parent
        if resolve_owned_path(self.source_root) in package.parents:
            return
        inventory = {entry["path"]: entry for entry in self.files}
        for current, dirs, names in os.walk(package, followlinks=False):
            dirs[:] = [name for name in dirs if name != "__pycache__"]
            for name in dirs + names:
                if is_link_like(Path(current) / name):
                    raise ValueError(f"Installed Molt package contains a link: {name}")
            for name in names:
                path = Path(current) / name
                relative = "src/molt/" + path.relative_to(package).as_posix()
                entry = inventory.get(relative)
                if entry is None:
                    raise ValueError(
                        f"Installed Molt package file is not release source: {relative}"
                    )
                identity = stable_regular_file_content_identity(
                    path, label="installed Molt package"
                )
                if (identity["sha256"], identity["size"]) != (
                    entry["sha256"],
                    entry["size"],
                ):
                    raise ValueError(
                        f"Installed Molt package differs from its release source: {relative}"
                    )


def installed_compiler(source_root: Path) -> InstalledCompiler | None:
    manifest_path = source_root / MANIFEST_NAME
    if not manifest_path.exists() and not is_link_like(manifest_path):
        # A damaged installation is installed damage, never source mode.
        if installed_distribution_root(source_root) is not None:
            raise ValueError("Installed compiler manifest is missing; reinstall Molt")
        return None
    source_root = resolve_owned_path(source_root)
    payload = read_exact(
        manifest_path, max_bytes=16 * 1024 * 1024, label="installed compiler manifest"
    )
    if (
        not isinstance(payload, dict)
        or set(payload) != {"schema", "git", "files", "compiler", "launcher", "runtime"}
        or payload["schema"] != MANIFEST_SCHEMA
    ):
        raise ValueError("Invalid installed compiler manifest")
    git = payload["git"]
    if not isinstance(git, dict) or set(git) != {"object_format", "commit", "tree"}:
        raise ValueError("Invalid installed compiler source identity")
    if not isinstance(git["object_format"], str):
        raise ValueError("Invalid installed compiler Git object format")
    length = {"sha1": 40, "sha256": 64}.get(git["object_format"])
    if length is None or any(
        not isinstance(git[key], str)
        or re.fullmatch(f"[0-9a-f]{{{length}}}", git[key]) is None
        for key in ("commit", "tree")
    ):
        raise ValueError("Invalid installed compiler Git identity")
    files = payload["files"]
    if not isinstance(files, list) or not 0 < len(files) <= MAX_SOURCE_FILES:
        raise ValueError("Invalid installed compiler source inventory")
    seen: set[str] = set()
    total = 0
    last = ""
    for entry in files:
        if not isinstance(entry, dict) or set(entry) != {
            "path",
            "mode",
            "blob_oid",
            "size",
            "sha256",
        }:
            raise ValueError("Invalid installed compiler source record")
        path = portable_relative_path(entry["path"]).as_posix()
        identity = portable_path_identity(path)
        if identity in seen or path <= last or path == MANIFEST_NAME:
            raise ValueError("Installed compiler source paths collide or are unordered")
        if (
            type(entry["size"]) is not int
            or entry["size"] < 0
            or type(entry["mode"]) is not int
            or entry["mode"] not in {0o100644, 0o100755}
            or not _digest(entry["sha256"])
            or not isinstance(entry["blob_oid"], str)
            or re.fullmatch(f"[0-9a-f]{{{length}}}", entry["blob_oid"]) is None
        ):
            raise ValueError("Invalid installed compiler source content identity")
        total += entry["size"]
        if total > MAX_SOURCE_BYTES:
            raise ValueError("Installed compiler source exceeds size policy")
        seen.add(identity)
        last = path
    compiler = validate_compiler_record(payload["compiler"])
    runtime = validate_runtime_inventory(
        payload["runtime"], platform=compiler["platform"], arch=compiler["arch"]
    )
    if runtime["source"] != git:
        raise ValueError("Installed runtime cells come from different source")
    return InstalledCompiler(
        source_root,
        git["commit"],
        compiler,
        validate_launcher_record(payload["launcher"]),
        tuple(files),
        runtime,
    )
