from __future__ import annotations

import re
from collections.abc import Mapping
from pathlib import Path
import shutil

from molt._wasm_runtime_exports import (
    wasm_runtime_missing_required_exports,
    wasm_split_runtime_missing_required_exports,
)
from molt.cli.atomic_io import _atomic_write_text
from molt.cli.command_runtime import _run_completed_command
from molt.cli.file_hashing import _sha256_file
from molt.wasm_artifact import (
    _collect_wasm_export_names,
    _wasm_import_minima,
    has_nonempty_wasm_code_section,
    inspect_wasm_binary,
)


_RUNTIME_WASM_INTEGRITY_KEY_RE = re.compile(r"\A[0-9a-f]{64}\Z")


def _runtime_wasm_integrity_key(fingerprint: Mapping[str, object]) -> str:
    """Return the integrity-pin key for one resolved runtime build identity.

    The key is the runtime fingerprint ``meta_digest`` (cargo profile, target,
    rustflags, and resolved feature set), computed by
    ``molt.cli.runtime_fingerprints._runtime_fingerprint``. It is the single
    authority for build identity, so builds with different resolved
    profile/feature identities publish to distinct pin slots instead of
    contending for one.
    """
    meta_digest = fingerprint.get("meta_digest")
    if (
        not isinstance(meta_digest, str)
        or _RUNTIME_WASM_INTEGRITY_KEY_RE.match(meta_digest) is None
    ):
        raise ValueError(
            "Runtime fingerprint carries no meta_digest integrity key: "
            f"{meta_digest!r}"
        )
    return meta_digest


def _runtime_wasm_integrity_sidecar_path(path: Path, integrity_key: str) -> Path:
    """Keyed integrity-pin slot ``<artifact>.<integrity_key>.sha256``."""
    if _RUNTIME_WASM_INTEGRITY_KEY_RE.match(integrity_key) is None:
        raise ValueError(f"Invalid runtime integrity key: {integrity_key!r}")
    return path.with_name(f"{path.name}.{integrity_key}.sha256")


def _runtime_wasm_integrity_pin_paths(path: Path) -> tuple[Path, ...]:
    """Every keyed integrity pin published for ``path``, sorted by name."""
    prefix = f"{path.name}."
    suffix = ".sha256"
    pins = [
        candidate
        for candidate in path.parent.glob(f"{path.name}.*{suffix}")
        if _RUNTIME_WASM_INTEGRITY_KEY_RE.match(
            candidate.name[len(prefix) : -len(suffix)]
        )
        is not None
    ]
    return tuple(sorted(pins))


def _runtime_wasm_has_matching_integrity_pin(path: Path) -> bool:
    """Return true when one keyed integrity pin matches ``path`` exactly."""
    try:
        digest = _sha256_file(path)
    except OSError:
        return False
    for sidecar in _runtime_wasm_integrity_pin_paths(path):
        try:
            raw = sidecar.read_text(encoding="utf-8").strip()
        except OSError:
            continue
        match = re.search(r"\b([0-9a-fA-F]{64})\b", raw)
        if match is not None and match.group(1).lower() == digest:
            return True
    return False


def _write_runtime_wasm_integrity_sidecar(path: Path, *, integrity_key: str) -> None:
    digest = _sha256_file(path)
    sidecar = _runtime_wasm_integrity_sidecar_path(path, integrity_key)
    _atomic_write_text(sidecar, f"{digest}\n")
    for stale_sidecar in path.parent.glob(f"{path.name}.*.sha256"):
        if stale_sidecar != sidecar:
            stale_sidecar.unlink(missing_ok=True)
    path.with_name(f"{path.name}.sha256").unlink(missing_ok=True)


def _validate_wasm_structural(path: Path) -> str | None:
    exe = shutil.which("wasm-tools")
    if exe is None:
        return (
            "wasm-tools is required for deep structural validation; "
            "artifact reuse is disabled until the validator is provisioned"
        )
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


def _reusable_wasm_artifact_validation_error(path: Path) -> str | None:
    state = inspect_wasm_binary(path)
    if state != "valid":
        return f"artifact is {state}"
    structural_error = _validate_wasm_structural(path)
    if structural_error is not None:
        return structural_error
    return None


def _runtime_wasm_artifact_validation_error(path: Path) -> str | None:
    artifact_error = _reusable_wasm_artifact_validation_error(path)
    if artifact_error is not None:
        return artifact_error
    if not has_nonempty_wasm_code_section(path):
        return "artifact has no non-empty code section"
    return None


def _shared_runtime_wasm_validation_error(path: Path) -> str | None:
    artifact_error = _runtime_wasm_artifact_validation_error(path)
    if artifact_error is not None:
        return artifact_error
    if not _runtime_wasm_has_shared_import_abi(path):
        return "artifact is missing the shared memory/table import ABI"
    return None


def _is_reusable_wasm_artifact(path: Path) -> bool:
    return _reusable_wasm_artifact_validation_error(path) is None


def _is_valid_runtime_wasm_artifact(path: Path) -> bool:
    return _runtime_wasm_artifact_validation_error(path) is None


def _runtime_wasm_has_shared_import_abi(path: Path) -> bool:
    try:
        memory_min, table_min = _wasm_import_minima(path)
    except (OSError, ValueError):
        return False
    return memory_min is not None and table_min is not None


def _is_valid_shared_runtime_wasm_artifact(path: Path) -> bool:
    return _shared_runtime_wasm_validation_error(path) is None


def _runtime_wasm_exports_satisfy(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    return not _runtime_wasm_missing_exports(path, required_exports)


def _split_runtime_wasm_exports_satisfy(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    return not _split_runtime_wasm_missing_exports(path, required_exports)


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


def _split_runtime_wasm_missing_exports(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> set[str]:
    export_names = _collect_wasm_export_names(path)
    if not export_names and required_exports:
        return wasm_split_runtime_missing_required_exports((), required_exports)
    return wasm_split_runtime_missing_required_exports(export_names, required_exports)
