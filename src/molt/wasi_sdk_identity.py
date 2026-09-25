"""Content identity for one manifest-provisioned wasi-sdk installation.

An installation is an identity-addressed prefix owned by
``tools/provision_wasi_sdk.py``::

    <prefix>/.molt-wasi-sdk.json   provision receipt (host asset + tree identity)
    <prefix>/sdk/                  the extracted wasi-sdk archive root

This module is stdlib-only so CI can verify an installation before the optional
Python dependency set exists.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
from typing import Any, TypeGuard

from molt.exact_json import canonical_json_bytes, dumps_exact, read_exact
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import open_stable_regular_file


INSTALL_RECEIPT_FILENAME = ".molt-wasi-sdk.json"
INSTALL_RECEIPT_SCHEMA = "molt.wasi-sdk-install.v1"
TREE_IDENTITY_SCHEMA = "molt.wasi-sdk-tree.v1"
SDK_DIRNAME = "sdk"
SDK_CARGO_TOOLS = (
    ("CC", "clang"),
    ("CXX", "clang++"),
    ("AR", "llvm-ar"),
    ("RANLIB", "llvm-ranlib"),
)
SDK_TOOL_NAMES = (*(name for _, name in SDK_CARGO_TOOLS), "wasm-ld", "llvm-nm")
SDK_CARGO_TARGETS = ("wasm32-wasip1", "wasm32-unknown-unknown")
# Admission bounds, not measured sizes: the largest pinned archive (Windows)
# materializes its tool aliases as copies, so the byte bound leaves headroom.
MAX_TREE_ENTRIES = 100_000
MAX_TREE_BYTES = 8 * 1024 * 1024 * 1024
ASSET_RECORD_KEYS = frozenset(
    {
        "id",
        "sdk_version",
        "llvm_version",
        "url",
        "size",
        "sha256",
        "archive_root",
        "provenance_url",
        "record_sha256",
    }
)
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_HOST_ID_RE = re.compile(r"(?:linux|macos|windows)-(?:x86_64|aarch64)")
_SDK_VERSION_RE = re.compile(r"\d+\.\d+(?:\+[A-Za-z0-9][A-Za-z0-9._-]*)?")
_LLVM_VERSION_RE = re.compile(r"\d+\.\d+\.\d+")


class WasiSdkIdentityError(ValueError):
    """Raised when an SDK tree or its provision receipt is not exact."""


def is_wasi_sdk_version(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _SDK_VERSION_RE.fullmatch(value) is not None


def is_wasi_sdk_llvm_version(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _LLVM_VERSION_RE.fullmatch(value) is not None


def is_wasi_sdk_host_id(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _HOST_ID_RE.fullmatch(value) is not None


def _is_sha256(value: object) -> bool:
    return isinstance(value, str) and _SHA256_RE.fullmatch(value) is not None


def executable_filename(name: str, host_id: str) -> str:
    """Spell one SDK executable for the host that runs it."""

    return f"{name}.exe" if host_id.startswith("windows-") else name


@dataclass(frozen=True, slots=True)
class WasiSdkVersionIdentity:
    sdk_version: str
    llvm_version: str


@dataclass(frozen=True, slots=True)
class WasiSdkTreeIdentity:
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


def read_wasi_sdk_version_identity(version_file: Path) -> WasiSdkVersionIdentity:
    """Read the exact SDK and LLVM producer identities from ``VERSION``."""

    try:
        with open_stable_regular_file(version_file, label="WASI SDK VERSION") as opened:
            if opened.stat.st_size > 64 * 1024:
                raise WasiSdkIdentityError("WASI SDK VERSION exceeds its size limit")
            raw = opened.stream.read(64 * 1024 + 1)
            if len(raw) > 64 * 1024:
                raise WasiSdkIdentityError("WASI SDK VERSION exceeds its size limit")
        version_text = raw.decode("utf-8", errors="strict")
    except (OSError, ValueError) as exc:
        raise WasiSdkIdentityError(
            f"WASI SDK VERSION is unavailable: {version_file}: {exc}"
        ) from exc
    version_lines = version_text.splitlines()
    sdk_version = version_lines[0].strip() if version_lines else ""
    llvm_versions = tuple(
        match.group(1)
        for line in version_lines[1:]
        if (match := re.fullmatch(r"llvm-version:\s*(\d+\.\d+\.\d+)\s*", line))
    )
    if not is_wasi_sdk_version(sdk_version):
        raise WasiSdkIdentityError(
            f"WASI SDK has no valid SDK identity at {version_file}"
        )
    if len(llvm_versions) != 1:
        raise WasiSdkIdentityError(
            f"WASI SDK has no unique LLVM producer identity at {version_file}"
        )
    return WasiSdkVersionIdentity(
        sdk_version=sdk_version,
        llvm_version=llvm_versions[0],
    )


def wasi_sdk_tree_identity(root: Path) -> WasiSdkTreeIdentity:
    """Hash every SDK path, file byte, and link target without following links."""

    lexical_root = root.absolute()
    if (
        not lexical_root.is_dir()
        or lexical_root.is_symlink()
        or lexical_root.is_junction()
    ):
        raise WasiSdkIdentityError(
            f"WASI SDK root is not a real directory: {lexical_root}"
        )
    root = lexical_root.resolve(strict=True)

    pending = [root]
    identities: set[str] = set()
    records: list[tuple[str, str, int, str]] = []
    total_bytes = 0
    while pending:
        directory = pending.pop()
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda item: item.name)
        except OSError as exc:
            raise WasiSdkIdentityError(
                f"WASI SDK directory is unreadable: {directory}: {exc}"
            ) from exc
        for entry in entries:
            path = Path(entry.path)
            relative_text = path.relative_to(root).as_posix()
            try:
                relative = portable_relative_path(relative_text)
                portable_identity = portable_path_identity(relative_text)
            except ValueError as exc:
                raise WasiSdkIdentityError(
                    f"WASI SDK path is not portable: {relative_text}"
                ) from exc
            if portable_identity in identities:
                raise WasiSdkIdentityError(
                    f"WASI SDK has a portable path collision: {relative_text}"
                )
            identities.add(portable_identity)
            if len(identities) > MAX_TREE_ENTRIES:
                raise WasiSdkIdentityError("WASI SDK tree exceeds its entry policy")

            if entry.is_symlink():
                try:
                    link_target = os.readlink(path)
                    path.resolve(strict=True).relative_to(root)
                except (OSError, RuntimeError, ValueError) as exc:
                    raise WasiSdkIdentityError(
                        f"WASI SDK link escapes its root or dangles: {relative_text}"
                    ) from exc
                records.append((relative.as_posix(), "link", 0, link_target))
            elif path.is_junction():
                raise WasiSdkIdentityError(
                    f"WASI SDK contains an unsupported junction: {relative_text}"
                )
            elif entry.is_dir(follow_symlinks=False):
                records.append((relative.as_posix(), "directory", 0, ""))
                pending.append(path)
            elif entry.is_file(follow_symlinks=False):
                try:
                    with open_stable_regular_file(
                        path, label="WASI SDK file"
                    ) as opened:
                        size = opened.stat.st_size
                        if total_bytes + size > MAX_TREE_BYTES:
                            raise WasiSdkIdentityError(
                                "WASI SDK tree exceeds its total-byte policy"
                            )
                        digest = hashlib.file_digest(
                            opened.stream, "sha256"
                        ).hexdigest()
                except (OSError, ValueError) as exc:
                    raise WasiSdkIdentityError(
                        f"WASI SDK file is unreadable: {relative_text}: {exc}"
                    ) from exc
                total_bytes += size
                records.append((relative.as_posix(), "file", size, digest))
            else:
                raise WasiSdkIdentityError(
                    f"WASI SDK contains a special node: {relative_text}"
                )

    digest = hashlib.sha256()
    for path, kind, size, content_identity in sorted(records):
        record = [path, kind, size, content_identity]
        digest.update(canonical_json_bytes(record))
        digest.update(b"\n")
    return WasiSdkTreeIdentity(
        entries=len(records),
        total_bytes=total_bytes,
        sha256=digest.hexdigest(),
    )


def render_wasi_sdk_install_receipt(
    asset: dict[str, object],
    tree: WasiSdkTreeIdentity,
) -> str:
    """Render the one canonical receipt encoding the loader accepts."""

    payload = {
        "schema": INSTALL_RECEIPT_SCHEMA,
        "asset": asset,
        "tree": tree.as_record(),
    }
    return dumps_exact(payload, indent=None)


def load_wasi_sdk_install_receipt(prefix: Path) -> dict[str, Any]:
    """Decode and validate the exact provision receipt schema."""

    path = prefix / INSTALL_RECEIPT_FILENAME
    try:
        payload = read_exact(
            path, max_bytes=64 * 1024, label="WASI SDK provision receipt"
        )
    except (OSError, ValueError) as exc:
        raise WasiSdkIdentityError(
            f"WASI SDK provision receipt is invalid: {path}: {exc}"
        ) from exc
    if not isinstance(payload, dict) or set(payload) != {"schema", "asset", "tree"}:
        raise WasiSdkIdentityError("WASI SDK provision receipt keys are not exact")
    asset = payload["asset"]
    tree = payload["tree"]
    if (
        payload["schema"] != INSTALL_RECEIPT_SCHEMA
        or not isinstance(asset, dict)
        or set(asset) != ASSET_RECORD_KEYS
        or not is_wasi_sdk_host_id(asset["id"])
        or not is_wasi_sdk_version(asset["sdk_version"])
        or not is_wasi_sdk_llvm_version(asset["llvm_version"])
        or not isinstance(asset["url"], str)
        or not asset["url"].startswith("https://")
        or type(asset["size"]) is not int
        or asset["size"] <= 0
        or not _is_sha256(asset["sha256"])
        or not isinstance(asset["archive_root"], str)
        or not asset["archive_root"]
        or not isinstance(asset["provenance_url"], str)
        or not asset["provenance_url"].startswith("https://")
        or not _is_sha256(asset["record_sha256"])
        or not isinstance(tree, dict)
        or set(tree) != {"schema", "entries", "total_bytes", "sha256"}
        or tree["schema"] != TREE_IDENTITY_SCHEMA
        or type(tree["entries"]) is not int
        or not 0 < tree["entries"] <= MAX_TREE_ENTRIES
        or type(tree["total_bytes"]) is not int
        or not 0 < tree["total_bytes"] <= MAX_TREE_BYTES
        or not _is_sha256(tree["sha256"])
    ):
        raise WasiSdkIdentityError("WASI SDK provision receipt identity is invalid")
    return payload
