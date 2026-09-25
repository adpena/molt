"""Installed compiler identity, independent of guest optimization policy.

The signed bundle owns this manifest. Its source inventory is a projection of
one Git commit; hashes detect local damage, not authenticate an unsigned bundle.
"""

from __future__ import annotations

from dataclasses import dataclass
from concurrent.futures import ThreadPoolExecutor
from collections.abc import Mapping, Sequence
import os
from pathlib import Path
import re
from typing import Any

from molt.exact_json import read_exact
from molt.file_publication import is_link_like, resolve_owned_path
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from molt.toolchain_identity import stable_regular_file_content_identity
from molt.verified_subset import current_host_coordinate

MANIFEST_NAME = "release-compiler-source.json"
MANIFEST_SCHEMA = "molt.release-compiler-source.v4"
PRODUCTION_COMPILER_PROFILE = "release"
PRODUCTION_COMPILER_FEATURES = (
    "luau-backend",
    "native-backend",
    "rust-backend",
    "wasm-backend",
)
MAX_SOURCE_FILES = 25_000
MAX_SOURCE_BYTES = 2 * 1024 * 1024 * 1024


def _source_mode_matches_git(actual: int, recorded: int) -> bool:
    actual &= 0o7777
    expected = recorded & 0o777
    # The archive/install umask may remove access, but not add permissions or
    # erase the owner's Git-tracked executable bit.
    return not (actual & ~expected) and bool(actual & 0o100) == bool(expected & 0o100)


def verify_source_inventory(
    root: Path,
    files: Sequence[Mapping[str, Any]],
    *,
    manifest_name: str | None = None,
    verify_modes: bool | None = None,
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
    check_modes = os.name != "nt" if verify_modes is None else verify_modes

    def verify_file(name: str) -> None:
        entry = expected[name]
        path = root / name
        identity = stable_regular_file_content_identity(path, label="compiler source")
        if any(identity[key] != entry[key] for key in ("size", "sha256")):
            raise ValueError(f"Compiler source content changed: {name}")
        if check_modes and not _source_mode_matches_git(
            path.stat().st_mode, entry["mode"]
        ):
            raise ValueError(f"Compiler source mode changed: {name}")

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


@dataclass(frozen=True)
class InstalledCompiler:
    source_root: Path
    source_sha: str
    record: dict[str, Any]
    launcher: dict[str, Any]
    files: tuple[dict[str, Any], ...]

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


def installed_compiler(source_root: Path) -> InstalledCompiler | None:
    manifest_path = source_root / MANIFEST_NAME
    if not manifest_path.exists() and not is_link_like(manifest_path):
        bundle = os.environ.get("MOLT_BUNDLE_ROOT")
        if bundle and source_root.resolve() == (Path(bundle) / "source").resolve():
            raise ValueError(
                "Installed compiler manifest is missing; reinstall the bundle"
            )
        return None
    source_root = resolve_owned_path(source_root)
    payload = read_exact(
        manifest_path, max_bytes=16 * 1024 * 1024, label="installed compiler manifest"
    )
    if (
        not isinstance(payload, dict)
        or set(payload) != {"schema", "git", "files", "compiler", "launcher"}
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
    return InstalledCompiler(
        source_root,
        git["commit"],
        validate_compiler_record(payload["compiler"]),
        validate_launcher_record(payload["launcher"]),
        tuple(files),
    )
