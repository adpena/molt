"""Closed, archive-derived Linux execution roots for release consumer replay.

This module assembles data. It never downloads, installs, probes or executes a
package. cross_run owns the systemd service transport; the native supervisor owns
process/image admission and event replay. Post-uninstall replay excludes Python;
the explicit compilation projection stages its captured frontend inputs.
"""

from __future__ import annotations

from collections.abc import Callable, Generator, Iterable, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import io
from pathlib import Path, PurePosixPath
import stat
import tarfile
import tomllib
from typing import TYPE_CHECKING, Any

from tools.cross_run import SealedExecutionProjection, SEALED_REPLAY_EXECUTION

from molt.file_publication import resolve_owned_path, durable_remove_path
from molt.portable_paths import portable_relative_path
from molt.python_identity_common import canonical_absolute_path
from molt.cli.static_archive_identity import open_static_archive_members
from molt.exact_json import canonical_json_sha256
from molt.python_native_dependency_custody import _elf_dependencies, elf_interpreter
from molt.tool_releases import load_tool_releases, open_pinned_archive
from molt.toolchain_identity import (
    StableRegularFileHandle,
    capture_stable_regular_file,
    open_stable_regular_file,
    snapshot_stable_regular_file,
    stable_regular_file_content_identity,
    stable_regular_file_handle_identity,
)
from .release_model import ROOT

if TYPE_CHECKING:
    from molt.compiler_distribution import InstalledCompiler
    from tools.proof_queue_pkg.custody_cas import FileRef

ROOT_SCHEMA = "molt.release-execution-root.v2"
_MAX_MEMBER = 256 * 1024 * 1024
_MAX_ARCHIVE_MEMBERS = 65536
_EMPTY_DIRECTORIES = ("app", "dev", "etc", "proc", "sys", "tmp", "evidence", "policies")


def relative_path(raw: object) -> PurePosixPath:
    return portable_relative_path(raw)


def linux_root_relative_path(raw: object) -> PurePosixPath:
    """Project a canonical Linux host coordinate into this sealed root.

    Archive coordinates are already relative and must not pass through here.
    Parsing retained host fields does not borrow the receiver's OS grammar;
    the current sealed provider supports only the POSIX / namespace.
    """
    host = canonical_absolute_path(raw)
    if not isinstance(host, PurePosixPath) or host.anchor != "/" or len(host.parts) < 2:
        raise ValueError("sealed execution root requires a Linux host coordinate")
    return relative_path(host.relative_to("/").as_posix())


def file_identity(path: Path) -> dict[str, object]:
    return dict(
        stable_regular_file_content_identity(path, label="execution-root input")
    )


def _tar_members(handle: tarfile.TarFile) -> dict[str, tarfile.TarInfo]:
    members: dict[str, tarfile.TarInfo] = {}
    for member in handle:
        if len(members) >= _MAX_ARCHIVE_MEMBERS:
            raise ValueError("execution-root archive has too many members")
        name = member.name.removeprefix("./").rstrip("/")
        if not name:
            continue
        name = relative_path(name).as_posix()
        if name in members:
            raise ValueError("execution-root archive has duplicate members")
        members[name] = member
    return members


def _regular_member(
    handle: tarfile.TarFile, members: dict[str, tarfile.TarInfo], name: str
) -> bytes:
    """Resolve only an archive-local symlink to one bounded regular payload."""
    visited: set[str] = set()
    while True:
        if name in visited or len(visited) > 8:
            raise ValueError("execution-root archive member link cycle")
        visited.add(name)
        member = members.get(name)
        if member is None:
            raise ValueError(f"execution-root pinned archive member is missing: {name}")
        if member.issym():
            # No filesystem resolution and no '..' traversal, even within root.
            link = relative_path(member.linkname)
            name = (PurePosixPath(name).parent / link).as_posix()
            continue
        if not member.isfile() or not 0 < member.size <= _MAX_MEMBER:
            raise ValueError("execution-root payload must be a bounded regular file")
        stream = handle.extractfile(member)
        if stream is None:
            raise ValueError("execution-root archive member is unreadable")
        data = stream.read(member.size + 1)
        if len(data) != member.size:
            raise ValueError("execution-root archive member size mismatch")
        return data


def package_payloads(
    opened: StableRegularFileHandle, selections: list[dict[str, str]]
) -> dict[str, bytes]:
    """Use the shared ar framing authority; only data.tar payloads are read."""
    with open_static_archive_members(opened.path, opened=opened) as (members, stream):
        if len(members) != 3 or members[0].name != "debian-binary":
            raise ValueError("execution-root package must use Debian ar framing")
        stream.seek(members[0].content_offset)
        if stream.read(members[0].size) != b"2.0\n":
            raise ValueError("execution-root package version is unsupported")
        data_members = [
            m for m in members if m.name in {"data.tar.xz", "data.tar.gz", "data.tar"}
        ]
        if len(data_members) != 1 or data_members[0].size > _MAX_MEMBER:
            raise ValueError(
                "execution-root package needs one bounded supported data archive"
            )
        member = data_members[0]
        stream.seek(member.content_offset)
        data = stream.read(member.size)
        if len(data) != member.size:
            raise ValueError("execution-root package payload was truncated")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as handle:
        indexed = _tar_members(handle)
        return {
            relative_path(row["guest"]).as_posix(): _regular_member(
                handle, indexed, relative_path(row["archive"]).as_posix()
            )
            for row in selections
        }


def archive_inputs(*, arch: str, source_root: Path = ROOT) -> list[dict[str, Any]]:
    """One source projection for both explicit provisioning and offline replay."""
    policy = tomllib.loads(
        (source_root / "config/release_execution_roots.toml").read_text(
            encoding="utf-8"
        )
    )
    if (
        set(policy) != {"schema", "linux"}
        or policy["schema"] != "molt.release-execution-roots.v1"
    ):
        raise ValueError("execution-root policy schema is invalid")
    coordinate = policy["linux"].get(arch)
    if not isinstance(coordinate, dict) or set(coordinate) != {"packages"}:
        raise ValueError(f"no execution-root OS payload closure for Linux/{arch}")
    providers = []
    for package in coordinate["packages"]:
        if set(package) != {
            "name",
            "filename",
            "url",
            "provenance",
            "size",
            "sha256",
            "members",
        }:
            raise ValueError("execution-root package policy fields are invalid")
        if package["name"] == "node":
            raise ValueError("Node is owned by the existing tool release registry")
        providers.append(dict(package))
    release = load_tool_releases(source_root)["node"]
    asset = release.assets[f"{arch}-linux"]
    providers.append(
        {
            "name": "node",
            "filename": asset.filename,
            "url": asset.url,
            "provenance": release.provenance.payload(),
            "size": asset.size,
            "sha256": asset.sha256,
            "members": [{"archive": asset.archive_member, "guest": "bin/node"}],
        }
    )
    filenames = []
    for provider in providers:
        filename = relative_path(provider["filename"])
        if (
            len(filename.parts) != 1
            or provider["url"].rsplit("/", 1)[-1] != filename.name
        ):
            raise ValueError("execution-root archive URL and cache filename differ")
        filenames.append(filename.as_posix())
    if len(filenames) != len(set(filenames)):
        raise ValueError("execution-root archive providers collide")
    return providers


def support_payloads(
    cache: Path, *, arch: str, source_root: Path = ROOT
) -> tuple[dict[str, bytes], list[dict[str, Any]]]:
    providers = archive_inputs(arch=arch, source_root=source_root)
    payloads: dict[str, bytes] = {}
    for provider in providers:
        with open_pinned_archive(
            cache / provider["filename"],
            size=provider["size"],
            sha256=provider["sha256"],
        ) as opened:
            if provider["name"] == "node":
                with tarfile.open(fileobj=opened.stream, mode="r:*") as handle:
                    indexed = _tar_members(handle)
                    selected = {
                        row["guest"]: _regular_member(handle, indexed, row["archive"])
                        for row in provider["members"]
                    }
            else:
                selected = package_payloads(opened, provider["members"])
        if payloads.keys() & selected.keys():
            raise ValueError("execution-root package providers overlap")
        payloads.update(selected)
    return payloads, providers


def write_payload(
    root: Path, name: str, data: bytes, *, executable: bool
) -> dict[str, Any]:
    path = root.joinpath(*relative_path(name).parts)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as stream:
        stream.write(data)
    path.chmod(0o555 if executable else 0o444)
    return {
        "path": name,
        "filename": path.name,
        "sha256": hashlib.sha256(data).hexdigest(),
        "size": len(data),
        "mode": 0o555 if executable else 0o444,
    }


def stage_file(
    root: Path,
    name: str,
    source: Path,
    *,
    executable: bool,
    max_bytes: int | None = None,
) -> dict[str, object]:
    target = root.joinpath(*relative_path(name).parts)
    target.parent.mkdir(parents=True, exist_ok=True)
    snapshot = snapshot_stable_regular_file(
        source, target, label="execution-root staged input", max_bytes=max_bytes
    )
    target.chmod(0o555 if executable else 0o444)
    return {
        "path": name,
        "filename": target.name,
        "sha256": snapshot.snapshot.sha256,
        "size": snapshot.snapshot.size,
        "mode": 0o555 if executable else 0o444,
    }


def audit_native_closure(
    root: Path,
    *,
    arch: str,
    executable_paths: list[str],
    expected_files: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    """Require every DT_NEEDED basename and PT_INTERP in the admitted root.

    This is a conservative availability check, not loader search emulation.
    The actual read-only filesystem bounds RPATH/dlopen lookups too.
    A libpython edge has no provider. A renamed/extra runtime cannot be added
    because the package/member identities are checked independently at replay
    admission. Explicit runtime loading sees this same closed filesystem.
    """
    expected = {row["path"]: row for row in expected_files}

    def read_member(name: str) -> bytes:
        row = expected[name]
        identity, data = capture_stable_regular_file(
            root.joinpath(*relative_path(name).parts),
            label="execution-root ELF input",
            max_bytes=max(1, row["size"]),
        )
        if (identity.size, identity.sha256) != (row["size"], row["sha256"]):
            raise ValueError("execution-root ELF bytes differ from admitted payload")
        return data

    return _audit_native_closure(
        expected, read_member, arch=arch, executable_paths=executable_paths
    )


def audit_native_payload_closure(
    payloads: dict[str, bytes], *, arch: str, executable_paths: list[str]
) -> list[dict[str, Any]]:
    """Audit already pin-admitted immutable payloads without filesystem staging."""
    return _audit_native_closure(
        payloads, payloads.__getitem__, arch=arch, executable_paths=executable_paths
    )


def _audit_native_closure(
    member_names: Iterable[str],
    read_member: Callable[[str], bytes],
    *,
    arch: str,
    executable_paths: list[str],
) -> list[dict[str, Any]]:
    """One lazy ELF edge traversal over the admitted member namespace."""
    members = {relative_path(name).as_posix() for name in member_names}
    libraries: dict[str, str] = {}
    for directory in (f"lib/{arch}-linux-gnu", "lib", "lib64"):
        for name in sorted(members):
            path = PurePosixPath(name)
            if path.parent.as_posix() == directory:
                libraries[path.name] = name
    pending = [relative_path(name).as_posix() for name in executable_paths]
    seen: set[str] = set()
    edges: list[dict[str, Any]] = []
    while pending:
        name = pending.pop()
        if name in seen:
            continue
        if name not in members:
            raise ValueError("execution-root ELF input is not admitted")
        seen.add(name)
        data = read_member(name)
        interpreter = elf_interpreter(data, architecture=arch)
        if interpreter:
            loader = linux_root_relative_path(interpreter).as_posix()
            if loader not in members:
                raise ValueError(
                    f"execution-root ELF loader is not admitted: {interpreter}"
                )
            pending.append(loader)
            edges.append({"from": name, "to": loader, "kind": "interpreter"})
        for dependency in _elf_dependencies(data, architecture=arch):
            if dependency.name not in libraries:
                raise ValueError(
                    f"execution-root dependency is not admitted: {PurePosixPath(name).name} -> {dependency.name}"
                )
            target = libraries[dependency.name]
            pending.append(target)
            edges.append({"from": name, "to": target, "kind": dependency.kind})
    return sorted(edges, key=lambda row: (row["from"], row["to"], row["kind"]))


def root_inventory(
    root: Path, *, excluded_names: frozenset[str] = frozenset()
) -> list[dict[str, Any]]:
    if resolve_owned_path(root) != root.absolute() or not root.is_dir():
        raise ValueError("execution-root is not a directly owned directory")
    if any(len(relative_path(name).parts) != 1 for name in excluded_names):
        raise ValueError("inventory exclusion must name a top-level owner")
    rows = []
    pending = [path for path in root.iterdir() if path.name not in excluded_names]
    while pending:
        path = pending.pop()
        relative_path(path.relative_to(root).as_posix())
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            pending.extend(path.iterdir())
            continue
        if not stat.S_ISREG(mode) or path.is_symlink():
            raise ValueError("execution-root contains a link or special file")
        rows.append(
            {
                "path": path.relative_to(root).as_posix(),
                **file_identity(path),
                "mode": 0o555 if mode & 0o111 else 0o444,
            }
        )
    return sorted(rows, key=lambda row: row["path"])


def seal_root(
    root: Path,
    archive: Path,
    *,
    expected_files: list[dict[str, Any]],
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> dict[str, Any]:
    for name in (
        *_EMPTY_DIRECTORIES,
        *(("work",) if projection.mode == "compilation" else ()),
    ):
        (root / name).mkdir(exist_ok=True)
    rows = sorted(expected_files, key=lambda row: row["path"])
    if root_inventory(root) != rows:
        raise ValueError("execution-root differs from admitted payloads before sealing")
    by_path = {row["path"]: row for row in rows}
    with tarfile.open(archive, "x", format=tarfile.PAX_FORMAT) as handle:
        for path in sorted(
            root.rglob("*"), key=lambda path: path.relative_to(root).as_posix()
        ):
            name = path.relative_to(root).as_posix()
            info = tarfile.TarInfo(name)
            info.uid = info.gid = info.mtime = 0
            if path.is_dir():
                info.type, info.mode = tarfile.DIRTYPE, 0o555
                handle.addfile(info)
            else:
                row = by_path[name]
                with open_stable_regular_file(
                    path, label="sealed root member"
                ) as opened:
                    identity = stable_regular_file_handle_identity(
                        opened,
                        label="sealed root member",
                        max_bytes=max(1, row["size"]),
                    )
                    if (identity.size, identity.sha256) != (row["size"], row["sha256"]):
                        raise ValueError(
                            "sealed root member differs from admitted payload"
                        )
                    info.mode, info.size = row["mode"], identity.size
                    handle.addfile(info, opened.stream)
    if root_inventory(root) != rows:
        raise ValueError("execution-root changed while sealing")
    return {
        "schema": ROOT_SCHEMA,
        "files": rows,
        "files_sha256": canonical_json_sha256(rows),
        "archive": file_identity(archive),
    }


@dataclass(frozen=True, slots=True)
class RootResource:
    """One root.files row; destination mode is independent of shared bytes."""

    path: PurePosixPath
    sha256: str
    size: int
    mode: int

    @classmethod
    def from_record(cls, row: object) -> RootResource:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "filename", "sha256", "size", "mode"}
            or type(row["mode"]) is not int
            or row["mode"] not in {0o444, 0o555}
            or type(row["size"]) is not int
            or row["size"] < 0
            or not isinstance(row["sha256"], str)
            or len(row["sha256"]) != 64
            or any(c not in "0123456789abcdef" for c in row["sha256"])
            or row["filename"] != relative_path(row["path"]).name
        ):
            raise ValueError("sealed root resource row is invalid")
        return cls(relative_path(row["path"]), row["sha256"], row["size"], row["mode"])

    def as_record(self) -> dict[str, Any]:
        return {
            "path": self.path.as_posix(),
            "filename": self.path.name,
            "sha256": self.sha256,
            "size": self.size,
            "mode": self.mode,
        }

    def reference(self, resources: Path) -> FileRef:
        from tools.proof_queue_pkg import custody_cas

        return custody_cas.file_reference(
            resources, sha256=self.sha256, size_bytes=self.size, logical_name="payload"
        )


def validate_root_record(sealed: dict[str, Any]) -> tuple[RootResource, ...]:
    """The file list alone owns lexical paths, modes and resource identities."""
    if (
        not isinstance(sealed, dict)
        or set(sealed) != {"schema", "files", "files_sha256", "archive"}
        or sealed["schema"] != ROOT_SCHEMA
        or not isinstance(sealed["files"], list)
        or len(sealed["files"]) > _MAX_ARCHIVE_MEMBERS
        or canonical_json_sha256(sealed["files"]) != sealed["files_sha256"]
    ):
        raise ValueError("sealed root resource record is invalid")
    rows = tuple(RootResource.from_record(row) for row in sealed["files"])
    names = [row.path.as_posix() for row in rows]
    if names != sorted(set(names)):
        raise ValueError("sealed root resource paths are not unique and ordered")
    archive = sealed["archive"]
    if not isinstance(archive, dict) or set(archive) != {"filename", "sha256", "size"}:
        raise ValueError("sealed root archive identity is invalid")
    identity = RootResource.from_record(
        {**archive, "path": archive["filename"], "mode": 0o444}
    )
    if len(identity.path.parts) != 1 or identity.size < 10240 or identity.size % 10240:
        raise ValueError("sealed root archive size or name is noncanonical")
    return rows


def retain_root(
    root: Path,
    archive: Path,
    *,
    sealed: dict[str, Any],
    resources: Path,
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> dict[str, int]:
    """Retain each closed root payload once in the existing custody CAS.

    All content is stored as data named payload. Destination executable modes
    belong to root.files, so two lexical uses of the same bytes share storage.
    Call only after native execution and teardown have closed successfully.
    This does not delete materializations. The caller must durably publish its
    complete root/evidence record before retiring the recoverable originals.
    """
    from tools.proof_queue_pkg import custody_cas

    inputs = validate_root_record(sealed)
    rows = [row.as_record() for row in inputs]
    if root_inventory(root) != rows:
        raise ValueError("closed root differs from admitted resources")
    validate_sealed_tar(
        archive, root, expected_archive=sealed["archive"], projection=projection
    )
    retained = {"new_files": 0, "new_bytes": 0, "reused_files": 0, "reused_bytes": 0}
    for resource in inputs:
        row = resource.as_record()
        reference = resource.reference(resources)
        if Path(reference.path).exists():
            custody_cas.verify_file_ref(reference.as_dict(), expected_root=resources)
            retained["reused_files"] += 1
            retained["reused_bytes"] += row["size"]
        else:
            reference = custody_cas.put_file(
                resources, root / row["path"], logical_name="payload", executable=False
            )
            retained["new_files"] += 1
            retained["new_bytes"] += row["size"]
        if (reference.sha256, reference.size_bytes) != (row["sha256"], row["size"]):
            raise ValueError("closed root resource changed during retention")
    if root_inventory(root) != rows:
        raise ValueError("closed root changed during retention")
    return retained


def retire_root(
    root: Path,
    archive: Path,
    *,
    sealed: dict[str, Any],
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> None:
    """Retire a materialization only after its caller commits retained custody."""
    rows = [row.as_record() for row in validate_root_record(sealed)]
    if root_inventory(root) != rows:
        raise ValueError("materialized root changed before retirement")
    validate_sealed_tar(
        archive, root, expected_archive=sealed["archive"], projection=projection
    )
    durable_remove_path(archive)
    durable_remove_path(root)


@contextmanager
def materialized_root(
    root: Path,
    archive: Path,
    *,
    sealed: dict[str, Any],
    resources: Path,
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> Generator[Path]:
    """Recreate the exact original root/tar; preserve failed replay material.

    Resource hashes select buffers, not evidence admission. The caller still
    verifies independent candidate, capture and original native receipt owners.
    """
    from tools.proof_queue_pkg import custody_cas

    inputs = validate_root_record(sealed)
    rows = [row.as_record() for row in inputs]
    if root.exists() or archive.exists():
        raise ValueError("retained root materialization destination already exists")
    root.mkdir(parents=True)
    for resource in inputs:
        row = resource.as_record()
        reference = resource.reference(resources)
        custody_cas.verify_file_ref(reference.as_dict(), expected_root=resources)
        actual = stage_file(
            root,
            row["path"],
            Path(reference.path),
            executable=bool(row["mode"] & 0o111),
            max_bytes=max(1, row["size"]),
        )
        if actual != row:
            raise ValueError("retained root resource differs from admitted payload")
    reconstructed = seal_root(root, archive, expected_files=rows, projection=projection)
    if reconstructed != sealed:
        raise ValueError(
            "retained resources do not reconstruct the original sealed tar"
        )
    yield root
    # Exceptions deliberately retain the materialized root and tar for recovery.
    retire_root(root, archive, sealed=sealed, projection=projection)


def validate_sealed_tar(
    archive: Path,
    root: Path,
    *,
    expected_archive: dict[str, Any],
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> None:
    """The imported archive must encode the retained root, not merely a hash."""
    expected = {row["path"]: row for row in root_inventory(root)}
    # ZIP retention may omit empty directories. The imported tar has this one
    # canonical directory closure regardless of the receiver filesystem.
    directories = set(_EMPTY_DIRECTORIES)
    if projection.mode == "compilation":
        directories.add("work")
    for name in expected:
        directories.update(
            p.as_posix() for p in PurePosixPath(name).parents if p.as_posix() != "."
        )
    actual_directories = {
        path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_dir()
    }
    if not actual_directories <= directories:
        raise ValueError("retained root has an unadmitted directory")
    with open_stable_regular_file(archive, label="sealed root archive") as opened:
        identity = stable_regular_file_handle_identity(
            opened, label="sealed root archive", max_bytes=expected_archive["size"]
        )
        if (identity.size, identity.sha256) != (
            expected_archive["size"],
            expected_archive["sha256"],
        ):
            raise ValueError("sealed root archive differs from admitted identity")
        _validate_sealed_tar_handle(opened, expected, directories)


def _validate_sealed_tar_handle(
    opened: StableRegularFileHandle,
    expected: dict[str, dict[str, Any]],
    directories: set[str],
) -> None:
    # Encode the one expected header sequence instead of asking a permissive
    # archive parser to interpret attacker-supplied PAX/longname metadata. PAX
    # is necessary for actual content-addressed runtime member basenames. Only
    # the exact metadata emitted by our producer is accepted, in bounded reads.
    if len(expected) > _MAX_ARCHIVE_MEMBERS:
        raise ValueError("sealed root archive has too many members")
    opened.stream.seek(0)
    for name in sorted(set(expected) | directories):
        info = tarfile.TarInfo(name)
        info.uid = info.gid = info.mtime = 0
        if name in directories:
            info.type, info.mode = tarfile.DIRTYPE, 0o555
        else:
            row = expected[name]
            info.mode, info.size = row["mode"], row["size"]
        header = info.tobuf(tarfile.PAX_FORMAT, "utf-8", "surrogateescape")
        if opened.stream.read(len(header)) != header:
            raise ValueError(
                "sealed root archive has hidden metadata or its file/directory closure differs"
            )
        if name in directories:
            continue
        remaining = info.size
        digest = hashlib.sha256()
        while remaining:
            block = opened.stream.read(min(remaining, 64 * 1024))
            if not block:
                raise ValueError("sealed root archive payload is incomplete")
            remaining -= len(block)
            digest.update(block)
        if digest.hexdigest() != expected[name]["sha256"]:
            raise ValueError("sealed root archive payload differs from retained root")
        padding = (-info.size) % 512
        if opened.stream.read(padding) != bytes(padding):
            raise ValueError("sealed root archive has noncanonical member padding")
    last_end = opened.stream.tell()
    padding = opened.stream.read(10241)
    if (
        not 1024 <= len(padding) <= 10240
        or any(padding)
        or (last_end + len(padding)) % 10240
    ):
        raise ValueError("sealed root archive has noncanonical trailing data")


def compilation_runtime_inventory(
    compiler: InstalledCompiler,
    cells: tuple[Mapping[str, Any], ...],
) -> dict[str, Any]:
    """Project exact selected records without changing the installed catalog."""
    if not isinstance(cells, tuple) or not cells:
        raise ValueError("compilation requires explicit selected runtime cells")
    catalog = {cell["id"]: cell for cell in compiler.runtime["cells"]}
    selected: dict[str, dict[str, Any]] = {}
    for cell in cells:
        if (
            not isinstance(cell, Mapping)
            or not isinstance(cell.get("id"), str)
            or cell["id"] not in catalog
            or cell != catalog[cell["id"]]
        ):
            raise ValueError("selected runtime cell differs from the installed catalog")
        # Multiple logical lanes can consume the same physical runtime cell.
        selected[cell["id"]] = catalog[cell["id"]]
    return {**compiler.runtime, "cells": [selected[key] for key in sorted(selected)]}


def stage_compilation_inputs(
    root: Path,
    *,
    compiler: Any,
    runtime_cells: tuple[Mapping[str, Any], ...],
    python_captures: list[dict[str, Any]],
    toolchain_capture: dict[str, Any],
    cas_root: Path,
) -> list[dict[str, Any]]:
    """Project existing admitted installed/Python/toolchain owners into a root.

    Preserve their absolute lexical locations so launchers, venv configuration,
    Python imports and tool resources keep the same meaning. The returned files
    are the only read-only compile inputs; no ambient directory is mounted.
    This function does not infer absence of undeclared inputs. Missing closure
    fails inside the sealed namespace and remains a failed compile coordinate.
    """
    from dataclasses import replace
    from molt.compiler_distribution import (
        InstalledCompiler,
        installed_compiler,
        verify_runtime_tree,
    )
    from molt.python_capture import validate_python_capture
    from molt.source_root import MANIFEST_NAME
    from tools.proof_queue_pkg import toolchain_capture as captures
    from tools.proof_queue_pkg.process_image_capture import custody_path

    if not isinstance(compiler, InstalledCompiler):
        raise TypeError("sealed compilation requires an admitted installed compiler")
    if compiler.record["platform"] != "linux":
        raise ValueError(
            "sealed compilation namespace is unavailable for this platform"
        )
    if installed_compiler(compiler.source_root) != compiler:
        raise ValueError("installed compiler manifest changed before root projection")
    runtime = compilation_runtime_inventory(compiler, runtime_cells)
    compiler.verify_sources()
    compiler.verify_binary(
        tuple(compiler.record["features"]), compiler.record["profile"]
    )
    compiler.verify_wasm_facts()
    if not python_captures:
        raise ValueError("compilation input projection requires a frontend capture")
    pythons = [validate_python_capture(capture) for capture in python_captures]
    captured = captures.load_capture(toolchain_capture, cas_root=cas_root)
    rows: dict[str, dict[str, Any]] = {}
    retained = {row["path"]: row for row in root_inventory(root)}

    def stage(
        source: Path, expected: dict[str, Any], *, destination: Path | None = None
    ) -> None:
        if not source.is_absolute():
            raise ValueError("compilation input has no absolute custody coordinate")
        name = linux_root_relative_path(str(destination or source)).as_posix()
        if PurePosixPath(name).parts[0] in {"work", "evidence", "proc", "sys", "dev"}:
            raise ValueError(
                "read-only compilation input intersects a writable or virtual root"
            )
        if (
            type(expected.get("size")) is not int
            or expected["size"] < 0
            or not isinstance(expected.get("sha256"), str)
        ):
            raise ValueError("compilation input lacks retained content custody")
        prior = rows.get(name)
        if prior is not None:
            if any(prior[k] != expected[k] for k in ("size", "sha256")):
                raise ValueError("compilation input owners disagree on one pathname")
            return
        target = root / name
        if target.exists():
            prior = retained.get(name)
            if prior is None or any(
                prior[k] != expected[k] for k in ("size", "sha256")
            ):
                raise ValueError("compilation input conflicts with the admitted root")
            rows[name] = prior
            return
        result = stage_file(
            root,
            name,
            source,
            executable=bool(source.stat().st_mode & 0o111),
            max_bytes=max(1, expected["size"]),
        )
        if any(result[k] != expected[k] for k in ("size", "sha256")):
            raise ValueError("compilation input changed after owning admission")
        rows[name] = result

    for entry in compiler.files:
        stage(compiler.source_root / entry["path"], entry)
    manifest = compiler.source_root / MANIFEST_NAME
    stage(manifest, file_identity(manifest))
    for entry in (compiler.record, compiler.launcher, compiler.wasm_facts):
        stage(compiler.source_root.parent / entry["path"], entry)
    for cell in runtime["cells"]:
        for entry in cell["files"]:
            stage(compiler.verify_runtime_file(cell, entry["role"]), entry)
    for python in pythons:
        for entry in python["file_custody"]:
            stage(Path(entry["path"]), entry)
        for destination, entry in compilation_python_files(python):
            stage(Path(entry["path"]), entry, destination=destination)
    # The canonical capture loader verifies CAS identity and reconstructs the
    # complete frozen file projection. A caller-authored tool list is not input.
    for entry in captured["files"]:
        lexical = Path(entry["path"])
        stage(
            custody_path(lexical).resolve(strict=True),
            {"size": entry["size"], "sha256": entry["sha256"]},
            destination=lexical,
        )
    staged_compiler = installed_compiler(
        root / linux_root_relative_path(str(compiler.source_root))
    )
    if (
        staged_compiler is None
        or replace(staged_compiler, source_root=compiler.source_root) != compiler
        or installed_compiler(compiler.source_root) != compiler
    ):
        raise ValueError("installed compiler manifest changed during root projection")
    compiler.verify_sources()
    verify_runtime_tree(staged_compiler.runtime_root, runtime)
    return [rows[name] for name in sorted(rows)]


def compilation_python_files(capture: object) -> list[tuple[Path, dict[str, Any]]]:
    """Project lexical file consumers from the canonical capture's node bindings.

    Custody paths own bytes; semantic entries own their lexical uses. Preserve
    those uses as regular read-only copies in the new namespace. This does not
    assert the staged environment has the original host's topology or identity.
    Ambiguous root locations reject rather than discovering ambient directories.
    """
    from molt.python_capture import validate_python_capture

    captured = validate_python_capture(capture)
    identity = captured["identity"]
    if "tree" not in identity:
        raise ValueError(
            "compilation requires an installed frontend environment capture"
        )
    bindings = {
        row["node"]: captured["file_custody"][row["file_index"]]
        for row in captured["node_custody"]
    }
    runtime = identity["runtime"]

    def pool(nodes: list[dict[str, Any]], pointer: str) -> dict[str, dict[str, Any]]:
        return {row["id"]: bindings[f"{pointer}/{i}"] for i, row in enumerate(nodes)}

    runtime_files = pool(runtime["file_nodes"], "/runtime/file_nodes")
    environment_files = pool(identity["tree"]["file_nodes"], "/tree/file_nodes")
    base = next(
        (
            runtime_files[row["node"]]
            for row in runtime["explicit_files"]
            if row["role"] == "base-executable"
        ),
        None,
    )
    if base is None:
        raise ValueError("compilation capture omitted its base interpreter node")
    config = environment_files[identity["pyvenv_config"]["node"]]
    config_path = Path(config["path"])
    if config_path.name != "pyvenv.cfg":
        raise ValueError(
            "compilation environment root has no unambiguous config custody"
        )
    result: dict[Path, dict[str, Any]] = {}

    def add(path: Path, row: dict[str, Any]) -> None:
        prior = result.setdefault(path, row)
        if (prior["size"], prior["sha256"]) != (row["size"], row["sha256"]):
            raise ValueError("Python lexical input owners disagree on content")

    def project(
        root: Path, tree: dict[str, Any], files: dict[str, dict[str, Any]]
    ) -> None:
        entries = tree["entries"]
        file_rows: dict[str, dict[str, Any]] = {}
        for entry in entries:
            if entry["kind"] not in {"file", "hardlink", "symlink"}:
                continue
            row = (
                base
                if entry.get("target_owner") == "base-runtime"
                else files[entry["node"]]
            )
            file_rows[entry["path"]] = row
            add(root / entry["path"], row)
        # Directory aliases reference one canonical same-root directory. Expand
        # only semantic file rows already admitted above, never a live walk.
        for entry in entries:
            if entry["kind"] != "directory-symlink":
                continue
            prefix = entry["target"] + "/"
            for name, row in file_rows.items():
                if name.startswith(prefix):
                    add(root / entry["path"] / name[len(prefix) :], row)

    project(config_path.parent, identity["tree"], environment_files)
    for tree in runtime["runtime_roots"]:
        candidates: set[Path] = set()
        for entry in tree["entries"]:
            if entry["kind"] != "file":
                continue
            physical = Path(runtime_files[entry["node"]]["path"])
            relative = PurePosixPath(entry["path"])
            if physical.parts[-len(relative.parts) :] == relative.parts:
                candidates.add(physical.parents[len(relative.parts) - 1])
        if len(candidates) != 1:
            raise ValueError(
                "compilation runtime root lacks unique retained path custody"
            )
        project(candidates.pop(), tree, runtime_files)
    return sorted(result.items(), key=lambda item: item[0].as_posix())


def stage_compilation_link_inputs(
    root: Path,
    *,
    reproducer: Path,
    observation: object,
) -> list[dict[str, Any]]:
    """Retain preflight native linker inputs missing from the replay OS root.

    CRT objects, linker scripts and archives come from LLD's exact input buffers,
    not guessed sysroot walks or selected-path hashes. Compiler output/runtime
    have their own fresh producers and are excluded here. These bytes prepare a
    new closed compile; only its subsequent report proves actual consumption.
    """
    from contextlib import contextmanager
    from molt.cli.link_input_observation import (
        LinkInputObservation,
        capture_link_inputs,
    )

    declared = LinkInputObservation.from_record(observation)
    if declared.dialect != "elf-gnu" or set(dict(declared.roles)) != {
        "compiler_output",
        "runtime",
    }:
        raise ValueError(
            "compilation native resources require an exact native linker observation"
        )
    # A foreign retained record is valid raw evidence, but cannot select a
    # host image for this Linux-only execution root.
    linux_root_relative_path(declared.linker_path)
    contents = {
        name: {"size": size, "sha256": digest} for name, size, digest in declared.inputs
    }
    excluded = set(dict(declared.roles).values())
    rows = {row["path"]: row for row in root_inventory(root)}
    retained = []

    @contextmanager
    def sink(name: str, size: int):
        if name in excluded:
            yield None
            return
        relative = relative_path(name)
        if relative.parts[0] in {"work", "evidence", "proc", "sys", "dev"}:
            raise ValueError(
                "preflight linker input intersects the writable or virtual compile root: "
                + name
            )
        key = relative.as_posix()
        expected = contents.get(name)
        if expected is None or expected["size"] != size:
            raise ValueError(
                "preflight linker resource is absent from its declared inventory"
            )
        existing = rows.get(key)
        if existing is not None:
            if any(existing[field] != expected[field] for field in ("size", "sha256")):
                raise ValueError(
                    "preflight linker resource conflicts with an admitted compilation input"
                )
            yield None
            return
        path = root.joinpath(*relative.parts)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as destination:
            yield destination
        path.chmod(0o444)
        retained.append({"path": key, "filename": path.name, "mode": 0o444, **expected})

    captured = capture_link_inputs(
        reproducer,
        dialect=declared.dialect,
        linker_path=declared.linker_path,
        expected={role: (name, contents[name]) for role, name in declared.roles},
        input_sink=sink,
    )
    if captured != declared:
        raise ValueError(
            "preflight linker buffers differ from their declared observation"
        )
    return sorted(retained, key=lambda row: row["path"])


def stage_compilation_driver_inputs(
    root: Path, *, preparation: object
) -> list[dict[str, Any]]:
    """Copy selected preprocessor files; never relabel them as consumed bytes."""
    from molt.cli.link_input_observation import validate_native_link_preparation

    selected = validate_native_link_preparation(preparation)
    linux_root_relative_path(selected["source"])
    entries = [selected["driver"], *selected["files"]]
    projected = [(entry, linux_root_relative_path(entry["path"])) for entry in entries]
    # Reject an unsupported host namespace before publishing any staged file.
    for entry in entries:
        linux_root_relative_path(entry["content_path"])
    existing = {row["path"]: row for row in root_inventory(root)}
    rows = []
    for entry, relative in projected:
        if entry["path"] == selected["source"]:
            continue  # The fresh build generates its own C entrypoint.
        if relative.parts[0] in {"work", "evidence", "proc", "sys", "dev"}:
            raise ValueError(
                "native preparation input intersects a writable or virtual root"
            )
        name = relative.as_posix()
        old = existing.get(name)
        if old is not None:
            if any(old[field] != entry[field] for field in ("size", "sha256")):
                raise ValueError(
                    "native preparation conflicts with an admitted root input"
                )
            continue
        if entry is selected["driver"]:
            raise ValueError(
                "native driver is absent from the admitted toolchain input root"
            )
        row = stage_file(
            root,
            name,
            Path(entry["content_path"]),
            executable=False,
            max_bytes=max(1, entry["size"]),
        )
        if any(row[field] != entry[field] for field in ("size", "sha256")):
            raise ValueError("native preparation selected input changed before staging")
        rows.append(row)
        existing[name] = row
    return sorted(rows, key=lambda row: row["path"])
