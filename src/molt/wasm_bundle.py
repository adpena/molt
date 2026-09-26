"""Deterministic, portable read-only VFS bundles for every WASM producer."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
from typing import TypedDict

from molt.artifact_publication import (
    discard_staged_output,
    publication_payload_snapshot,
    publish_validated_outputs,
    staged_output_path,
)
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import open_stable_regular_file


class BundleFile(TypedDict):
    path: str
    size: int


class BundleManifest(TypedDict):
    files: list[BundleFile]
    total_bytes: int


def _bundle_entries(
    snapshot: Mapping[Path, tuple[Path, ...]],
) -> list[tuple[str, Path]]:
    entries: dict[str, Path] = {}
    identities = {portable_path_identity("__manifest__.json")}
    directories: set[str] = set()
    for root, paths in snapshot.items():
        for path in paths:
            name = path.relative_to(root).as_posix()
            relative = portable_relative_path(name)
            identity = portable_path_identity(name)
            parents = {
                portable_path_identity(parent.as_posix())
                for parent in relative.parents
                if parent != PurePosixPath(".")
            }
            if (
                identity in identities
                or identity in directories
                or parents & identities
            ):
                raise ValueError(f"portable bundle path collision: {name}")
            identities.add(identity)
            directories.update(parents)
            entries[name] = path
    # Archive order is the portable role name, never checkout/OS collation.
    return sorted(entries.items())


def _header(name: str, size: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = 0o644  # VFS payload is read-only data, not host executables.
    try:
        info.tobuf(format=tarfile.USTAR_FORMAT, encoding="utf-8", errors="strict")
    except (ValueError, UnicodeError) as exc:
        raise ValueError(
            f"bundle member cannot be represented as USTAR: {name}: {exc}"
        ) from exc
    return info


def write_wasm_bundle(
    roots: Iterable[Path],
    output: Path,
    *,
    include: Callable[[Path, Path], bool] | None = None,
    omit_empty: bool = False,
) -> BundleManifest | None:
    """Stream one coherent source snapshot, then publish its complete archive.

    USTAR regular files are shared by the native and browser VFS readers. Host
    timestamps, permissions, owner IDs, source roots and root ordering are not
    payload. Unrepresentable or colliding paths fail before replacing an archive.
    """
    roots = tuple(dict.fromkeys(Path(root).absolute() for root in roots))
    output = output.absolute()
    if any(output.resolve().is_relative_to(root.resolve()) for root in roots):
        raise ValueError("bundle output must be outside its source trees")
    staged = staged_output_path(output, purpose="bundle")
    manifest: BundleManifest = {"files": [], "total_bytes": 0}
    try:
        with publication_payload_snapshot(roots, include=include) as snapshot:
            entries = _bundle_entries(snapshot)
            if not entries and omit_empty:
                return None
            with tarfile.open(
                staged,
                "w",
                format=tarfile.USTAR_FORMAT,
                encoding="utf-8",
                errors="strict",
            ) as archive:
                for name, path in entries:
                    with open_stable_regular_file(
                        path, label="bundle payload"
                    ) as opened:
                        info = _header(name, opened.stat.st_size)
                        archive.addfile(info, opened.stream)
                    manifest["files"].append({"path": name, "size": info.size})
                    manifest["total_bytes"] += info.size
                payload = json.dumps(manifest, indent=2, sort_keys=True).encode("utf-8")
                archive.addfile(
                    _header("__manifest__.json", len(payload)), io.BytesIO(payload)
                )
        publish_validated_outputs([(staged, output)])
        return manifest
    finally:
        discard_staged_output(staged)
