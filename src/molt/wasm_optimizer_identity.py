"""Shared discovery, executable identity, and proof schema for ``wasm-opt``."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import shutil
from typing import Any, TypeGuard

from molt.binaryen_identity import (
    TREE_IDENTITY_SCHEMA,
    BinaryenIdentityError,
    binaryen_installation_identity,
    is_binaryen_version_output,
    load_binaryen_install_receipt,
    read_binaryen_version,
)
from molt.binaryen_toolchain import (
    BinaryenConfigError,
    BinaryenHostAsset,
    binaryen_host_asset,
)
from molt.exact_json import canonical_json_bytes, dumps_exact, loads_exact
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.wasm_optimization import WASM_OPT_LEVELS, wasm_opt_pipeline


WASM_OPTIMIZER_ATTESTATION_SCHEMA = "molt.wasm-optimizer-attestation.v4"
_ATTESTATION_KEYS = frozenset(
    {
        "schema",
        "status",
        "binaryen_version",
        "wasm_opt_sha256",
        "optimization_level",
        "optimization_converge",
        "optimization_apply_level",
        "optimization_preserve_debug",
        "optimization_extra_passes",
        "pipeline",
        "pipeline_authority_sha256",
        "optimizer_input_sha256",
        "optimizer_output_sha256",
        "published_output_sha256",
    }
)
_HEX = frozenset("0123456789abcdef")


class WasmOptimizerIdentityError(ValueError):
    """Raised when optimizer discovery or attestation is not exact."""


@dataclass(frozen=True, slots=True)
class WasmOptimizerExecutableIdentity:
    """One stable executable, content, and canonical Binaryen version identity."""

    executable: StableRegularFileIdentity
    binaryen_version: str

    @property
    def path(self) -> Path:
        return self.executable.path

    @property
    def sha256(self) -> str:
        return self.executable.sha256

    def matches_path(self, selected: str | Path) -> bool:
        """Return whether *selected* still has this exact filesystem identity."""

        try:
            path = Path(selected).expanduser().resolve(strict=True)
            if path != self.path:
                return False
            verify_stable_regular_file_identity(
                self.executable,
                label="wasm-opt executable",
            )
        except (OSError, ValueError):
            return False
        return True


def _is_sha256(value: object) -> TypeGuard[str]:
    return (
        isinstance(value, str)
        and len(value) == 64
        and not any(character not in _HEX for character in value)
    )


def _managed_binaryen_asset(path: Path) -> BinaryenHostAsset | None:
    """Return the exact receipted host asset when *path* is managed Binaryen."""

    from molt.dx import (
        DxConfigError,
        canonical_toolchain_root,
        selected_toolchain_contains,
    )

    try:
        source = compiler_source_root()
        if not selected_toolchain_contains(source, path):
            return None
        target_root = canonical_toolchain_root(source, require_exists=False)
        asset = binaryen_host_asset(source)
        installation = target_root / "toolchains" / asset.archive_root
        expected = installation.joinpath(
            *PurePosixPath(asset.executable).parts
        ).resolve(strict=False)
    except DxConfigError as exc:
        raise WasmOptimizerIdentityError(str(exc)) from exc
    except (BinaryenConfigError, OSError):
        return None
    if path != expected:
        return None
    try:
        receipt = load_binaryen_install_receipt(installation)
    except BinaryenIdentityError as exc:
        raise WasmOptimizerIdentityError(
            f"managed Binaryen receipt is invalid: {installation}: {exc}"
        ) from exc
    expected_tree = {
        "schema": TREE_IDENTITY_SCHEMA,
        "entries": asset.tree_entries,
        "total_bytes": asset.tree_total_bytes,
        "sha256": asset.tree_sha256,
    }
    if receipt["asset"] != asdict(asset) or receipt["tree"] != expected_tree:
        raise WasmOptimizerIdentityError(
            f"managed Binaryen receipt differs from its host manifest: {installation}"
        )
    return asset


def wasm_optimizer_executable_identity(
    selected: str | Path,
    *,
    verify_managed_installation: bool = True,
) -> WasmOptimizerExecutableIdentity:
    """Resolve one exact executable, optionally proving its managed tree."""

    try:
        path = Path(selected).expanduser().resolve(strict=True)
    except OSError as exc:
        raise WasmOptimizerIdentityError(
            f"wasm-opt cannot be resolved exactly: {selected}: {exc}"
        ) from exc
    try:
        executable = stable_regular_file_identity(
            path,
            label="wasm-opt executable",
        )
    except ValueError as exc:
        raise WasmOptimizerIdentityError(str(exc)) from exc
    managed = _managed_binaryen_asset(path)
    if managed is not None and executable.sha256 != managed.executable_sha256:
        raise WasmOptimizerIdentityError(
            f"managed wasm-opt executable differs from its manifest identity: {path}"
        )
    if managed is not None and verify_managed_installation:
        try:
            installation = binaryen_installation_identity(
                path.parent.parent,
                include_modes=managed.tree_includes_modes,
                executable=managed.executable,
            )
        except (BinaryenIdentityError, OSError) as exc:
            raise WasmOptimizerIdentityError(
                f"managed wasm-opt installation identity is invalid: {path}: {exc}"
            ) from exc
        expected_tree = {
            "schema": TREE_IDENTITY_SCHEMA,
            "entries": managed.tree_entries,
            "total_bytes": managed.tree_total_bytes,
            "sha256": managed.tree_sha256,
        }
        if (
            installation.tree.as_record() != expected_tree
            or installation.executable_sha256 != managed.executable_sha256
            or installation.executable_sha256 != executable.sha256
        ):
            raise WasmOptimizerIdentityError(
                f"managed wasm-opt differs from its manifest identity: {path}"
            )
    try:
        _, binaryen_version = read_binaryen_version(
            path, expected_sha256=executable.sha256
        )
    except (BinaryenIdentityError, OSError) as exc:
        raise WasmOptimizerIdentityError(
            f"wasm-opt version identity is invalid: {path}: {exc}"
        ) from exc
    try:
        verify_stable_regular_file_identity(
            executable,
            label="wasm-opt executable",
        )
    except ValueError as exc:
        raise WasmOptimizerIdentityError(
            f"wasm-opt changed while reading its version identity: {path}"
        ) from exc
    if managed is not None:
        expected_version = (
            f"wasm-opt version {managed.version} (version_{managed.version})"
        )
        if binaryen_version != expected_version:
            raise WasmOptimizerIdentityError(
                f"managed wasm-opt version differs from its manifest: {path}"
            )
    return WasmOptimizerExecutableIdentity(
        executable=executable,
        binaryen_version=binaryen_version,
    )


def find_wasm_opt() -> str | None:
    """Resolve ``wasm-opt`` through one fail-closed discovery authority.

    A non-empty ``MOLT_WASM_OPT`` is an explicit pin. If that pin is unusable,
    resolution stops instead of substituting an ambient executable from
    ``PATH`` or a managed-toolchain directory.
    """

    pinned = os.environ.get("MOLT_WASM_OPT", "").strip()
    if pinned:
        pinned_path = Path(pinned).expanduser()
        return str(pinned_path) if pinned_path.is_file() else None
    on_path = shutil.which("wasm-opt")
    if on_path is not None:
        return on_path
    from molt.dx import DxConfigError, canonical_toolchain_root

    try:
        source = compiler_source_root()
        toolchains = (
            canonical_toolchain_root(source, require_exists=False) / "toolchains"
        )
        asset = binaryen_host_asset(source)
    except (BinaryenConfigError, DxConfigError, OSError):
        return None
    candidate = toolchains / asset.archive_root
    executable = candidate.joinpath(*PurePosixPath(asset.executable).parts)
    if executable.is_file():
        try:
            resolved = executable.resolve(strict=True)
            if _managed_binaryen_asset(resolved) == asset:
                return str(executable)
        except (OSError, WasmOptimizerIdentityError):
            return None
    return None


def wasm_optimizer_invocation_identity() -> WasmOptimizerExecutableIdentity:
    """Resolve and validate the optimizer once for one linker invocation."""

    selected = find_wasm_opt()
    if selected is None:
        raise WasmOptimizerIdentityError(
            "wasm-opt is unavailable through the configured toolchain authority"
        )
    return wasm_optimizer_executable_identity(selected)


def wasm_optimizer_cache_fact() -> dict[str, str]:
    """Return the executable dependency that governs cached optimizer output.

    Managed discovery already binds the receipt to the host manifest. Cache
    reuse depends on the executable bytes and canonical version, not unrelated
    files in the Binaryen distribution, so this path deliberately avoids the
    full installation scan required immediately before execution.
    """

    selected = find_wasm_opt()
    if selected is None:
        raise WasmOptimizerIdentityError(
            "wasm-opt is unavailable through the configured toolchain authority"
        )
    identity = wasm_optimizer_executable_identity(
        selected,
        verify_managed_installation=False,
    )
    return {
        "tool": "wasm-opt",
        "sha256": identity.sha256,
        "binaryen_version": identity.binaryen_version,
    }


def wasm_optimizer_attestation_path(linked_output: Path) -> Path:
    """Return the single sidecar path for one linked WASM artifact."""

    return linked_output.with_name(f"{linked_output.name}.wasm-opt.json")


def _string_list(value: object, *, field: str, nonempty: bool) -> list[str]:
    if (
        not isinstance(value, Sequence)
        or isinstance(value, (str, bytes))
        or (nonempty and not value)
        or any(not isinstance(item, str) or not item for item in value)
    ):
        requirement = "a non-empty" if nonempty else "an"
        raise WasmOptimizerIdentityError(
            f"wasm optimizer {field} must be {requirement} exact string sequence"
        )
    return list(value)


def wasm_optimizer_pipeline_authority_sha256(
    *,
    level: str,
    converge: bool,
    apply_level: bool,
    preserve_debug: bool,
    extra_passes: Sequence[str],
    pipeline: Sequence[str],
) -> str:
    """Digest one exact, reproducible optimizer configuration and argv pipeline."""

    if (
        level not in WASM_OPT_LEVELS
        or type(converge) is not bool
        or type(apply_level) is not bool
        or type(preserve_debug) is not bool
    ):
        raise WasmOptimizerIdentityError("wasm optimizer configuration is invalid")
    normalized_extra_passes = list(
        dict.fromkeys(
            _string_list(
                extra_passes,
                field="extra passes",
                nonempty=False,
            )
        )
    )
    normalized_pipeline = _string_list(pipeline, field="pipeline", nonempty=True)
    expected_pipeline = list(
        wasm_opt_pipeline(
            level,
            extra_passes=normalized_extra_passes,
            converge=converge,
            apply_level=apply_level,
            preserve_debug=preserve_debug,
        )
    )
    if normalized_pipeline != expected_pipeline:
        raise WasmOptimizerIdentityError(
            "wasm optimizer pipeline differs from its canonical configuration"
        )
    encoded = canonical_json_bytes(
        {
            "apply_level": apply_level,
            "preserve_debug": preserve_debug,
            "converge": converge,
            "extra_passes": normalized_extra_passes,
            "level": level,
            "pipeline": normalized_pipeline,
        },
    )
    return hashlib.sha256(encoded).hexdigest()


def build_wasm_optimizer_attestation(
    raw: Mapping[str, object],
    *,
    published_output: bytes,
) -> dict[str, object]:
    """Project one invocation onto canonical reproducible optimizer provenance."""

    if raw.get("schema") == WASM_OPTIMIZER_ATTESTATION_SCHEMA:
        normalized = validate_wasm_optimizer_attestation(
            {key: raw.get(key) for key in _ATTESTATION_KEYS}
        )
        normalized["published_output_sha256"] = hashlib.sha256(
            published_output
        ).hexdigest()
        return validate_wasm_optimizer_attestation(normalized)
    if raw.get("ok") is not True:
        raise WasmOptimizerIdentityError(
            "cannot publish provenance for an unsuccessful wasm optimizer invocation"
        )
    level = raw.get("optimization_level")
    converge = raw.get("optimization_converge")
    apply_level = raw.get("optimization_apply_level")
    preserve_debug = raw.get("optimization_preserve_debug")
    extra_passes = list(
        dict.fromkeys(
            _string_list(
                raw.get("optimization_extra_passes"),
                field="extra passes",
                nonempty=False,
            )
        )
    )
    pipeline = _string_list(raw.get("pipeline"), field="pipeline", nonempty=True)
    if not isinstance(level, str):
        raise WasmOptimizerIdentityError("wasm optimizer level must be a string")
    if (
        type(converge) is not bool
        or type(apply_level) is not bool
        or type(preserve_debug) is not bool
    ):
        raise WasmOptimizerIdentityError(
            "wasm optimizer boolean configuration is invalid"
        )
    payload: dict[str, object] = {
        "schema": WASM_OPTIMIZER_ATTESTATION_SCHEMA,
        "status": "success",
        "binaryen_version": raw.get("binaryen_version"),
        "wasm_opt_sha256": raw.get("wasm_opt_sha256"),
        "optimization_level": level,
        "optimization_converge": converge,
        "optimization_apply_level": apply_level,
        "optimization_preserve_debug": preserve_debug,
        "optimization_extra_passes": extra_passes,
        "pipeline": pipeline,
        "pipeline_authority_sha256": wasm_optimizer_pipeline_authority_sha256(
            level=level,
            converge=converge,
            apply_level=apply_level,
            preserve_debug=preserve_debug,
            extra_passes=extra_passes,
            pipeline=pipeline,
        ),
        "optimizer_input_sha256": raw.get("optimizer_input_sha256"),
        "optimizer_output_sha256": raw.get("optimizer_output_sha256"),
        "published_output_sha256": hashlib.sha256(published_output).hexdigest(),
    }
    return validate_wasm_optimizer_attestation(payload)


def validate_wasm_optimizer_attestation(payload: object) -> dict[str, Any]:
    """Validate the exact durable optimizer proof schema."""

    if not isinstance(payload, dict) or set(payload) != _ATTESTATION_KEYS:
        raise WasmOptimizerIdentityError(
            "wasm optimizer attestation keys are not exact"
        )
    pipeline = payload.get("pipeline")
    extra_passes = payload.get("optimization_extra_passes")
    version = payload.get("binaryen_version")
    level = payload.get("optimization_level")
    converge = payload.get("optimization_converge")
    apply_level = payload.get("optimization_apply_level")
    preserve_debug = payload.get("optimization_preserve_debug")
    try:
        expected_pipeline_authority = wasm_optimizer_pipeline_authority_sha256(
            level=level if isinstance(level, str) else "",
            converge=converge if type(converge) is bool else False,
            apply_level=apply_level if type(apply_level) is bool else False,
            preserve_debug=preserve_debug if type(preserve_debug) is bool else False,
            extra_passes=(
                extra_passes
                if isinstance(extra_passes, Sequence)
                and not isinstance(extra_passes, (str, bytes))
                else ()
            ),
            pipeline=(
                pipeline
                if isinstance(pipeline, Sequence)
                and not isinstance(pipeline, (str, bytes))
                else ()
            ),
        )
    except WasmOptimizerIdentityError:
        expected_pipeline_authority = None
    valid = (
        payload.get("schema") == WASM_OPTIMIZER_ATTESTATION_SCHEMA
        and payload.get("status") == "success"
        and is_binaryen_version_output(version)
        and level in WASM_OPT_LEVELS
        and type(converge) is bool
        and type(apply_level) is bool
        and type(preserve_debug) is bool
        and isinstance(extra_passes, list)
        and all(isinstance(item, str) and item for item in extra_passes)
        and extra_passes == list(dict.fromkeys(extra_passes))
        and isinstance(pipeline, list)
        and bool(pipeline)
        and all(isinstance(item, str) and item for item in pipeline)
        and _is_sha256(payload.get("wasm_opt_sha256"))
        and _is_sha256(payload.get("pipeline_authority_sha256"))
        and payload.get("pipeline_authority_sha256") == expected_pipeline_authority
        and _is_sha256(payload.get("optimizer_input_sha256"))
        and _is_sha256(payload.get("optimizer_output_sha256"))
        and _is_sha256(payload.get("published_output_sha256"))
    )
    if not valid:
        raise WasmOptimizerIdentityError("wasm optimizer attestation is invalid")
    return dict(payload)


def load_wasm_optimizer_attestation(path: Path) -> dict[str, Any]:
    """Load one exact optimizer sidecar without accepting JSON extensions."""

    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise WasmOptimizerIdentityError(
            f"wasm optimizer attestation is unreadable: {path}: {exc}"
        ) from exc
    return validate_wasm_optimizer_attestation(payload)


def encode_wasm_optimizer_attestation(payload: object) -> str:
    """Encode one already-validated attestation deterministically."""

    validated = validate_wasm_optimizer_attestation(payload)
    return dumps_exact(validated, indent=None)
