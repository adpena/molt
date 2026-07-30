"""Canonical content identity capture for proof process images."""

from __future__ import annotations

import hashlib
from pathlib import Path
import sys
from typing import Mapping, Sequence


PROCESS_IMAGE_SCHEMA = "molt.proof-process-image-capture.v1"
_ROOT_EXIT_DISPOSITIONS = frozenset({"require-exit", "terminate"})


def capture_image(
    role: str,
    path: Path,
    root_exit_disposition: str = "require-exit",
) -> dict[str, object]:
    """Capture one exact executable image without selecting it through PATH."""

    if not isinstance(role, str) or not role:
        raise ValueError("process image role must be non-empty")
    if root_exit_disposition not in _ROOT_EXIT_DISPOSITIONS:
        raise ValueError(
            "process image has invalid root-exit disposition: "
            f"{root_exit_disposition!r}"
        )
    resolved = path.resolve(strict=True)
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
        if not isinstance(disposition, str) or disposition not in _ROOT_EXIT_DISPOSITIONS:
            raise ValueError("process image has invalid root-exit disposition")
        current = capture_image(role, Path(raw_path), disposition)
        if current != dict(raw):
            raise ValueError(
                f"process image changed while live custody armed: {raw_path}"
            )
        current_rows.append(current)
    return current_rows


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
