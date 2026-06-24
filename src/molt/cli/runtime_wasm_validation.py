from __future__ import annotations

from pathlib import Path
import shutil

from molt._wasm_runtime_exports import wasm_runtime_missing_required_exports
from molt.cli.atomic_io import _atomic_write_text
from molt.cli.command_runtime import _run_completed_command
from molt.cli.file_hashing import _sha256_file
from molt.cli.runtime_fingerprints import _inspect_wasm_binary
from molt.cli.wasm import _collect_wasm_export_names, _wasm_import_minima


def _runtime_wasm_integrity_sidecar_path(path: Path) -> Path:
    return path.with_name(f"{path.name}.sha256")


def _write_runtime_wasm_integrity_sidecar(path: Path) -> None:
    digest = _sha256_file(path)
    sidecar = _runtime_wasm_integrity_sidecar_path(path)
    _atomic_write_text(sidecar, f"{digest}\n")


def _try_read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int] | None:
    value = 0
    shift = 0
    while offset < len(data):
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return value, offset
        shift += 7
        if shift > 35:
            return None
    return None


def _wasm_has_nonempty_code_section(path: Path) -> bool:
    try:
        data = path.read_bytes()
    except OSError:
        return False
    if len(data) < 8 or data[:8] != b"\x00asm\x01\x00\x00\x00":
        return False
    offset = 8
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        size_info = _try_read_wasm_varuint(data, offset)
        if size_info is None:
            return False
        section_size, offset = size_info
        payload_end = offset + section_size
        if payload_end > len(data):
            return False
        if section_id == 10:
            count_info = _try_read_wasm_varuint(data, offset)
            return bool(count_info and count_info[0] > 0)
        offset = payload_end
    return False


def _validate_wasm_structural(path: Path) -> str | None:
    exe = shutil.which("wasm-tools")
    if exe is None:
        return None
    try:
        result = _run_completed_command(
            [exe, "validate", str(path)],
            capture_output=True,
            timeout=60,
            env=None,
            cwd=path.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
    except Exception as exc:
        return f"wasm-tools validate failed to run: {exc}"
    if result.returncode == 0:
        return None
    detail = (result.stderr or result.stdout).strip()
    return f"wasm-tools validate failed: {detail}"


def _is_reusable_wasm_artifact(path: Path) -> bool:
    if _inspect_wasm_binary(path) != "valid":
        return False
    structural_error = _validate_wasm_structural(path)
    return structural_error is None


def _is_valid_runtime_wasm_artifact(path: Path) -> bool:
    return _is_reusable_wasm_artifact(path) and _wasm_has_nonempty_code_section(path)


def _runtime_wasm_has_shared_import_abi(path: Path) -> bool:
    try:
        memory_min, table_min = _wasm_import_minima(path)
    except (OSError, ValueError):
        return False
    return memory_min is not None and table_min is not None


def _is_valid_shared_runtime_wasm_artifact(path: Path) -> bool:
    return _is_valid_runtime_wasm_artifact(
        path
    ) and _runtime_wasm_has_shared_import_abi(path)


def _runtime_wasm_exports_satisfy(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    return not _runtime_wasm_missing_exports(path, required_exports)


def _runtime_wasm_missing_exports(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> set[str]:
    export_names = _collect_wasm_export_names(path)
    if not export_names and required_exports:
        return {
            name if name.startswith("molt_") else f"molt_{name}"
            for name in required_exports
        }
    return wasm_runtime_missing_required_exports(export_names, required_exports)
