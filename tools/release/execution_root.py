"""Closed, archive-derived Linux execution roots for release consumer replay.

This module assembles data. It never downloads, installs, probes or executes a
package. cross_run owns the container transport; the native supervisor owns
process/image admission and event replay. Reference Python stays outside.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
import hashlib
import io
from pathlib import Path, PurePosixPath
import stat
import tarfile
import tomllib
from typing import Any

from molt.file_publication import resolve_owned_path
from molt.portable_paths import portable_relative_path
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

ROOT_SCHEMA = "molt.release-execution-root.v1"
_MAX_MEMBER = 256 * 1024 * 1024
_MAX_ARCHIVE_MEMBERS = 65536
_EMPTY_DIRECTORIES = ("app", "dev", "etc", "proc", "sys", "tmp", "evidence", "policies")


def relative_path(raw: object) -> PurePosixPath:
    return portable_relative_path(raw)


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
            loader = relative_path(interpreter.removeprefix("/")).as_posix()
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


def root_inventory(root: Path) -> list[dict[str, Any]]:
    if resolve_owned_path(root) != root.absolute() or not root.is_dir():
        raise ValueError("execution-root is not a directly owned directory")
    rows = []
    for path in sorted(root.rglob("*")):
        relative_path(path.relative_to(root).as_posix())
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
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
    return rows


def seal_root(
    root: Path, archive: Path, *, expected_files: list[dict[str, Any]]
) -> dict[str, Any]:
    for name in _EMPTY_DIRECTORIES:
        (root / name).mkdir(exist_ok=True)
    rows = sorted(expected_files, key=lambda row: row["path"])
    if root_inventory(root) != rows:
        raise ValueError("execution-root differs from admitted payloads before sealing")
    by_path = {row["path"]: row for row in rows}
    with tarfile.open(archive, "x", format=tarfile.USTAR_FORMAT) as handle:
        for path in sorted(root.rglob("*")):
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


def validate_sealed_tar(
    archive: Path, root: Path, *, expected_archive: dict[str, Any]
) -> None:
    """The imported archive must encode the retained root, not merely a hash."""
    expected = {row["path"]: row for row in root_inventory(root)}
    # ZIP retention may omit empty directories. The imported tar has this one
    # canonical directory closure regardless of the receiver filesystem.
    directories = set(_EMPTY_DIRECTORIES)
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
    seen_files: set[str] = set()
    seen_directories: set[str] = set()
    last_end = 0
    with tarfile.open(fileobj=opened.stream, mode="r:") as handle:
        for member in handle:
            if member.offset != last_end:
                raise ValueError("sealed root archive has hidden metadata")
            last_end = member.offset_data + ((member.size + 511) // 512) * 512
            name = relative_path(member.name).as_posix()
            if (
                member.uid
                or member.gid
                or member.mtime
                or member.pax_headers
                or member.linkname
            ):
                raise ValueError("sealed root archive has unadmitted metadata")
            if member.isdir():
                if (
                    name not in directories
                    or name in seen_directories
                    or member.mode != 0o555
                ):
                    raise ValueError("sealed root archive directory closure differs")
                seen_directories.add(name)
            elif member.isfile():
                row = expected.get(name)
                if (
                    row is None
                    or name in seen_files
                    or member.size != row["size"]
                    or member.mode != row["mode"]
                ):
                    raise ValueError("sealed root archive file closure differs")
                stream = handle.extractfile(member)
                if (
                    stream is None
                    or hashlib.file_digest(stream, "sha256").hexdigest()
                    != row["sha256"]
                ):
                    raise ValueError(
                        "sealed root archive payload differs from retained root"
                    )
                seen_files.add(name)
            else:
                raise ValueError("sealed root archive has a link or special file")
    if seen_files != set(expected) or seen_directories != directories:
        raise ValueError("sealed root archive is incomplete")

    opened.stream.seek(last_end)
    padding = opened.stream.read(10241)
    if (
        not 1024 <= len(padding) <= 10240
        or any(padding)
        or (last_end + len(padding)) % 10240
    ):
        raise ValueError("sealed root archive has noncanonical trailing data")
