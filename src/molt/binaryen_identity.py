"""Content identity for one manifest-provisioned Binaryen filesystem tree."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
from typing import Any, Iterable, TypeGuard

from molt.exact_json import canonical_json_bytes, loads_exact
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.process_guard import run_completed_command
from molt.toolchain_identity import stable_native_executable_probe


INSTALL_RECEIPT_FILENAME = ".molt-binaryen-source.json"
INSTALL_RECEIPT_SCHEMA = "molt.binaryen-install.v3"
TREE_IDENTITY_SCHEMA = "molt.binaryen-tree.v3"
MAX_TREE_ENTRIES = 10_000
MAX_TREE_BYTES = 2 * 1024 * 1024 * 1024
_HEX = frozenset("0123456789abcdef")
_VERSION_RE = re.compile(r"[1-9]\d*")
_VERSION_OUTPUT_RE = re.compile(r"wasm-opt version ([1-9]\d*) \(version_\1\)")
_ASSET_ID_RE = re.compile(r"(?:linux|macos|windows)-(?:x86_64|aarch64)")


class BinaryenIdentityError(ValueError):
    """Raised when a Binaryen tree or its provision receipt is not exact."""


def is_binaryen_version(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _VERSION_RE.fullmatch(value) is not None


def parse_binaryen_version_output(output: str) -> tuple[str, str]:
    """Parse Binaryen's one canonical ``wasm-opt --version`` record."""

    canonical = output[:-1] if output.endswith("\n") else output
    match = _VERSION_OUTPUT_RE.fullmatch(canonical)
    if match is None:
        raise BinaryenIdentityError(
            f"provisioned wasm-opt reported an invalid version: {output!r}"
        )
    return match.group(1), canonical


def is_binaryen_version_output(value: object) -> TypeGuard[str]:
    """Return whether *value* is one canonical Binaryen version record."""

    if not isinstance(value, str):
        return False
    try:
        _, canonical = parse_binaryen_version_output(value)
    except BinaryenIdentityError:
        return False
    return canonical == value


@dataclass(frozen=True, slots=True)
class BinaryenTreeIdentity:
    entries: int
    total_bytes: int
    sha256: str

    def as_record(self) -> dict[str, object]:
        return {
            "schema": TREE_IDENTITY_SCHEMA,
            "entries": self.entries,
            "total_bytes": self.total_bytes,
            "sha256": self.sha256,
        }


@dataclass(frozen=True, slots=True)
class BinaryenInstallationIdentity:
    """One filesystem scan's whole-tree and selected executable identities."""

    tree: BinaryenTreeIdentity
    executable_sha256: str


@dataclass(frozen=True, slots=True)
class BinaryenTreeEntry:
    """One canonical extracted-tree entry before whole-tree hashing."""

    path: str
    kind: str
    size: int
    mode: int
    sha256: str


def binaryen_tree_identity_from_entries(
    entries: Iterable[BinaryenTreeEntry],
) -> BinaryenTreeIdentity:
    """Hash validated tree records through the single Binaryen identity grammar."""

    records: list[tuple[str, str, int, int, str]] = []
    identities: set[str] = set()
    total_bytes = 0
    for entry in entries:
        if entry.path == ".":
            relative_text = "."
            identity = "."
        else:
            try:
                relative_text = portable_relative_path(entry.path).as_posix()
                identity = portable_path_identity(entry.path)
            except ValueError as exc:
                raise BinaryenIdentityError(
                    f"Binaryen path is not portable: {entry.path}"
                ) from exc
        if identity in identities:
            raise BinaryenIdentityError(
                f"Binaryen has a portable path collision: {entry.path}"
            )
        identities.add(identity)
        if len(identities) > MAX_TREE_ENTRIES:
            raise BinaryenIdentityError("Binaryen tree exceeds its entry policy")
        if (
            type(entry.mode) is not int
            or entry.mode < 0
            or stat.S_IMODE(entry.mode) != entry.mode
        ):
            raise BinaryenIdentityError(
                f"Binaryen tree entry has an invalid mode: {entry.path}"
            )
        if entry.kind == "directory":
            if entry.size != 0 or entry.sha256:
                raise BinaryenIdentityError(
                    f"Binaryen directory identity is invalid: {entry.path}"
                )
        elif entry.kind == "file":
            if (
                type(entry.size) is not int
                or entry.size < 0
                or not _is_sha256(entry.sha256)
            ):
                raise BinaryenIdentityError(
                    f"Binaryen file identity is invalid: {entry.path}"
                )
            total_bytes += entry.size
            if total_bytes > MAX_TREE_BYTES:
                raise BinaryenIdentityError(
                    "Binaryen tree exceeds its total-byte policy"
                )
        else:
            raise BinaryenIdentityError(
                f"Binaryen tree entry has an invalid kind: {entry.path}"
            )
        if entry.path == "." and entry.kind != "directory":
            raise BinaryenIdentityError("Binaryen tree root is not a directory")
        records.append(
            (
                relative_text,
                entry.kind,
                entry.size,
                entry.mode,
                entry.sha256,
            )
        )
    if not records:
        raise BinaryenIdentityError("Binaryen tree is empty")

    digest = hashlib.sha256()
    for record in sorted(records):
        digest.update(canonical_json_bytes(list(record)))
        digest.update(b"\n")
    return BinaryenTreeIdentity(
        entries=len(records),
        total_bytes=total_bytes,
        sha256=digest.hexdigest(),
    )


def _is_junction(path: Path) -> bool:
    predicate = getattr(path, "is_junction", None)
    return bool(predicate is not None and predicate())


def _sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_binaryen_version(executable: Path, *, expected_sha256: str) -> tuple[str, str]:
    """Probe one manifest-admitted native executable without changing identity."""

    try:
        with stable_native_executable_probe(
            executable, label="provisioned wasm-opt"
        ) as (entrypoint, identity):
            if identity.sha256 != expected_sha256:
                raise BinaryenIdentityError(
                    "Binaryen executable differs from its manifest identity"
                )
            # This is the shared bounded metadata-probe mode. Preserve stdout
            # and strict UTF-8 rather than accepting a combined stream identity.
            result = run_completed_command(
                [str(entrypoint), "--version"],
                memory_guard_prefix=None,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="strict",
                timeout=30,
            )
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        raise BinaryenIdentityError(
            f"provisioned wasm-opt cannot report its version: {exc}"
        ) from exc
    if result.returncode != 0 or result.stderr:
        raise BinaryenIdentityError(
            "provisioned wasm-opt version probe failed: "
            f"exit={result.returncode} stderr={result.stderr.strip()!r}"
        )
    return parse_binaryen_version_output(result.stdout)


def _scan_binaryen_tree(
    root: Path,
    *,
    include_modes: bool,
    executable: str | None,
) -> tuple[BinaryenTreeIdentity, str | None]:
    """Hash one tree once and retain a selected executable digest when requested."""

    lexical_root = root.absolute()
    if (
        not lexical_root.is_dir()
        or lexical_root.is_symlink()
        or _is_junction(lexical_root)
    ):
        raise BinaryenIdentityError(
            f"Binaryen root is not a real directory: {lexical_root}"
        )
    root = lexical_root.resolve(strict=True)

    pending = [root]
    try:
        root_mode = stat.S_IMODE(root.stat().st_mode) if include_modes else 0
    except OSError as exc:
        raise BinaryenIdentityError(
            f"Binaryen root is unreadable: {root}: {exc}"
        ) from exc
    records: list[BinaryenTreeEntry] = [
        BinaryenTreeEntry(
            path=".",
            kind="directory",
            size=0,
            mode=root_mode,
            sha256="",
        )
    ]
    executable_sha256: str | None = None
    while pending:
        directory = pending.pop()
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda item: item.name)
        except OSError as exc:
            raise BinaryenIdentityError(
                f"Binaryen directory is unreadable: {directory}: {exc}"
            ) from exc
        for entry in entries:
            path = Path(entry.path)
            relative_text = path.relative_to(root).as_posix()
            if relative_text == INSTALL_RECEIPT_FILENAME:
                continue
            try:
                relative = portable_relative_path(relative_text)
            except ValueError as exc:
                raise BinaryenIdentityError(
                    f"Binaryen path is not portable: {relative_text}"
                ) from exc
            if entry.is_symlink() or _is_junction(path):
                raise BinaryenIdentityError(
                    f"Binaryen tree contains an unsupported link: {relative_text}"
                )
            if entry.is_dir(follow_symlinks=False):
                try:
                    mode = (
                        stat.S_IMODE(entry.stat(follow_symlinks=False).st_mode)
                        if include_modes
                        else 0
                    )
                except OSError as exc:
                    raise BinaryenIdentityError(
                        f"Binaryen directory is unreadable: {relative_text}: {exc}"
                    ) from exc
                records.append(
                    BinaryenTreeEntry(
                        path=relative.as_posix(),
                        kind="directory",
                        size=0,
                        mode=mode,
                        sha256="",
                    )
                )
                pending.append(path)
            elif entry.is_file(follow_symlinks=False):
                try:
                    file_stat = entry.stat(follow_symlinks=False)
                    size = file_stat.st_size
                    mode = stat.S_IMODE(file_stat.st_mode) if include_modes else 0
                    digest = _sha256_file(path)
                except OSError as exc:
                    raise BinaryenIdentityError(
                        f"Binaryen file is unreadable: {relative_text}: {exc}"
                    ) from exc
                records.append(
                    BinaryenTreeEntry(
                        path=relative.as_posix(),
                        kind="file",
                        size=size,
                        mode=mode,
                        sha256=digest,
                    )
                )
                if relative.as_posix() == executable:
                    executable_sha256 = digest
            else:
                raise BinaryenIdentityError(
                    f"Binaryen tree contains a special node: {relative_text}"
                )

    return binaryen_tree_identity_from_entries(records), executable_sha256


def binaryen_tree_identity(
    root: Path,
    *,
    include_modes: bool,
) -> BinaryenTreeIdentity:
    """Hash every Binaryen path and file byte without following links."""

    return _scan_binaryen_tree(
        root,
        include_modes=include_modes,
        executable=None,
    )[0]


def binaryen_installation_identity(
    root: Path,
    *,
    include_modes: bool,
    executable: str,
) -> BinaryenInstallationIdentity:
    """Hash a Binaryen tree and its wasm-opt bytes in one filesystem pass."""

    try:
        executable_path = portable_relative_path(executable).as_posix()
    except ValueError as exc:
        raise BinaryenIdentityError(
            f"Binaryen executable path is not portable: {executable}"
        ) from exc
    tree, executable_sha256 = _scan_binaryen_tree(
        root,
        include_modes=include_modes,
        executable=executable_path,
    )
    if executable_sha256 is None:
        raise BinaryenIdentityError(
            f"Binaryen tree lacks its selected executable: {executable_path}"
        )
    return BinaryenInstallationIdentity(
        tree=tree,
        executable_sha256=executable_sha256,
    )


def validate_binaryen_install_receipt(payload: object) -> dict[str, Any]:
    """Validate and return one exact Binaryen provision receipt."""

    if not isinstance(payload, dict) or set(payload) != {"schema", "asset", "tree"}:
        raise BinaryenIdentityError("Binaryen provision receipt keys are not exact")
    asset = payload["asset"]
    tree = payload["tree"]
    asset_keys = {
        "id",
        "version",
        "url",
        "size",
        "sha256",
        "archive_root",
        "executable",
        "tree_entries",
        "tree_total_bytes",
        "tree_sha256",
        "executable_sha256",
        "record_sha256",
    }
    valid = (
        payload["schema"] == INSTALL_RECEIPT_SCHEMA
        and isinstance(asset, dict)
        and set(asset) == asset_keys
        and isinstance(asset.get("id"), str)
        and _ASSET_ID_RE.fullmatch(asset["id"]) is not None
        and is_binaryen_version(asset.get("version"))
        and asset.get("archive_root") == f"binaryen-version_{asset.get('version')}"
        and isinstance(asset.get("url"), str)
        and asset["url"].startswith("https://")
        and type(asset.get("size")) is int
        and asset["size"] > 0
        and _is_sha256(asset.get("sha256"))
        and isinstance(asset.get("executable"), str)
        and type(asset.get("tree_entries")) is int
        and 0 < asset["tree_entries"] <= MAX_TREE_ENTRIES
        and type(asset.get("tree_total_bytes")) is int
        and 0 < asset["tree_total_bytes"] <= MAX_TREE_BYTES
        and _is_sha256(asset.get("tree_sha256"))
        and _is_sha256(asset.get("executable_sha256"))
        and _is_sha256(asset.get("record_sha256"))
        and isinstance(tree, dict)
        and set(tree) == {"schema", "entries", "total_bytes", "sha256"}
        and tree.get("schema") == TREE_IDENTITY_SCHEMA
        and type(tree.get("entries")) is int
        and 0 < tree["entries"] <= MAX_TREE_ENTRIES
        and type(tree.get("total_bytes")) is int
        and 0 < tree["total_bytes"] <= MAX_TREE_BYTES
        and _is_sha256(tree.get("sha256"))
        and tree.get("entries") == asset.get("tree_entries")
        and tree.get("total_bytes") == asset.get("tree_total_bytes")
        and tree.get("sha256") == asset.get("tree_sha256")
    )
    try:
        executable = portable_relative_path(asset.get("executable")) if valid else None
    except ValueError:
        executable = None
    expected_executable = (
        "bin/wasm-opt.exe"
        if isinstance(asset, dict)
        and isinstance(asset.get("id"), str)
        and asset["id"].startswith("windows-")
        else "bin/wasm-opt"
    )
    if not valid or executable is None or executable.as_posix() != expected_executable:
        raise BinaryenIdentityError("Binaryen provision receipt identity is invalid")
    asset_record = {key: asset[key] for key in asset_keys - {"record_sha256"}}
    expected_record_sha256 = hashlib.sha256(
        canonical_json_bytes(asset_record)
    ).hexdigest()
    if asset["record_sha256"] != expected_record_sha256:
        raise BinaryenIdentityError(
            "Binaryen provision receipt record identity is invalid"
        )
    return payload


def _is_sha256(value: object) -> TypeGuard[str]:
    return (
        isinstance(value, str)
        and len(value) == 64
        and not any(character not in _HEX for character in value)
    )


def load_binaryen_install_receipt(root: Path) -> dict[str, Any]:
    """Decode and validate the exact provision receipt schema."""

    path = root / INSTALL_RECEIPT_FILENAME
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise BinaryenIdentityError(
            f"Binaryen provision receipt is invalid: {path}: {exc}"
        ) from exc
    return validate_binaryen_install_receipt(payload)
