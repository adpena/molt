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

from dataclasses import dataclass, replace
from collections.abc import Generator, Mapping, Sequence
from contextlib import contextmanager
from contextvars import ContextVar
from copy import deepcopy
from enum import Enum
import os
from pathlib import Path
import re
import stat
from types import MappingProxyType
from typing import Any, TypedDict

from molt.exact_json import canonical_json_sha256, loads_exact
from molt.file_publication import (
    is_link_like,
    metadata_is_link_like,
    resolve_owned_path,
)
from molt.host_tool_products import HostToolProduct
from molt.portable_paths import (
    portable_path_component,
    portable_path_identity,
)
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from molt.source_root import MANIFEST_NAME, installed_distribution_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    open_stable_regular_file,
    stable_regular_file_content_identity,
    stable_regular_file_handle_identity,
    stable_regular_file_identity,
    StableRegularFileObservation,
    verify_stable_regular_file_identity,
)
from molt.verified_subset import current_host_coordinate

MANIFEST_SCHEMA = "molt.release-compiler-source.v8"
STDLIB_SOURCE_SUMMARIES_PATH = "share/molt/stdlib-source-summaries.zip"
MAX_STDLIB_SOURCE_SUMMARIES_BYTES = 64 * 1024 * 1024
_STDLIB_SUMMARIES_LABEL = "installed stdlib source summaries"
PRODUCTION_COMPILER_PROFILE = "release"
PRODUCTION_COMPILER_FEATURES = (
    "llvm",
    "native-backend",
)
MAX_SOURCE_FILES = 25_000
MAX_SOURCE_BYTES = 2 * 1024 * 1024 * 1024


class StdlibSourceSummariesRecord(TypedDict):
    path: str
    size: int
    sha256: str


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
    """Hash the same exact source closure before packaging and after install.

    Enumeration owns one no-follow metadata row per member. Hashing binds
    that row to the opened file through the shared stable-file authority;
    directory fences reject observed membership-generation changes. The
    optional root manifest participates in closure only.
    """
    if len(files) > MAX_SOURCE_FILES:
        raise ValueError("Compiler source exceeds file count policy")
    return _verify_source_files(
        root,
        {entry["path"]: entry for entry in files},
        scope=_SourceInventoryScope.RELEASE,
        manifest_name=manifest_name,
    )


class _SourceInventoryScope(Enum):
    RELEASE = "Compiler source"
    EXECUTING_PACKAGE = "Installed Molt package"


def _verify_source_files(
    root: Path,
    expected: Mapping[str, Mapping[str, Any]],
    *,
    scope: _SourceInventoryScope,
    manifest_name: str | None = None,
) -> Path:
    root = resolve_owned_path(root)
    label = scope.value
    exact = scope is _SourceInventoryScope.RELEASE
    # Inventory names are canonical POSIX paths. Once a parent is known,
    # all its ancestors were already admitted by the same walk.
    directories: set[str] = set()
    if exact:
        for name in expected:
            parent = name.rpartition("/")[0]
            while parent and parent not in directories:
                directories.add(parent)
                parent = parent.rpartition("/")[0]
    allowed = set(expected)
    if manifest_name is not None:
        allowed.add(manifest_name)
    observed: dict[str, os.stat_result] = {}
    observed_dirs: dict[str, tuple[Path, os.stat_result]] = {}
    root_metadata = root.lstat()
    pending = [(root, root_metadata, "")]

    def directory_generation(
        metadata: os.stat_result,
    ) -> tuple[int, int, int, int, int]:
        return (
            metadata.st_dev,
            metadata.st_ino,
            metadata.st_mode,
            metadata.st_mtime_ns,
            metadata.st_ctime_ns,
        )

    def check_directory(path: Path, metadata: os.stat_result) -> None:
        current = path.lstat()
        if (
            metadata_is_link_like(current)
            or not stat.S_ISDIR(current.st_mode)
            or directory_generation(current) != directory_generation(metadata)
        ):
            raise ValueError(f"{label} directory changed: {path}")

    while pending:
        current, metadata, prefix = pending.pop()
        check_directory(current, metadata)
        with os.scandir(current) as entries:
            for entry in entries:
                relative = prefix + entry.name
                # DirEntry.stat omits device/inode identity on Windows.
                member = (
                    (current / entry.name).lstat()
                    if os.name == "nt"
                    else entry.stat(follow_symlinks=False)
                )
                if not exact and entry.name == "__pycache__" and entry.is_dir():
                    continue
                if metadata_is_link_like(member):
                    raise ValueError(f"{label} contains a link: {current / entry.name}")
                if stat.S_ISDIR(member.st_mode) and (
                    not exact or relative in directories
                ):
                    child = current / entry.name
                    observed_dirs[relative] = (child, member)
                    pending.append((child, member, relative + "/"))
                elif stat.S_ISREG(member.st_mode) and relative in allowed:
                    observed[relative] = member
                elif not (stat.S_ISDIR(member.st_mode) or stat.S_ISREG(member.st_mode)):
                    raise ValueError(f"{label} is not a regular file: {relative}")
                elif exact:
                    raise ValueError(
                        "Compiler source file/directory closure is not exact"
                    )
                else:
                    raise ValueError(f"{label} file is not release source: {relative}")
    if exact and (set(observed) != allowed or set(observed_dirs) != directories):
        raise ValueError("Compiler source file/directory closure is not exact")
    check_modes = exact and os.name != "nt"
    capture_label = label.lower()

    # Check entries in manifest order. Most are small enough that scheduling
    # and contended metadata calls cost more than hashing their bytes.
    for name, entry in expected.items():
        metadata = observed.get(name)
        if metadata is None:
            continue
        identity = stable_regular_file_identity(
            root / name, label=capture_label, expected_path_stat=metadata
        )
        if (identity.size, identity.sha256) != (entry["size"], entry["sha256"]):
            raise ValueError(f"{label} content changed: {name}")
        # The supplied mode participated in the opened path/handle fence.
        # A later independent stat could instead inspect another generation.
        if check_modes and not _source_mode_matches_git(
            metadata.st_mode, entry["mode"]
        ):
            raise ValueError(
                f"Compiler source mode changed: {name} (found "
                f"{metadata.st_mode & 0o7777:o}, recorded {entry['mode'] & 0o777:o})"
            )
    if resolve_owned_path(root) != root:
        raise ValueError(f"{label} root changed during verification")
    check_directory(root, root_metadata)
    for path, metadata in observed_dirs.values():
        check_directory(path, metadata)
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
    expected_name = HostToolProduct.BACKEND.value + (
        ".exe" if record["platform"] == "windows" else ""
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


def validate_wasm_facts_record(record: object) -> dict[str, Any]:
    if (
        not isinstance(record, dict)
        or set(record) != {"path", "sha256", "size", "platform", "arch", "profile"}
        or not _digest(record.get("sha256"))
        or type(record.get("size")) is not int
        or record["size"] <= 0
        or record.get("profile") != PRODUCTION_COMPILER_PROFILE
        or not isinstance(record.get("platform"), str)
        or not isinstance(record.get("arch"), str)
        or (record["platform"], record["arch"]) not in RUST_TARGET_BY_COORDINATE
    ):
        raise ValueError("invalid production WASM facts identity")
    name = HostToolProduct.WASM_FACTS.value + (
        ".exe" if record["platform"] == "windows" else ""
    )
    if record["path"] != f"bin/{name}":
        raise ValueError("invalid production WASM facts identity")
    return record


RUNTIME_INVENTORY_SCHEMA = "molt.installed-runtime.v3"
RUNTIME_CELL_SCHEMA = "molt.installed-runtime-cell.v3"
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


@dataclass(frozen=True, slots=True)
class InstalledExecutableSelection:
    """Current manifest policy and the directly owned executable it selects."""

    product: HostToolProduct
    path: Path
    size: int
    sha256: str

    @property
    def label(self) -> str:
        if self.product is HostToolProduct.BACKEND:
            return "installed compiler"
        return "installed WASM facts tool"

    def _require_content(self, observed: StableRegularFileIdentity) -> None:
        if observed.path != self.path:
            raise ValueError("Installed executable selection changed")
        if (observed.size, observed.sha256) != (self.size, self.sha256):
            if self.product is HostToolProduct.BACKEND:
                raise ValueError(
                    "Installed compiler content differs from its release manifest"
                )
            raise ValueError(
                "Installed WASM facts tool differs from its release manifest"
            )

    def capture(self) -> StableRegularFileObservation:
        """Hash the selected current bytes independently of every prior receipt."""
        observation = StableRegularFileObservation.capture(
            resolve_owned_path(self.path), label=self.label
        )
        observed = observation.identity
        self._require_content(observed)
        if resolve_owned_path(self.path) != observed.path:
            raise ValueError("Installed executable selection changed")
        verify_stable_regular_file_identity(observed, label=self.label)
        return observation

    def verify(self, observation: StableRegularFileObservation) -> None:
        """Refuse replaced, rewritten or redirected operation observations."""
        observed = observation.identity
        self._require_content(observed)
        if resolve_owned_path(self.path) != observed.path:
            raise ValueError("Installed executable selection changed")
        observation.verify(label=self.label)
        if resolve_owned_path(self.path) != observed.path:
            raise ValueError("Installed executable selection changed")


@dataclass(frozen=True)
class InstalledCompiler:
    source_root: Path
    source_sha: str
    record: dict[str, Any]
    launcher: dict[str, Any]
    wasm_facts: dict[str, Any]
    files: tuple[Mapping[str, Any], ...]
    runtime: dict[str, Any]
    stdlib_source_summaries: StdlibSourceSummariesRecord

    def source_inventory_sha256(self) -> str:
        """Canonical SHA-256 of the released source record array."""
        return canonical_json_sha256(self.files, default=dict)

    def release_fingerprint(self) -> str:
        """Hash the admitted release facts once within their live operation."""
        manifests = _INSTALLED_MANIFESTS.get()
        owner = None if manifests is None else manifests.get(self.source_root)
        admitted = owner is not None and owner.compiler == self
        if admitted and owner.fingerprint is not None:
            return owner.fingerprint
        fingerprint = canonical_json_sha256(
            {
                "schema": "molt.installed-compiler-identity.v1",
                "source": self.source_sha,
                "files": self.files,
                "compiler": self.record,
                "launcher": self.launcher,
                "runtime": self.runtime,
                "wasm_facts": self.wasm_facts,
                "stdlib_source_summaries": self.stdlib_source_summaries,
            },
            default=dict,
        )
        if admitted:
            owner.fingerprint = fingerprint
        return fingerprint

    @property
    def binary(self) -> Path:
        return self.source_root.parent / self.record["path"]

    @property
    def wasm_facts_binary(self) -> Path:
        return self.source_root.parent / self.wasm_facts["path"]

    def verify_wasm_facts(self) -> dict[str, str | int]:
        return (
            self.select_executable(HostToolProduct.WASM_FACTS)
            .capture()
            .identity.content_record()
        )

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

    def select_executable(
        self,
        product: HostToolProduct,
        features: tuple[str, ...] | None = None,
        cargo_profile: str | None = None,
    ) -> InstalledExecutableSelection:
        """Apply current host/feature/profile policy before admitting any bytes."""
        if product is HostToolProduct.WASM_FACTS:
            record = self.wasm_facts
            path = self.wasm_facts_binary
            if current_host_coordinate() != (record["platform"], record["arch"]):
                raise ValueError("Installed WASM facts tool does not match this host")
        elif product is HostToolProduct.BACKEND:
            record = self.record
            path = self.binary
            features = tuple(record["features"]) if features is None else features
            cargo_profile = (
                record["profile"] if cargo_profile is None else cargo_profile
            )
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
            if current_host_coordinate() != (
                self.record["platform"],
                self.record["arch"],
            ):
                raise ValueError(
                    "Installed compiler does not match this host OS/architecture"
                )
        else:
            raise ValueError("Installed Molt ships no selected host tool")
        return InstalledExecutableSelection(
            product, resolve_owned_path(path), record["size"], record["sha256"]
        )

    def verify_binary(
        self, features: tuple[str, ...], cargo_profile: str
    ) -> dict[str, str | int]:
        """Exact release boundary: every call captures the actual current bytes."""
        return (
            self.select_executable(HostToolProduct.BACKEND, features, cargo_profile)
            .capture()
            .identity.content_record()
        )

    def verify_sources(self) -> None:
        self.verify_launcher()
        verify_source_inventory(
            self.source_root, self.files, manifest_name=MANIFEST_NAME
        )
        if self._stdlib_summary_owner() is None:
            self._read_stdlib_source_summaries(retain=False)
        else:
            self.capture_stdlib_source_summaries()

    def _stdlib_source_summaries_path(self) -> Path:
        return resolve_owned_path(
            self.source_root.parent / STDLIB_SOURCE_SUMMARIES_PATH
        )

    def _stdlib_summary_owner(self) -> _InstalledManifest | None:
        manifests = _INSTALLED_MANIFESTS.get()
        owner = None if manifests is None else manifests.get(self.source_root)
        if (
            owner is not None
            and owner.compiler.stdlib_source_summaries == self.stdlib_source_summaries
        ):
            return owner
        return None

    def _read_stdlib_source_summaries(
        self, *, retain: bool
    ) -> _StdlibSummarySnapshot | None:
        from molt import input_report

        path = self._stdlib_source_summaries_path()
        input_report.report_probe(path)
        with input_report.capture_read(path) as receipt:
            if retain:
                identity, contents = capture_stable_regular_file(
                    path,
                    max_bytes=MAX_STDLIB_SOURCE_SUMMARIES_BYTES,
                    label=_STDLIB_SUMMARIES_LABEL,
                )
            else:
                with open_stable_regular_file(
                    path, label=_STDLIB_SUMMARIES_LABEL
                ) as opened:
                    identity = stable_regular_file_handle_identity(
                        opened,
                        max_bytes=MAX_STDLIB_SOURCE_SUMMARIES_BYTES,
                        label=_STDLIB_SUMMARIES_LABEL,
                    )
                contents = None
            self._verify_stdlib_source_summaries_identity(identity)
            receipt.opened()
            receipt.complete(identity.stat_token(), identity.sha256)
        if contents is None:
            return None
        return _StdlibSummarySnapshot(identity, contents, receipt.session)

    def capture_stdlib_source_summaries(self) -> bytes:
        from molt import input_report

        owner = self._stdlib_summary_owner()
        snapshot = None if owner is None else owner.stdlib_summary
        if snapshot is None:
            snapshot = self._read_stdlib_source_summaries(retain=True)
            assert snapshot is not None
            if owner is not None:
                owner.stdlib_summary = snapshot
        else:
            # The immutable bytes belong to this operation. A changed live
            # generation cannot silently become a second archive in it.
            path = self._stdlib_source_summaries_path()
            if (
                path != snapshot.identity.path
                or input_report.stat_token(path) != snapshot.identity.stat_token()
            ):
                raise ValueError(
                    "Installed stdlib source summaries changed during admission"
                )
            input_report.report_consumed_buffer(
                path, snapshot.contents, previous_session=snapshot.recording_session
            )
        return snapshot.contents

    def _verify_stdlib_source_summaries_identity(
        self, identity: StableRegularFileIdentity
    ) -> None:
        if (
            identity.sha256 != self.stdlib_source_summaries["sha256"]
            or identity.size != self.stdlib_source_summaries["size"]
        ):
            raise ValueError(
                "Installed stdlib source summaries differ from their manifest"
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
        package_prefix = "src/molt/"
        _verify_source_files(
            package,
            {
                entry["path"][len(package_prefix) :]: entry
                for entry in self.files
                if entry["path"].startswith(package_prefix)
            },
            scope=_SourceInventoryScope.EXECUTING_PACKAGE,
        )


class InstalledSourceAdmission(Enum):
    INVENTORY = "inventory"
    REUSE = "reuse"
    FRESH = "fresh"


@dataclass(frozen=True, slots=True)
class _StdlibSummarySnapshot:
    identity: StableRegularFileIdentity
    contents: bytes
    recording_session: object | None


@dataclass(slots=True)
class _InstalledManifest:
    raw: bytes
    compiler: InstalledCompiler
    fingerprint: str | None = None
    sources_verified: bool = False
    stdlib_summary: _StdlibSummarySnapshot | None = None


_INSTALLED_MANIFESTS: ContextVar[dict[Path, _InstalledManifest] | None] = ContextVar(
    "_INSTALLED_MANIFESTS", default=None
)


@contextmanager
def installed_manifest_transaction() -> Generator[None]:
    """Keep validated manifest projections private to the calling operation."""
    token = _INSTALLED_MANIFESTS.set({})
    try:
        yield
    finally:
        _INSTALLED_MANIFESTS.reset(token)


def installed_compiler(
    source_root: Path,
    *,
    sources: InstalledSourceAdmission = InstalledSourceAdmission.INVENTORY,
) -> InstalledCompiler | None:
    from molt import input_report

    manifest_path = source_root / MANIFEST_NAME
    input_report.report_probe(manifest_path)
    if not manifest_path.exists() and not is_link_like(manifest_path):
        # A damaged installation is installed damage, never source mode.
        if installed_distribution_root(source_root) is not None:
            raise ValueError("Installed compiler manifest is missing; reinstall Molt")
        return None
    source_root = resolve_owned_path(source_root)
    # Selection always reads the current direct-file generation. Only the
    # pure projection of identical bytes belongs to the operation; neither
    # metadata nor a persisted digest can authorize reuse.
    _, raw = capture_stable_regular_file(
        manifest_path, max_bytes=16 * 1024 * 1024, label="installed compiler manifest"
    )
    manifests = _INSTALLED_MANIFESTS.get()
    manifest = None if manifests is None else manifests.get(source_root)
    if manifest is None or manifest.raw != raw:
        if manifests is not None:
            manifests.pop(source_root, None)
        manifest = _InstalledManifest(
            raw, _validate_installed_manifest(source_root, raw)
        )
    if manifests is not None:
        manifests[source_root] = manifest
    if sources is InstalledSourceAdmission.FRESH:
        manifest.stdlib_summary = None
    if sources is InstalledSourceAdmission.FRESH or (
        sources is InstalledSourceAdmission.REUSE and not manifest.sources_verified
    ):
        manifest.sources_verified = False
        try:
            manifest.compiler.verify_sources()
            manifest.compiler.verify_executing_package()
        except BaseException:
            manifest.stdlib_summary = None
            raise
        manifest.sources_verified = True
    if manifests is None:
        return manifest.compiler
    # Source rows contain only checked scalar values and are immutable.
    # Release metadata returned to callers remains a private projection.
    compiler = manifest.compiler
    return replace(
        compiler,
        record=deepcopy(compiler.record),
        launcher=deepcopy(compiler.launcher),
        wasm_facts=deepcopy(compiler.wasm_facts),
        runtime=deepcopy(compiler.runtime),
        stdlib_source_summaries=deepcopy(compiler.stdlib_source_summaries),
    )


def _validate_installed_manifest(source_root: Path, raw: bytes) -> InstalledCompiler:
    payload = loads_exact(raw.decode("utf-8", errors="strict"))
    if (
        not isinstance(payload, dict)
        or set(payload)
        != {
            "schema",
            "git",
            "files",
            "compiler",
            "launcher",
            "wasm_facts",
            "runtime",
            "stdlib_source_summaries",
        }
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
        path = entry["path"]
        if not isinstance(path, str):
            raise ValueError("Invalid installed compiler source path")
        # Collision identity also admits the portable spelling verbatim.
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
    wasm_facts = validate_wasm_facts_record(payload["wasm_facts"])
    if (wasm_facts["platform"], wasm_facts["arch"]) != (
        compiler["platform"],
        compiler["arch"],
    ):
        raise ValueError("Installed WASM facts tool target differs from compiler")
    runtime = validate_runtime_inventory(
        payload["runtime"], platform=compiler["platform"], arch=compiler["arch"]
    )
    if runtime["source"] != git:
        raise ValueError("Installed runtime cells come from different source")
    summaries = payload["stdlib_source_summaries"]
    if (
        not isinstance(summaries, dict)
        or set(summaries) != {"path", "size", "sha256"}
        or summaries["path"] != STDLIB_SOURCE_SUMMARIES_PATH
        or type(summaries["size"]) is not int
        or not 0 < summaries["size"] <= MAX_STDLIB_SOURCE_SUMMARIES_BYTES
        or not _digest(summaries["sha256"])
    ):
        raise ValueError("Invalid installed stdlib source summaries manifest record")
    return InstalledCompiler(
        source_root,
        git["commit"],
        compiler,
        validate_launcher_record(payload["launcher"]),
        wasm_facts,
        tuple(MappingProxyType(entry) for entry in files),
        runtime,
        StdlibSourceSummariesRecord(
            path=summaries["path"], size=summaries["size"], sha256=summaries["sha256"]
        ),
    )
