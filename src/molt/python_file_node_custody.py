"""Stable no-follow, handle-bound file-node and path-topology custody."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, replace
import os
import stat
import time
import unicodedata
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path, PurePath
from typing import TypeVar, cast

from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    read_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.python_identity_common import (
    PythonEnvironmentIdentityError,
    _valid_relative_payload_path,
    _valid_sha256,
)


def _is_file_entry(value: Mapping[str, object]) -> bool:
    return value.get("kind") in {"file", "hardlink", "symlink"}


def _portable_path_key(value: str) -> str:
    """Identify path aliases consistently across capture and live consumers."""
    return unicodedata.normalize("NFC", value).casefold()


_RootPath = TypeVar("_RootPath", bound=PurePath)


def _root_forest(
    candidates: Sequence[tuple[str, _RootPath]], *, root_prefix: str
) -> tuple[list[tuple[str, _RootPath]], list[dict[str, object]]]:
    """Own overlapping resolved or logical regions once, ordered by their roles.

    Callers resolve host paths before entry. Pure logical receipt paths use the
    same forest and role-reference rules without consulting the validating host.
    """
    unique: dict[_RootPath, set[str]] = {}
    for role, path in candidates:
        unique.setdefault(path, set()).add(role)
    owners = [
        path
        for path in unique
        if not any(path != other and path.is_relative_to(other) for other in unique)
    ]
    owner_roles = {
        owner: tuple(
            sorted(
                role
                for path, roles in unique.items()
                if path.is_relative_to(owner)
                for role in roles
            )
        )
        for owner in owners
    }
    owners.sort(key=lambda path: owner_roles[path])
    roots = [(f"{root_prefix}-{index}", path) for index, path in enumerate(owners)]
    root_ids = {path: root_id for root_id, path in roots}
    references: list[dict[str, object]] = []
    for path, roles in unique.items():
        owner = next(owner for owner in owners if path.is_relative_to(owner))
        relative = unicodedata.normalize("NFC", path.relative_to(owner).as_posix())
        references.extend(
            {"role": role, "root": root_ids[owner], "path": relative} for role in roles
        )
    references.sort(key=lambda row: str(row["role"]))
    return roots, references


def _stat_identity(value: os.stat_result) -> tuple[int, int, int, int, int]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _path_stat_identity(value: os.stat_result) -> tuple[int, ...]:
    device, inode, size, mtime, ctime = _stat_identity(value)
    # Directory storage allocation is not membership. Windows can report a
    # directory as size 0 then 4096 on read-only enumeration; the complete
    # membership snapshots below own its children. Retain object, mode and
    # timestamp checks, and exact byte length for every non-directory entry.
    return (
        value.st_mode,
        device,
        inode,
        0 if stat.S_ISDIR(value.st_mode) else size,
        mtime,
        ctime,
    )


def _semantic_access(metadata: os.stat_result) -> dict[str, bool]:
    mode = stat.S_IMODE(metadata.st_mode)
    return {
        "readable": bool(mode & (stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)),
        "writable": bool(mode & (stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH)),
        "executable": bool(mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)),
    }


def _stable_object_key(metadata: os.stat_result) -> tuple[int, int]:
    return (metadata.st_dev, metadata.st_ino)


class PythonFileCaptureContext:
    """One bounded capture lane, shared hashes, and exact nonsemantic file custody."""

    def __init__(self, *, hash_workers: int = 1) -> None:
        if type(hash_workers) is not int or not 1 <= hash_workers <= 32:
            raise PythonEnvironmentIdentityError(
                "hash_workers must be an integer in 1..32"
            )
        self.hash_workers = hash_workers
        self._files: dict[Path, StableRegularFileIdentity] = {}
        self._objects: dict[tuple[int, int], StableRegularFileIdentity] = {}
        # Keep each emitted semantic row alive until the envelope binds its
        # location. Separate pools can reference the same physical generation.
        self._node_custody: dict[
            int, tuple[dict[str, object], StableRegularFileIdentity]
        ] = {}
        self._hashed_bytes = 0
        self._hashed_files = 0
        self._hash_seconds = 0.0
        self._verification_fences: list[Callable[[], None]] = []

    def _remember(self, identity: StableRegularFileIdentity) -> None:
        self._files[identity.path] = identity
        device, inode, _mode, _size, _mtime_ns, _ctime_ns = identity._stat_identity
        if inode:
            self._objects[(device, inode)] = identity
        self._hashed_bytes += identity.size
        self._hashed_files += 1

    def prepare(
        self, rows: Sequence[tuple[Path, os.stat_result]], *, label: str
    ) -> None:
        work: list[Path] = []
        seen: set[tuple[int, int] | Path] = set()
        for path, metadata in rows:
            key = _stable_object_key(metadata) if metadata.st_ino else path
            if key in seen or path.absolute() in self._files or key in self._objects:
                continue
            seen.add(key)
            work.append(path)
        started = time.perf_counter()

        def capture(path: Path) -> StableRegularFileIdentity:
            return stable_regular_file_identity(path, label=label)

        if self.hash_workers == 1:
            for path in work:
                self._remember(capture(path))
        elif work:
            # Bound pending futures as well as worker buffers on large environments.
            with ThreadPoolExecutor(max_workers=self.hash_workers) as executor:
                window = self.hash_workers * 4
                for offset in range(0, len(work), window):
                    for identity in executor.map(
                        capture, work[offset : offset + window]
                    ):
                        self._remember(identity)
        self._hash_seconds += time.perf_counter() - started

    def bind(
        self, path: Path, expected: os.stat_result, *, label: str
    ) -> StableRegularFileIdentity:
        lexical = path.absolute()
        if not stat.S_ISREG(expected.st_mode):
            raise PythonEnvironmentIdentityError(
                f"{label} is not a regular file: {path}"
            )
        identity = self._files.get(lexical)
        if identity is None and expected.st_ino:
            prior = self._objects.get(_stable_object_key(expected))
            if prior is not None:
                identity = replace(prior, path=lexical)
        if identity is None:
            started = time.perf_counter()
            identity = stable_regular_file_identity(lexical, label=label)
            self._hash_seconds += time.perf_counter() - started
            self._remember(identity)
        verify_stable_regular_file_identity(identity, label=label)
        if _path_stat_identity(expected) != _path_stat_identity(lexical.lstat()):
            raise PythonEnvironmentIdentityError(
                f"{label} changed since tree snapshot: {path}"
            )
        self._files[lexical] = identity
        return identity

    def verify(self) -> None:
        identities = list(self._files.values())

        def verify_identity(identity: StableRegularFileIdentity) -> None:
            verify_stable_regular_file_identity(
                identity, label="Python capture custody"
            )

        if self.hash_workers == 1:
            for identity in identities:
                verify_identity(identity)
        elif identities:
            with ThreadPoolExecutor(max_workers=self.hash_workers) as executor:
                window = self.hash_workers * 4
                for offset in range(0, len(identities), window):
                    tuple(
                        executor.map(
                            verify_identity,
                            identities[offset : offset + window],
                        )
                    )
        for verify in self._verification_fences:
            verify()

    def register_verification_fence(self, verify: Callable[[], None]) -> None:
        """Retain a producer's non-file snapshot through outer publication."""
        self._verification_fences.append(verify)

    def register_node(
        self, node: dict[str, object], identity: StableRegularFileIdentity
    ) -> None:
        self._node_custody[id(node)] = (node, identity)

    def node_path(self, node: Mapping[str, object]) -> Path:
        binding = self._node_custody.get(id(node))
        if binding is None or binding[0] is not node:
            raise PythonEnvironmentIdentityError(
                "Python semantic file node has no producer custody binding"
            )
        identity = binding[1]
        if self._files.get(identity.path) != identity:
            raise PythonEnvironmentIdentityError(
                f"Python semantic file node has no captured generation: {identity.path}"
            )
        return identity.path

    def file_custody(self) -> list[dict[str, object]]:
        self.verify()
        return [
            {"path": str(path), "size": identity.size, "sha256": identity.sha256}
            for path, identity in sorted(
                self._files.items(), key=lambda pair: str(pair[0])
            )
        ]

    def inventory_profile(self) -> dict[str, object]:
        return {
            "hash_workers": self.hash_workers,
            "hashed_files": self._hashed_files,
            "hashed_bytes": self._hashed_bytes,
            "hash_seconds": self._hash_seconds,
        }


class _FileNodePool:
    """Topology nodes backed by the shared stable-file authority, not retained bytes."""

    def __init__(
        self, *, capture_context: PythonFileCaptureContext | None = None
    ) -> None:
        self.capture_context = (
            capture_context
            if capture_context is not None
            else PythonFileCaptureContext()
        )
        self._node_by_object: dict[tuple[int, int] | Path, str] = {}
        self._nodes: list[dict[str, object]] = []
        self._identities: dict[str, StableRegularFileIdentity] = {}

    def prepare(
        self, rows: Sequence[tuple[Path, os.stat_result]], *, label: str
    ) -> None:
        self.capture_context.prepare(rows, label=label)

    def bind(self, path: Path, expected: os.stat_result, *, label: str) -> str:
        try:
            identity = self.capture_context.bind(path, expected, label=label)
        except (OSError, ValueError) as exc:
            raise PythonEnvironmentIdentityError(f"cannot bind {label}: {exc}") from exc
        key = _stable_object_key(expected) if expected.st_ino else identity.path
        existing = self._node_by_object.get(key)
        if existing is not None:
            return existing
        node_id = f"file-node-{len(self._nodes)}"
        self._node_by_object[key] = node_id
        self._nodes.append(
            {"id": node_id, "size": identity.size, "sha256": identity.sha256}
        )
        self._identities[node_id] = identity
        return node_id

    def node_for_metadata(self, metadata: os.stat_result) -> str | None:
        if not metadata.st_ino:
            return None
        return self._node_by_object.get(_stable_object_key(metadata))

    def read_bound(self, node_id: str, *, label: str) -> bytes:
        try:
            identity = self._identities[node_id]
            return read_stable_regular_file(identity, label=label)
        except (KeyError, OSError, ValueError) as exc:
            raise PythonEnvironmentIdentityError(
                f"{label} has no stable captured bytes: {exc}"
            ) from exc

    @property
    def nodes(self) -> list[dict[str, object]]:
        rows = [dict(node) for node in self._nodes]
        for row in rows:
            self.capture_context.register_node(row, self._identities[str(row["id"])])
        return rows


def _relative_path(path: Path, root: Path, *, label: str) -> str:
    try:
        relative = path.relative_to(root)
    except ValueError as exc:
        raise PythonEnvironmentIdentityError(
            f"{label} escapes environment root: {path}"
        ) from exc
    value = unicodedata.normalize("NFC", relative.as_posix())
    if (
        not value
        or value == "."
        or value.startswith("../")
        or any(part in {"", ".", ".."} for part in value.split("/"))
    ):
        raise PythonEnvironmentIdentityError(f"invalid {label} path: {path}")
    return value


def _is_junction(path: Path) -> bool:
    predicate = getattr(path, "is_junction", None)
    return bool(predicate is not None and predicate())


def _metadata_is_junction(metadata: os.stat_result) -> bool:
    reparse_point = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    return bool(
        stat.S_ISDIR(metadata.st_mode)
        and reparse_point
        and getattr(metadata, "st_file_attributes", 0) & reparse_point
    )


@dataclass(frozen=True)
class VerifiedTreeFile:
    path: Path
    content: StableRegularFileIdentity


def resolve_native_tree_path(
    root: Path,
    relative: str,
    *,
    directory_names: dict[Path, dict[str, str]] | None = None,
    inspect_directory: Callable[[Path], None] | None = None,
) -> Path:
    """Resolve canonical semantic names to collision-checked native spelling.

    Normalization is for receipt keys only; never pass a normalized spelling
    to the host filesystem. Live file verification can share its directory
    snapshots and cache while external directory roots use the same locator.
    """
    if relative != "." and not _valid_relative_payload_path(relative):
        raise PythonEnvironmentIdentityError(
            f"tree path is not canonical: {relative!r}"
        )
    names_by_directory = {} if directory_names is None else directory_names
    current = root
    for component in PurePath(relative).parts:
        if current not in names_by_directory:
            if inspect_directory is not None:
                inspect_directory(current)
            else:
                metadata = current.lstat()
                if not stat.S_ISDIR(metadata.st_mode) or _metadata_is_junction(
                    metadata
                ):
                    raise PythonEnvironmentIdentityError(
                        f"tree parent uses path indirection: {current}"
                    )
            names: dict[str, str] = {}
            portable_names: set[str] = set()
            with os.scandir(current) as children:
                for child in children:
                    normalized = unicodedata.normalize("NFC", child.name)
                    portable = _portable_path_key(child.name)
                    if portable in portable_names:
                        raise PythonEnvironmentIdentityError(
                            f"tree has a case/Unicode path collision: {current}"
                        )
                    portable_names.add(portable)
                    names[normalized] = child.name
            names_by_directory[current] = names
        raw = names_by_directory[current].get(component)
        if raw is None:
            raise PythonEnvironmentIdentityError(f"tree path is absent: {relative}")
        current = current / raw
    return current


def verify_tree_file(
    root: Path,
    relative: str,
    *,
    entries: Mapping[str, Mapping[str, object]],
    nodes: Mapping[str, Mapping[str, object]],
) -> VerifiedTreeFile:
    """Revalidate a selected tree file's content, access and alias topology.

    This is a bounded consumer check, not a replacement for capture-wide
    mutation watching. All parent directories and same-node aliases are bound
    on both sides of the content read using the scanner's existing semantics.
    """
    entry = entries.get(relative)
    if entry is None or not _is_file_entry(entry) or entry.get("node") not in nodes:
        raise PythonEnvironmentIdentityError(
            f"file is absent from tree custody: {relative}"
        )
    node_id = str(entry["node"])
    peers = {name: row for name, row in entries.items() if row.get("node") == node_id}
    snapshots: dict[Path, tuple[int, ...]] = {}
    primary: Path | None = None
    directory_names: dict[Path, dict[str, str]] = {}

    def host_path(name: str) -> Path:
        return resolve_native_tree_path(
            root,
            name,
            directory_names=directory_names,
            inspect_directory=check_directory,
        )

    def check_directory(path: Path) -> None:
        metadata = path.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or _metadata_is_junction(metadata):
            raise PythonEnvironmentIdentityError(
                f"tree parent uses path indirection: {path}"
            )
        if path != root:
            row = entries.get(
                unicodedata.normalize("NFC", path.relative_to(root).as_posix())
            )
            if (
                row is None
                or row.get("kind") != "directory"
                or row.get("access") != _semantic_access(metadata)
            ):
                raise PythonEnvironmentIdentityError(
                    f"tree parent differs from receipt: {path}"
                )
        snapshots[path] = _path_stat_identity(metadata)

    check_directory(root)
    for name, row in peers.items():
        path = host_path(name)
        for parent in reversed(path.parents):
            if parent.is_relative_to(root) and parent not in snapshots:
                check_directory(parent)
        metadata = path.lstat()
        snapshots[path] = _path_stat_identity(metadata)
        kind = row["kind"]
        if kind == "symlink":
            if (
                not stat.S_ISLNK(metadata.st_mode)
                or row.get("target_owner") != "same-root"
            ):
                raise PythonEnvironmentIdentityError(
                    f"tree symlink differs from receipt: {path}"
                )
            target = host_path(str(row["target"]))
            if path.resolve(strict=True) != target:
                raise PythonEnvironmentIdentityError(
                    f"tree symlink target differs from receipt: {path}"
                )
            access = target.stat()
        else:
            if not stat.S_ISREG(metadata.st_mode):
                raise PythonEnvironmentIdentityError(
                    f"tree file kind differs from receipt: {path}"
                )
            access = metadata
            if primary is None:
                primary = path
            elif not path.samefile(primary):
                raise PythonEnvironmentIdentityError(
                    f"tree hardlink differs from receipt: {path}"
                )
        if row.get("access") != _semantic_access(access):
            raise PythonEnvironmentIdentityError(
                f"tree file access differs from receipt: {path}"
            )
    if primary is None:
        raise PythonEnvironmentIdentityError(
            f"tree file has no direct owner: {relative}"
        )
    identity = stable_regular_file_identity(primary, label="tree consumer file")
    node = nodes[node_id]
    if identity.size != node["size"] or identity.sha256 != node["sha256"]:
        raise PythonEnvironmentIdentityError(
            f"tree file content differs from receipt: {relative}"
        )
    for path, before in snapshots.items():
        if _path_stat_identity(path.lstat()) != before:
            raise PythonEnvironmentIdentityError(
                f"tree topology changed during consumer check: {path}"
            )
    return VerifiedTreeFile(path=host_path(relative), content=identity)


def _tree_membership_snapshot(
    root: Path,
    *,
    label: str,
    excluded: frozenset[str] = frozenset(),
    pruned_components: frozenset[str] = frozenset(),
) -> tuple[tuple[int, ...], list[tuple[str, Path, os.stat_result]]]:
    """Take a no-follow, complete membership/entry snapshot of one root."""

    try:
        root_metadata = root.lstat()
    except OSError as exc:
        raise PythonEnvironmentIdentityError(
            f"cannot stat {label} root {root}: {exc}"
        ) from exc
    if (
        not stat.S_ISDIR(root_metadata.st_mode)
        or stat.S_ISLNK(root_metadata.st_mode)
        or _metadata_is_junction(root_metadata)
    ):
        raise PythonEnvironmentIdentityError(
            f"{label} root is not a real directory: {root}"
        )
    records: list[tuple[str, Path, os.stat_result]] = []
    folded: set[str] = set()
    pending = [(root, "")]
    while pending:
        directory, relative_prefix = pending.pop()
        try:
            children = sorted(os.scandir(directory), key=lambda item: item.name)
        except OSError as exc:
            raise PythonEnvironmentIdentityError(
                f"cannot enumerate {label} root {directory}: {exc}"
            ) from exc
        for child in children:
            path = Path(child.path)
            name = unicodedata.normalize("NFC", child.name)
            relative = f"{relative_prefix}/{name}" if relative_prefix else name
            components = frozenset(part.casefold() for part in relative.split("/"))
            if relative in excluded or components & pruned_components:
                continue
            folded_path = _portable_path_key(relative)
            if folded_path in folded:
                raise PythonEnvironmentIdentityError(
                    f"{label} has a case/Unicode path collision: {relative}"
                )
            folded.add(folded_path)
            try:
                metadata = (
                    path.lstat()
                    if os.name == "nt"
                    else child.stat(follow_symlinks=False)
                )
            except OSError as exc:
                raise PythonEnvironmentIdentityError(
                    f"{label} entry changed during membership snapshot: {path}"
                ) from exc
            if _metadata_is_junction(metadata):
                raise PythonEnvironmentIdentityError(
                    f"{label} contains a directory junction: {path}"
                )
            records.append((relative, path, metadata))
            if stat.S_ISDIR(metadata.st_mode):
                pending.append((path, relative))
    records.sort(key=lambda row: (row[0].casefold(), row[0]))
    return _path_stat_identity(root_metadata), records


_TreeSnapshotFingerprint = tuple[
    tuple[int, ...], tuple[tuple[str, tuple[int, ...]], ...]
]


def _snapshot_fingerprint(
    root_identity: tuple[int, ...], records: Sequence[tuple[str, Path, os.stat_result]]
) -> _TreeSnapshotFingerprint:
    return (
        root_identity,
        tuple(
            (relative, _path_stat_identity(metadata))
            for relative, _path, metadata in records
        ),
    )


def _snapshot_difference(
    before: _TreeSnapshotFingerprint,
    after: _TreeSnapshotFingerprint,
) -> str:
    def metadata_delta(old: tuple[int, ...], new: tuple[int, ...]) -> str:
        # Keep the exact changed fields in failure evidence; a path-only report
        # cannot distinguish replacement, access-mode drift and timestamp drift.
        fields = ("mode", "device", "inode", "size", "mtime_ns", "ctime_ns")
        return ", ".join(
            f"{field}={left}->{right}"
            for field, left, right in zip(fields, old, new, strict=True)
            if left != right
        )

    if before[0] != after[0]:
        return f"root metadata changed: {metadata_delta(before[0], after[0])}"
    before_by_path = dict(before[1])
    after_by_path = dict(after[1])
    for relative in sorted(
        before_by_path.keys() | after_by_path.keys(),
        key=lambda value: (value.casefold(), value),
    ):
        if relative not in before_by_path:
            return f"entry added: {relative}"
        if relative not in after_by_path:
            return f"entry removed: {relative}"
        if before_by_path[relative] != after_by_path[relative]:
            return (
                f"entry metadata changed: {relative} "
                f"[{metadata_delta(before_by_path[relative], after_by_path[relative])}]"
            )
    return "snapshot identity changed"


def _stable_tree_inventory(
    root: Path,
    *,
    root_id: str,
    label: str,
    pool: _FileNodePool,
    excluded: frozenset[str] = frozenset(),
    pruned_components: frozenset[str] = frozenset(),
    external_symlink_roles: Mapping[str, Path] | None = None,
) -> tuple[dict[str, object], set[str], dict[str, os.stat_result]]:
    """Capture a tree with one hash per file object and a closing snapshot."""

    canonical = root.resolve(strict=True)
    before_root, before = _tree_membership_snapshot(
        canonical,
        label=label,
        excluded=excluded,
        pruned_components=pruned_components,
    )
    by_path = {relative: (path, metadata) for relative, path, metadata in before}
    regular_nodes: dict[str, str] = {}
    node_paths: dict[str, list[str]] = {}
    pool.prepare(
        [
            (path, metadata)
            for _relative, path, metadata in before
            if stat.S_ISREG(metadata.st_mode)
        ],
        label=f"{label} file",
    )
    for relative, path, metadata in before:
        if not stat.S_ISREG(metadata.st_mode):
            continue
        node = pool.bind(
            path,
            metadata,
            label=f"{label} file",
        )
        regular_nodes[relative] = node
        node_paths.setdefault(node, []).append(relative)
    primary_paths = {
        node: min(paths, key=lambda value: (value.casefold(), value))
        for node, paths in node_paths.items()
    }
    rows: list[dict[str, object]] = []
    files: set[str] = set()
    for relative, path, metadata in before:
        if stat.S_ISDIR(metadata.st_mode):
            rows.append(
                {
                    "path": relative,
                    "kind": "directory",
                    "access": _semantic_access(metadata),
                }
            )
            continue
        if stat.S_ISREG(metadata.st_mode):
            node = regular_nodes[relative]
            rows.append(
                {
                    "path": relative,
                    "kind": "file" if primary_paths[node] == relative else "hardlink",
                    "node": node,
                    "access": _semantic_access(metadata),
                }
            )
            files.add(relative)
            continue
        if not stat.S_ISLNK(metadata.st_mode):
            raise PythonEnvironmentIdentityError(
                f"{label} contains a non-file entry: {path}"
            )
        try:
            resolved = path.resolve(strict=True)
        except OSError as exc:
            raise PythonEnvironmentIdentityError(
                f"{label} contains a broken symlink: {path}"
            ) from exc
        try:
            target_relative = _relative_path(
                resolved, canonical, label=f"{label} symlink target"
            )
        except PythonEnvironmentIdentityError:
            if resolved.is_dir():
                raise PythonEnvironmentIdentityError(
                    f"{label} directory symlink escapes custody: {path} -> {resolved}"
                ) from None
            external_matches: list[str] = []
            for external_role, external_path in sorted(
                (external_symlink_roles or {}).items()
            ):
                try:
                    same_external = resolved.samefile(external_path)
                except OSError:
                    same_external = False
                if same_external:
                    external_matches.append(external_role)
            if not external_matches:
                raise PythonEnvironmentIdentityError(
                    f"{label} file symlink escapes custody: {path} -> {resolved}"
                ) from None
            if len(external_matches) != 1:
                raise PythonEnvironmentIdentityError(
                    f"{label} file symlink has ambiguous external runtime custody: "
                    f"{path} -> {resolved}"
                )
            external_role = external_matches[0]
            rows.append(
                {
                    "path": relative,
                    "kind": "symlink",
                    "target_owner": "base-runtime",
                    "target_role": external_role,
                    "access": _semantic_access(resolved.stat()),
                }
            )
            files.add(relative)
            continue
        target = by_path.get(target_relative)
        if resolved.is_dir():
            if target is None or not stat.S_ISDIR(target[1].st_mode):
                raise PythonEnvironmentIdentityError(
                    f"{label} directory symlink target is not captured: {path}"
                )
            rows.append(
                {
                    "path": relative,
                    "kind": "directory-symlink",
                    "target_owner": "same-root",
                    "target": target_relative,
                    "access": _semantic_access(target[1]),
                }
            )
        elif target is None or not stat.S_ISREG(target[1].st_mode):
            raise PythonEnvironmentIdentityError(
                f"{label} symlink target is not a captured regular file: {path}"
            )
        else:
            node = regular_nodes[target_relative]
            rows.append(
                {
                    "path": relative,
                    "kind": "symlink",
                    "target_owner": "same-root",
                    "target": target_relative,
                    "node": node,
                    "access": _semantic_access(target[1]),
                }
            )
            files.add(relative)
        try:
            after_link = path.lstat()
        except OSError as exc:
            raise PythonEnvironmentIdentityError(
                f"{label} symlink changed: {path}"
            ) from exc
        if _path_stat_identity(after_link) != _path_stat_identity(metadata):
            raise PythonEnvironmentIdentityError(f"{label} symlink changed: {path}")
    expected_membership = _snapshot_fingerprint(before_root, before)

    def verify_membership() -> None:
        after_root, after = _tree_membership_snapshot(
            canonical,
            label=label,
            excluded=excluded,
            pruned_components=pruned_components,
        )
        actual = _snapshot_fingerprint(after_root, after)
        if expected_membership != actual:
            difference = _snapshot_difference(expected_membership, actual)
            raise PythonEnvironmentIdentityError(
                f"{label} changed during inventory between stable snapshots: {canonical} "
                f"({difference})"
            )

    verify_membership()
    # Keep only compact metadata, not parser bytes or duplicate Path/stat rows.
    # Each public capture checks this same fence after all later root inventories.
    pool.capture_context.register_verification_fence(verify_membership)
    rows.sort(key=lambda row: (str(row["path"]).casefold(), str(row["path"])))
    node_ids = sorted(
        {str(row["node"]) for row in rows if "node" in row},
        key=lambda value: int(value.removeprefix("file-node-")),
    )
    return (
        {
            "id": root_id,
            "file_count": len(files),
            "node_ids": node_ids,
            "entries": rows,
            "manifest_sha256": canonical_json_sha256(rows),
        },
        files,
        {relative: metadata for relative, _path, metadata in before},
    )


def _runtime_root_inventory(root: Path, *, root_id: str) -> dict[str, object]:
    pool = _FileNodePool()
    inventory, _files, _metadata = _stable_tree_inventory(
        root,
        root_id=root_id,
        label="Python runtime",
        pool=pool,
        pruned_components=frozenset({"site-packages", "dist-packages"}),
    )
    return inventory


def _valid_access(value: object) -> bool:
    return (
        isinstance(value, Mapping)
        and set(value) == {"readable", "writable", "executable"}
        and all(type(value.get(field)) is bool for field in value)
    )


def _validate_file_nodes(
    value: object, *, label: str
) -> tuple[list[Mapping[str, object]], dict[str, Mapping[str, object]]]:
    if not isinstance(value, list):
        raise PythonEnvironmentIdentityError(f"{label} file nodes are invalid")
    nodes: list[Mapping[str, object]] = []
    by_id: dict[str, Mapping[str, object]] = {}
    for index, raw in enumerate(value):
        if not isinstance(raw, Mapping):
            raise PythonEnvironmentIdentityError(f"{label} file node is invalid")
        node = cast(Mapping[str, object], raw)
        size = node.get("size")
        node_id = node.get("id")
        if (
            set(node) != {"id", "size", "sha256"}
            or node_id != f"file-node-{index}"
            or type(size) is not int
            or size < 0
            or not _valid_sha256(node.get("sha256"))
        ):
            raise PythonEnvironmentIdentityError(f"{label} file node is invalid")
        nodes.append(node)
        by_id[str(node_id)] = node
    return nodes, by_id


def _validate_inventory_entries(
    value: object,
    *,
    label: str,
    nodes: Mapping[str, Mapping[str, object]],
    external_runtime_roles: Mapping[str, object] | None = None,
) -> tuple[list[Mapping[str, object]], set[str], set[str]]:
    if not isinstance(value, list):
        raise PythonEnvironmentIdentityError(f"{label} entries are invalid")
    entries: list[Mapping[str, object]] = []
    paths: set[str] = set()
    folded: set[str] = set()
    referenced_nodes: set[str] = set()
    for raw in value:
        if not isinstance(raw, Mapping):
            raise PythonEnvironmentIdentityError(f"{label} entry is invalid")
        raw = cast(Mapping[str, object], raw)
        if not _valid_relative_payload_path(raw.get("path")):
            raise PythonEnvironmentIdentityError(f"{label} entry is invalid")
        path = str(raw["path"])
        identity = _portable_path_key(path)
        if identity in folded:
            raise PythonEnvironmentIdentityError(f"{label} path is duplicated: {path}")
        folded.add(identity)
        paths.add(path)
        kind = raw.get("kind")
        if kind == "directory":
            if set(raw) != {"path", "kind", "access"} or not _valid_access(
                raw.get("access")
            ):
                raise PythonEnvironmentIdentityError(
                    f"{label} directory entry is invalid"
                )
        elif kind in {"file", "hardlink"}:
            node = raw.get("node")
            if (
                set(raw) != {"path", "kind", "node", "access"}
                or node not in nodes
                or not _valid_access(raw.get("access"))
            ):
                raise PythonEnvironmentIdentityError(f"{label} file entry is invalid")
            referenced_nodes.add(str(node))
        elif kind == "symlink":
            owner = raw.get("target_owner")
            if owner == "same-root":
                valid = (
                    set(raw)
                    == {"path", "kind", "target_owner", "target", "node", "access"}
                    and _valid_relative_payload_path(raw.get("target"))
                    and raw.get("node") in nodes
                    and _valid_access(raw.get("access"))
                )
                if valid:
                    referenced_nodes.add(str(raw["node"]))
            elif owner == "base-runtime" and external_runtime_roles is not None:
                valid = (
                    set(raw)
                    == {"path", "kind", "target_owner", "target_role", "access"}
                    and isinstance(raw.get("target_role"), str)
                    and raw.get("target_role") in external_runtime_roles
                    and _valid_access(raw.get("access"))
                )
            else:
                valid = False
            if not valid:
                raise PythonEnvironmentIdentityError(
                    f"{label} symlink entry is invalid"
                )
        elif kind == "directory-symlink":
            if (
                set(raw) != {"path", "kind", "target_owner", "target", "access"}
                or raw.get("target_owner") != "same-root"
                or not _valid_relative_payload_path(raw.get("target"))
                or not _valid_access(raw.get("access"))
            ):
                raise PythonEnvironmentIdentityError(
                    f"{label} directory symlink entry is invalid"
                )
        else:
            raise PythonEnvironmentIdentityError(f"{label} entry kind is invalid")
        entries.append(raw)
    expected_order = sorted(
        entries, key=lambda row: (str(row["path"]).casefold(), str(row["path"]))
    )
    if entries != expected_order:
        raise PythonEnvironmentIdentityError(f"{label} entries are not canonical")
    by_path = {str(row["path"]): row for row in entries}
    for row in entries:
        path = str(row["path"])
        parts = path.split("/")
        for index in range(1, len(parts)):
            parent = "/".join(parts[:index])
            if by_path.get(parent, {}).get("kind") != "directory":
                raise PythonEnvironmentIdentityError(
                    f"{label} parent directory is absent: {parent}"
                )
        if row.get("target_owner") != "same-root" or row.get("kind") not in {
            "symlink",
            "directory-symlink",
        }:
            continue
        target = by_path.get(str(row["target"]))
        if row.get("kind") == "directory-symlink":
            valid_target = (
                target is not None
                and target.get("kind") == "directory"
                and target.get("access") == row.get("access")
            )
        else:
            valid_target = (
                target is not None
                and target.get("kind") in {"file", "hardlink"}
                and target.get("node") == row.get("node")
                and target.get("access") == row.get("access")
            )
        if not valid_target:
            raise PythonEnvironmentIdentityError(
                f"{label} symlink target differs from its owned row: {path}"
            )
    ordinary_by_node: dict[str, list[Mapping[str, object]]] = {}
    for row in entries:
        if row.get("kind") in {"file", "hardlink"}:
            ordinary_by_node.setdefault(str(row["node"]), []).append(row)
    for node, ordinary in ordinary_by_node.items():
        primary = min(
            ordinary, key=lambda row: (str(row["path"]).casefold(), str(row["path"]))
        )
        if primary.get("kind") != "file" or any(
            row is not primary and row.get("kind") != "hardlink" for row in ordinary
        ):
            raise PythonEnvironmentIdentityError(
                f"{label} hardlink topology is not canonical for {node}"
            )
    return entries, paths, referenced_nodes
