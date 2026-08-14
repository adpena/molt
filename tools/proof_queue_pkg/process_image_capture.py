"""Canonical content identity capture for proof process images."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import sys
from typing import Mapping, Sequence


PROCESS_IMAGE_SCHEMA = "molt.proof-process-image-capture.v1"
_ROOT_EXIT_DISPOSITIONS = frozenset({"require-exit", "terminate"})
_SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")


def _filesystem_path(path: Path) -> Path:
    """Normalize kernel-reported Windows device paths without deriving layout."""

    raw = os.fspath(path)
    if os.name == "nt" and raw.startswith("\\\\?\\UNC\\"):
        return Path("\\\\" + raw[8:])
    if os.name == "nt" and raw.startswith("\\\\?\\"):
        return Path(raw[4:])
    return path


def capture_image(
    role: str,
    path: Path,
    root_exit_disposition: str = "require-exit",
    *,
    preserve_path: bool = False,
) -> dict[str, object]:
    """Capture one exact executable image without selecting it through PATH."""

    if not isinstance(role, str) or not role:
        raise ValueError("process image role must be non-empty")
    if root_exit_disposition not in _ROOT_EXIT_DISPOSITIONS:
        raise ValueError(
            "process image has invalid root-exit disposition: "
            f"{root_exit_disposition!r}"
        )
    selected = _filesystem_path(path)
    resolved = (
        Path(os.path.abspath(selected))
        if preserve_path
        else selected.resolve(strict=True)
    )
    if not resolved.is_file():
        raise ValueError(f"process image is not a file: {resolved}")
    file_stat = resolved.stat()
    with resolved.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    image: dict[str, object] = {
        "schema": PROCESS_IMAGE_SCHEMA,
        "role": role,
        "path": str(resolved),
        "sha256": digest,
        "size_bytes": file_stat.st_size,
    }
    if root_exit_disposition != "require-exit":
        image["root_exit_disposition"] = root_exit_disposition
    if preserve_path:
        image["path_kind"] = "selection"
    return image


def revalidate_images(
    rows: Sequence[Mapping[str, object]],
) -> list[dict[str, object]]:
    """Rehash captured images by exact path and reject any identity drift."""

    current_rows: list[dict[str, object]] = []
    for raw in rows:
        if not isinstance(raw, Mapping):
            raise ValueError("process image row is malformed")
        if raw.get("schema") != PROCESS_IMAGE_SCHEMA:
            raise ValueError("process image schema mismatch")
        role = raw.get("role")
        if not isinstance(role, str) or not role:
            raise ValueError("process image has no role")
        raw_path = raw.get("path")
        if not isinstance(raw_path, str) or not Path(raw_path).is_absolute():
            raise ValueError("process image has no absolute path")
        disposition = raw.get("root_exit_disposition", "require-exit")
        path_kind = raw.get("path_kind", "resolved")
        if (
            not isinstance(disposition, str)
            or disposition not in _ROOT_EXIT_DISPOSITIONS
        ):
            raise ValueError("process image has invalid root-exit disposition")
        if path_kind not in {"resolved", "selection"}:
            raise ValueError("process image has invalid path kind")
        current = capture_image(
            role,
            Path(raw_path),
            disposition,
            preserve_path=path_kind == "selection",
        )
        if current != dict(raw):
            raise ValueError(
                f"process image changed while live custody armed: {raw_path}"
            )
        current_rows.append(current)
    return current_rows


def canonical_images(
    rows: Sequence[Mapping[str, object]],
) -> list[dict[str, object]]:
    """Validate and deterministically order one exact executable-image set."""

    identities: dict[str, tuple[str, str]] = {}
    canonical: dict[tuple[str, str], dict[str, object]] = {}
    for raw in rows:
        if not isinstance(raw, Mapping) or raw.get("schema") != PROCESS_IMAGE_SCHEMA:
            raise ValueError("process image row is malformed")
        role = raw.get("role")
        raw_path = raw.get("path")
        digest = raw.get("sha256")
        size = raw.get("size_bytes")
        disposition = raw.get("root_exit_disposition", "require-exit")
        path_kind = raw.get("path_kind", "resolved")
        if not isinstance(role, str) or not role:
            raise ValueError("process image has no role")
        if not isinstance(raw_path, str) or not Path(raw_path).is_absolute():
            raise ValueError("process image has no absolute path")
        if not isinstance(digest, str) or _SHA256_PATTERN.fullmatch(digest) is None:
            raise ValueError("process image has no SHA-256 identity")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ValueError("process image has no non-negative size")
        if (
            not isinstance(disposition, str)
            or disposition not in _ROOT_EXIT_DISPOSITIONS
        ):
            raise ValueError("process image has invalid root-exit disposition")
        if path_kind not in {"resolved", "selection"}:
            raise ValueError("process image has invalid path kind")
        path = Path(raw_path)
        if not path.is_file():
            raise ValueError(f"process image is unavailable: {path}")
        normalized = os.path.normcase(os.path.abspath(path))
        identity = (digest, disposition)
        prior = identities.get(normalized)
        if prior is not None and prior != identity:
            raise ValueError(f"process image has conflicting identities: {path}")
        identities[normalized] = identity
        row: dict[str, object] = {
            "schema": PROCESS_IMAGE_SCHEMA,
            "role": role,
            "path": str(path),
            "sha256": digest,
            "size_bytes": size,
        }
        if disposition != "require-exit":
            row["root_exit_disposition"] = disposition
        if path_kind == "selection":
            row["path_kind"] = "selection"
        key = (normalized, role)
        prior_row = canonical.get(key)
        if prior_row is not None:
            prior_kind = prior_row.get("path_kind", "resolved")
            if prior_kind == "selection":
                continue
            if path_kind != "selection":
                continue
        canonical[key] = row
    return [canonical[key] for key in sorted(canonical)]


def toolchain_images(
    name: str, identity: Mapping[str, object]
) -> list[dict[str, object]]:
    """Project one toolchain identity into its sole exact-image authority."""

    raw_images = identity.get("process_images")
    if name == "python" and raw_images is None:
        raw_path = identity.get("executable")
        digest = identity.get("executable_sha256")
        if not isinstance(raw_path, str) or not isinstance(digest, str):
            raise ValueError("python toolchain has no executable image identity")
        image = capture_image("python", Path(raw_path), preserve_path=True)
        if image["sha256"] != digest:
            raise ValueError(
                "python executable image disagrees with toolchain identity"
            )
        return [image]
    if not isinstance(raw_images, list) or not raw_images:
        raise ValueError(f"{name} toolchain has no process-image closure")
    images = canonical_images(raw_images)

    required: list[tuple[object, object, str]] = []
    if name != "python":
        required.extend(
            (
                (identity.get("path"), identity.get("launcher_sha256"), "launcher"),
                (
                    identity.get("content_path"),
                    identity.get("executable_sha256"),
                    "content",
                ),
            )
        )
    for raw_path, digest, label in required:
        if not isinstance(raw_path, str) or not isinstance(digest, str):
            raise ValueError(f"{name} toolchain has no {label} image identity")
        normalized = os.path.normcase(os.path.abspath(raw_path))
        if not any(
            os.path.normcase(os.path.abspath(str(image["path"]))) == normalized
            and image["sha256"] == digest
            for image in images
        ):
            raise ValueError(
                f"{name} toolchain {label} image is outside its process closure"
            )
    return images


def platform_auxiliary_images(descendants: object) -> list[dict[str, object]]:
    """Capture exact OS broker images admitted by a declared-toolchain tree."""

    if sys.platform != "win32" or descendants != "declared-toolchains":
        return []

    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    get_system_directory = kernel32.GetSystemDirectoryW
    get_system_directory.argtypes = [wintypes.LPWSTR, wintypes.UINT]
    get_system_directory.restype = wintypes.UINT
    buffer = ctypes.create_unicode_buffer(32_768)
    length = int(get_system_directory(buffer, len(buffer)))
    if length <= 0 or length >= len(buffer):
        raise OSError(
            ctypes.get_last_error(), "GetSystemDirectoryW failed for proof custody"
        )
    return [
        capture_image(
            "windows-console-broker",
            Path(buffer.value) / "conhost.exe",
            root_exit_disposition="terminate",
        )
    ]
