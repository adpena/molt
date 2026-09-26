from __future__ import annotations

import contextlib
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Collection, TypedDict

from molt.capability_manifest import ResolvedRuntimePolicy
from molt import artifact_publication
from molt.wasm_bundle import BundleManifest, write_wasm_bundle
from molt.file_publication import staged_file_path
from molt.exact_json import canonical_json_bytes
from molt._wasm_abi_generated import (
    WASM_ESSENTIAL_EXPORTS,
    WASM_OUTPUT_RUNTIME_EXPORT_ALIASES,
)
from molt.cli import wasm_link_inputs
from molt.cli.app_export_contract import load_app_export_contract
from molt.browser_asset_closure import (
    BROWSER_WASM_ENTRY_ASSETS,
    browser_asset_manifest_keys,
    canonical_wasm_loader_asset_bytes,
    wasm_loader_asset_closure,
)
from molt.cli import link_fingerprints
from molt.cli.wasm_deployment import (
    WasmDeploymentGeneration,
    WasmDeploymentPlan,
    WasmDeploymentSources,
)
from molt.link_outputs import validate_link_output_paths, wasm_link_output_paths
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_bytes,
    _atomic_write_json,
    _atomic_write_text,
    _remove_file_or_tree,
)
from molt.cli.browser_target_features import (
    TARGET_FEATURE_MANIFEST_ASSET_NAME,
    browser_target_feature_metadata,
)
from molt.cli.command_runtime import _run_completed_command
from molt.cli.external_native import (
    _external_native_link_requirements,
    _stage_external_package_native_artifacts_for_build,
)
from molt.cli.models import (
    BuildProfile,
    _ExternalPackageNativeArtifactPlan,
    _PreparedNonNativeResult,
    _RuntimeArtifactState,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.output import (
    CliFailure as _CliFailure,
    fail as _fail,
    subprocess_output_text,
)
from molt.cli.python_source_closure import local_python_import_closure
from molt.cli.runtime_wasm_validation import (
    _validate_wasm_structural,
)
from molt.cli.wasm_host import resolve_molt_wasm_host_binary
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    merge_source_extension_link_requirements,
    source_extension_link_file,
)
from molt.cli.wasm import (
    WASM_WORKER_COMPATIBILITY_DATE,
    _effective_split_worker_table_base,
    _generate_split_worker_js,
    _generate_split_wrangler_jsonc,
    _runtime_export_name_for_import_from_manifest,
    _runtime_import_canonical_names_from_manifest,
    _runtime_import_export_names_from_manifest,
    _runtime_import_name_for_export_from_manifest,
    _runtime_import_result_kinds_from_manifest,
    _runtime_import_signatures_from_manifest,
    _split_runtime_browser_abi_from_manifest,
)
from molt.native_callable_abi import (
    NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
    native_callable_browser_signature,
)
from molt.toolchain_identity import stable_regular_file_identity
from molt.wasm_artifact import (
    _collect_wasm_module_import_names,
    _wasm_export_function_signatures,
    _wasm_import_minima,
    read_wasm_callable_table_attestation,
    wasm_callable_table_manifest_summary,
)


_BUNDLE_EXCLUDED_NATIVE_SUFFIXES = {
    ".a",
    ".dll",
    ".dylib",
    ".o",
    ".pyd",
    ".rlib",
    ".so",
    ".wasm",
}


def _file_asset(path: Path, asset_path: str) -> dict[str, object]:
    h = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            h.update(chunk)
    return {
        "path": asset_path,
        "size": path.stat().st_size,
        "sha256": h.hexdigest(),
    }


_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _precompile_receipt_artifacts(stdout: str) -> dict[str, dict[str, object]]:
    """Validate the sole receipt emitted by the Rust precompile authority."""
    try:
        receipt = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise ValueError(
            f"molt-wasm-host returned invalid precompile JSON: {exc}"
        ) from exc
    if not isinstance(receipt, dict):
        raise ValueError("molt-wasm-host precompile receipt must be an object")
    if (
        type(receipt.get("version")) is not int
        or receipt.get("version") != 1
        or receipt.get("kind") != "molt-wasm-precompile"
    ):
        raise ValueError("molt-wasm-host returned an unsupported precompile receipt")
    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, dict) or set(artifacts).difference(
        {"main", "runtime"}
    ):
        raise ValueError("molt-wasm-host precompile receipt has invalid artifacts")
    validated: dict[str, dict[str, object]] = {}
    for name in ("main", "runtime"):
        if name not in artifacts and name == "runtime":
            continue
        artifact = artifacts.get(name)
        if not isinstance(artifact, dict):
            raise ValueError(f"molt-wasm-host precompile receipt lacks {name} artifact")
        if set(artifact) != {"source", "path", "source_sha256", "sha256", "size"}:
            raise ValueError(
                f"molt-wasm-host precompile receipt has invalid {name} fields"
            )
        source, path = artifact.get("source"), artifact.get("path")
        size = artifact.get("size")
        source_sha256 = artifact.get("source_sha256")
        sha256 = artifact.get("sha256")
        if (
            not isinstance(source, str)
            or not source
            or not isinstance(path, str)
            or not path
            or isinstance(size, bool)
            or not isinstance(size, int)
            or size <= 0
            or not isinstance(source_sha256, str)
            or not _SHA256_RE.fullmatch(source_sha256)
            or not isinstance(sha256, str)
            or not _SHA256_RE.fullmatch(sha256)
        ):
            raise ValueError(
                f"molt-wasm-host precompile receipt has invalid {name} artifact"
            )
        validated[name] = artifact
    return validated


def _validate_precompile_receipt_outputs(
    artifacts: dict[str, dict[str, object]],
) -> None:
    """Fail before success if the host receipt does not name its published bytes."""
    for name, artifact in artifacts.items():
        path = Path(str(artifact["path"]))
        if not path.is_file():
            raise ValueError(f"molt-wasm-host did not publish {name} artifact: {path}")
        observed = stable_regular_file_identity(path, label=f"host precompiled {name}")
        if observed.size != artifact["size"] or observed.sha256 != artifact["sha256"]:
            raise ValueError(
                f"molt-wasm-host published corrupt {name} artifact: {path}"
            )


def _bytes_asset(payload: bytes, asset_path: str) -> dict[str, object]:
    return {
        "path": asset_path,
        "size": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def _app_export_manifest(
    contract: dict[str, object], artifact: Path
) -> dict[str, object]:
    raw_bindings = contract.get("bindings")
    if not isinstance(raw_bindings, list):
        raise ValueError("app export contract bindings are unavailable")
    export_symbols = {
        symbol
        for binding in raw_bindings
        if isinstance(binding, dict)
        and binding.get("disposition") == "export"
        and isinstance((symbol := binding.get("symbol")), str)
    }
    signatures = _wasm_export_function_signatures(
        artifact,
        export_names=export_symbols,
    )
    missing = sorted(export_symbols - signatures.keys())
    if missing:
        raise ValueError(
            "linked artifact is missing contract-declared app callable export(s): "
            + ", ".join(missing)
        )
    bindings: list[dict[str, object]] = []
    for raw_binding in raw_bindings:
        if not isinstance(raw_binding, dict):
            raise ValueError("app export contract binding is not an object")
        binding = dict(raw_binding)
        symbol = binding.get("symbol")
        if binding.get("disposition") == "export" and isinstance(symbol, str):
            binding["signature"] = signatures[symbol]
        bindings.append(binding)
    return {
        "schema": contract["schema"],
        "contract_digest": contract["contract_digest"],
        "registry_digest": contract["registry_digest"],
        "entry_module": contract["entry_module"],
        "call_abi": contract["call_abi"],
        "bindings": bindings,
    }


class _RuntimeImportAbiManifest(TypedDict):
    module: str
    names: list[str]
    canonical_names: dict[str, str]
    export_names: dict[str, str]
    signatures: dict[str, dict[str, object]]
    runtime_export_signatures: dict[str, dict[str, object]]
    result_kinds: dict[str, str]


def _external_static_bundle_arcname(root: Path, path: Path) -> str | None:
    if not path.is_file() or path.is_symlink():
        return None
    rel = path.relative_to(root)
    parts = rel.parts
    if "__pycache__" in parts or any(part in {"", ".", ".."} for part in parts):
        return None
    if path.suffix in _BUNDLE_EXCLUDED_NATIVE_SUFFIXES:
        return None
    if path.name.endswith((".pyc", ".pyo")):
        return None
    return rel.as_posix()


def _write_external_static_packages_bundle(
    runtime_roots: Collection[Path],
    output: Path,
) -> BundleManifest | None:
    roots = tuple(
        dict.fromkeys(
            root.resolve(strict=False)
            for root in runtime_roots
            if root.exists() and root.is_dir()
        )
    )
    if not roots:
        return None

    return write_wasm_bundle(
        roots,
        output,
        include=lambda root, path: (
            _external_static_bundle_arcname(root, path) is not None
        ),
        omit_empty=True,
    )


def _runtime_export_signatures_for_imports(
    runtime_wasm: Path, import_names: set[str]
) -> dict[str, dict[str, object]]:
    import_to_export = {}
    for import_name in import_names:
        export_name = _runtime_export_name_for_import_from_manifest(import_name)
        if export_name is not None:
            import_to_export[import_name] = export_name
    export_signatures = _wasm_export_function_signatures(
        runtime_wasm,
        export_names=import_to_export.values(),
    )
    return {
        import_name: export_signatures[export_name]
        for import_name, export_name in sorted(import_to_export.items())
        if export_name in export_signatures
    }


def _runtime_import_abi_manifest(
    runtime_module: Path, import_names: Collection[str]
) -> _RuntimeImportAbiManifest:
    names = set(import_names)
    runtime_export_signatures = _runtime_export_signatures_for_imports(
        runtime_module, names
    )
    return {
        "module": "molt_runtime",
        "names": sorted(names),
        "canonical_names": _runtime_import_canonical_names_from_manifest(names),
        "export_names": _runtime_import_export_names_from_manifest(names),
        "signatures": _runtime_import_signatures_from_manifest(
            names,
            runtime_export_signatures=runtime_export_signatures,
        ),
        "runtime_export_signatures": runtime_export_signatures,
        "result_kinds": _runtime_import_result_kinds_from_manifest(
            names,
            runtime_export_signatures=runtime_export_signatures,
        ),
    }


def _runtime_host_abi_import_names() -> set[str]:
    """Project generated host publication roots into runtime-import ABI keys."""

    return {
        import_name
        for export_name in WASM_ESSENTIAL_EXPORTS
        if (import_name := _runtime_import_name_for_export_from_manifest(export_name))
        is not None
    }


def _replace_directory_tree_from_source(
    src: Path,
    dst: Path,
    *,
    ignore: Any = None,
) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    backup_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.old")
    try:
        shutil.copytree(src, tmp_path, ignore=ignore)
        had_existing = dst.exists() or dst.is_symlink()
        if had_existing:
            os.replace(dst, backup_path)
        try:
            os.replace(tmp_path, dst)
        except BaseException:
            if had_existing and backup_path.exists() and not dst.exists():
                os.replace(backup_path, dst)
            raise
        if backup_path.exists():
            _remove_file_or_tree(backup_path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(dst.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                _remove_file_or_tree(tmp_path)
        with contextlib.suppress(OSError):
            if backup_path.exists():
                _remove_file_or_tree(backup_path)


def _snapshot_manifest_asset_digest(
    *, manifest_path: Path, modules: object, role: str
) -> str:
    if not isinstance(modules, dict):
        raise ValueError("snapshot execution manifest modules must be an object")
    descriptor = modules.get(role)
    if not isinstance(descriptor, dict):
        raise ValueError(f"snapshot execution manifest missing modules.{role}")
    path_value = descriptor.get("path")
    size_value = descriptor.get("size")
    digest_value = descriptor.get("sha256")
    if not isinstance(path_value, str) or not path_value:
        raise ValueError(f"snapshot execution manifest modules.{role}.path is invalid")
    descriptor_path = Path(path_value)
    if (
        descriptor_path.name != path_value
        or descriptor_path.drive
        or any(character in path_value for character in "\\:")
    ):
        raise ValueError(
            f"snapshot execution manifest modules.{role}.path must name an adjacent file"
        )
    if (
        not isinstance(size_value, int)
        or isinstance(size_value, bool)
        or size_value < 0
    ):
        raise ValueError(f"snapshot execution manifest modules.{role}.size is invalid")
    if (
        not isinstance(digest_value, str)
        or len(digest_value) != 64
        or any(character not in "0123456789abcdef" for character in digest_value)
    ):
        raise ValueError(
            f"snapshot execution manifest modules.{role}.sha256 is invalid"
        )
    module_path = manifest_path.parent / descriptor_path
    if not module_path.is_file():
        raise ValueError(
            f"snapshot execution manifest modules.{role}.path is not a file: {module_path}"
        )
    actual = _file_asset(module_path, path_value)
    if actual["size"] != size_value:
        raise ValueError(
            f"snapshot execution manifest modules.{role} size mismatch: "
            f"manifest={size_value} actual={actual['size']}"
        )
    if actual["sha256"] != digest_value:
        raise ValueError(
            f"snapshot execution manifest modules.{role} SHA-256 mismatch: "
            f"manifest={digest_value} actual={actual['sha256']}"
        )
    return f"sha256:{digest_value}"


def _snapshot_execution_identity(output_wasm: Path) -> str | None:
    manifest_path = output_wasm.parent / "manifest.json"
    if not manifest_path.exists():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"failed to read snapshot execution manifest: {exc}") from exc
    if not isinstance(manifest, dict) or manifest.get("version") != 2:
        raise ValueError("snapshot execution manifest must use version 2")
    mode = manifest.get("mode")
    modules = manifest.get("modules")
    if mode == "linked":
        linked = _snapshot_manifest_asset_digest(
            manifest_path=manifest_path,
            modules=modules,
            role="linked",
        )
        return f"molt.snapshot.execution.v2|linked|linked={linked}"
    if mode == "split-runtime":
        app = _snapshot_manifest_asset_digest(
            manifest_path=manifest_path,
            modules=modules,
            role="app",
        )
        runtime = _snapshot_manifest_asset_digest(
            manifest_path=manifest_path,
            modules=modules,
            role="runtime",
        )
        return f"molt.snapshot.execution.v2|split-runtime|app={app}|runtime={runtime}"
    raise ValueError(f"snapshot execution manifest has unsupported mode: {mode!r}")


def _generate_snapshot_header(
    *,
    output_wasm: Path,
    target_profile: str,
    resolved_capability_policy: ResolvedRuntimePolicy,
    verbose: bool,
) -> None:
    """Generate a non-restorable v2 snapshot metadata template."""
    snapshot_dir = output_wasm.parent
    snapshot_path = snapshot_dir / "molt.snapshot.json"

    execution_identity = _snapshot_execution_identity(output_wasm)

    mount_plan = [
        {
            "path": mount.path,
            "mount_type": mount.type,
            "max_size": mount.max_size,
            "source": mount.source,
        }
        for mount in resolved_capability_policy.mounts
    ]

    source_date_epoch_raw = os.environ.get("SOURCE_DATE_EPOCH", "315532800")
    try:
        source_date_epoch = int(source_date_epoch_raw)
        if source_date_epoch < 0:
            raise ValueError
        determinism_stamp = (
            dt.datetime.fromtimestamp(source_date_epoch, tz=dt.timezone.utc)
            .replace(microsecond=0)
            .isoformat()
            .replace("+00:00", "Z")
        )
    except (OverflowError, OSError, ValueError) as exc:
        raise ValueError(
            "SOURCE_DATE_EPOCH must be a non-negative representable integer"
        ) from exc

    header = {
        "snapshot_version": 2,
        "artifact_kind": "metadata-template",
        "restorable": False,
        "state_scope": None,
        "abi_version": "0.1.0",
        "target_profile": target_profile,
        "execution_identity": execution_identity,
        "mount_plan": mount_plan,
        "capability_manifest": list(resolved_capability_policy.grants.capabilities),
        "capability_policy": resolved_capability_policy.canonical_payload(),
        "capability_policy_digest": resolved_capability_policy.digest(),
        "determinism_stamp": determinism_stamp,
        "init_state_size": 0,
        "payload_hash": None,
        "integrity_hash": None,
    }

    _atomic_write_json(snapshot_path, header, indent=2)
    if verbose:
        print(
            f"Wrote non-restorable snapshot metadata template: {snapshot_path}",
            file=sys.stderr,
        )


def _browser_native_callable_manifest(
    native_artifact_plan: _ExternalPackageNativeArtifactPlan | None,
    *,
    required_symbols: Collection[str] = (),
) -> dict[str, Any]:
    required = frozenset(required_symbols)
    symbols: dict[str, dict[str, Any]] = {}
    if not required:
        return {"module": "molt_native", "symbols": symbols}
    if native_artifact_plan is None:
        missing = ", ".join(sorted(required))
        raise ValueError(
            "app imports native callable symbol(s) without staged native "
            f"artifact custody: {missing}"
        )

    def add_symbol(
        symbol: str,
        *,
        abi: str,
        export_payload: dict[str, Any],
    ) -> None:
        if symbol not in required:
            return
        existing = symbols.get(symbol)
        if existing is None:
            symbols[symbol] = {
                "abi": abi,
                "binding": "direct_symbol",
                "signature": native_callable_browser_signature(abi),
                "exports": [export_payload],
            }
            return
        if existing.get("abi") != abi:
            raise ValueError(
                f"native symbol {symbol!r} has conflicting callable "
                f"ABIs {existing.get('abi')!r} and {abi!r}"
            )
        existing_exports = existing.setdefault("exports", [])
        if not isinstance(existing_exports, list):
            raise ValueError(
                f"native symbol {symbol!r} manifest exports were corrupted"
            )
        existing_exports.append(export_payload)

    for artifact in native_artifact_plan.artifacts:
        artifact_payload = {
            "package": artifact.package,
            "module": artifact.module,
            "manifest_sha256": artifact.manifest_sha256,
            "extension_sha256": artifact.extension_sha256,
        }
        if artifact.init_symbol:
            add_symbol(
                artifact.init_symbol,
                abi=NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                export_payload={
                    "qualified_name": artifact.module,
                    "module": artifact.module,
                    "name": artifact.module.rsplit(".", 1)[-1],
                    "binding": "direct_symbol",
                    "abi": NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                    "symbol": artifact.init_symbol,
                    "kind": "extension_init",
                    "artifact": artifact_payload,
                },
            )
        for export in artifact.callable_exports:
            if export.binding != "direct_symbol":
                continue
            if not export.symbol:
                raise ValueError(
                    f"{export.qualified_name} direct_symbol export is missing symbol"
                )
            if export.symbol not in required:
                continue
            export_payload = export.digest_payload()
            export_payload["qualified_name"] = export.qualified_name
            export_payload["artifact"] = artifact_payload
            add_symbol(export.symbol, abi=export.abi, export_payload=export_payload)
    missing_required = sorted(required - symbols.keys())
    if missing_required:
        missing = ", ".join(missing_required)
        raise ValueError(
            "app imports native callable symbol(s) missing from staged native "
            f"artifact plan: {missing}"
        )
    for symbol in symbols:
        symbols[symbol]["exports"] = sorted(
            symbols[symbol]["exports"],
            key=lambda item: item["qualified_name"],
        )
    return {
        "module": "molt_native",
        "symbols": {symbol: symbols[symbol] for symbol in sorted(symbols)},
    }


def _staged_artifacts_need_wasm_libc_link(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> bool:
    return any(
        symbol.status == "external_link"
        and symbol.primitive_class == "wasm_libc_link_import"
        for artifact in artifacts
        for symbol in artifact.abi_symbols
    )


def _staged_artifacts_need_wasm_compiler_rt_link(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> bool:
    return any(
        symbol.status == "external_link"
        and symbol.primitive_class == "wasm_compiler_rt_link_import"
        for artifact in artifacts
        for symbol in artifact.abi_symbols
    )


def _staged_artifacts_need_wasm_libcxx_link(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> bool:
    return any(
        symbol.status == "external_link"
        and symbol.primitive_class == "wasm_libcxx_link_import"
        for artifact in artifacts
        for symbol in artifact.abi_symbols
    )


def _external_native_artifact_fingerprint_inputs(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> tuple[Path, ...]:
    return tuple(
        path
        for artifact in artifacts
        for path in (
            artifact.staged_path,
            artifact.staged_manifest_path,
            *artifact.staged_support_paths,
            *artifact.staged_link_input_paths,
        )
    )


def _prepare_non_native_build_result(
    *,
    is_rust_transpile: bool,
    is_luau_transpile: bool,
    is_wasm: bool,
    is_wasm_freestanding: bool = False,
    wasm_opt_enabled: bool = True,
    wasm_opt_level: str = "Oz",
    wasm_table_base: int | None = None,
    linked: bool,
    require_linked: bool,
    linked_output_path: Path | None,
    output_artifact: Path,
    json_output: bool,
    resolved_capability_policy: ResolvedRuntimePolicy,
    runtime_state: _RuntimeArtifactState,
    ensure_runtime_wasm_both: (
        Callable[[set[str] | frozenset[str] | None], bool] | None
    ) = None,
    runtime_cargo_profile: str,
    molt_root: Path,
    split_runtime: bool = False,
    precompile: bool = False,
    project_root: Path | None = None,
    profile: BuildProfile = "dev",
    warnings: list[str] | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan | None = None,
    artifacts_root: Path | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    phase_starts: dict[str, float] | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
) -> tuple[_PreparedNonNativeResult | None, _CliFailure | None]:
    with contextlib.ExitStack() as generation_custody:
        return _prepare_non_native_build_result_in_generation(
            generation_custody=generation_custody,
            is_rust_transpile=is_rust_transpile,
            is_luau_transpile=is_luau_transpile,
            is_wasm=is_wasm,
            is_wasm_freestanding=is_wasm_freestanding,
            wasm_opt_enabled=wasm_opt_enabled,
            wasm_opt_level=wasm_opt_level,
            wasm_table_base=wasm_table_base,
            linked=linked,
            require_linked=require_linked,
            linked_output_path=linked_output_path,
            output_artifact=output_artifact,
            json_output=json_output,
            resolved_capability_policy=resolved_capability_policy,
            runtime_state=runtime_state,
            ensure_runtime_wasm_both=ensure_runtime_wasm_both,
            runtime_cargo_profile=runtime_cargo_profile,
            molt_root=molt_root,
            split_runtime=split_runtime,
            precompile=precompile,
            project_root=project_root,
            profile=profile,
            warnings=warnings,
            native_artifact_plan=native_artifact_plan,
            artifacts_root=artifacts_root,
            stage_timings_ms=stage_timings_ms,
            phase_starts=phase_starts,
            wasm_facts_scanner=wasm_facts_scanner,
            app_export_contract_path=app_export_contract_path,
        )


def _prepare_non_native_build_result_in_generation(
    *,
    generation_custody: contextlib.ExitStack,
    is_rust_transpile: bool,
    is_luau_transpile: bool,
    is_wasm: bool,
    is_wasm_freestanding: bool = False,
    wasm_opt_enabled: bool = True,
    wasm_opt_level: str = "Oz",
    wasm_table_base: int | None = None,
    linked: bool,
    require_linked: bool,
    linked_output_path: Path | None,
    output_artifact: Path,
    json_output: bool,
    resolved_capability_policy: ResolvedRuntimePolicy,
    runtime_state: _RuntimeArtifactState,
    ensure_runtime_wasm_both: (
        Callable[[set[str] | frozenset[str] | None], bool] | None
    ) = None,
    runtime_cargo_profile: str,
    molt_root: Path,
    split_runtime: bool = False,
    precompile: bool = False,
    project_root: Path | None = None,
    profile: BuildProfile = "dev",
    warnings: list[str] | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan | None = None,
    artifacts_root: Path | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    phase_starts: dict[str, float] | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
) -> tuple[_PreparedNonNativeResult | None, _CliFailure | None]:
    if is_rust_transpile:
        return _PreparedNonNativeResult(
            primary_output=output_artifact,
            consumer_output=output_artifact,
            bundle_root=None,
            linked_output_path=linked_output_path,
            success_messages=[f"Successfully transpiled {output_artifact}"],
            extra_fields={},
            artifacts={"rust": str(output_artifact)},
        ), None
    if is_luau_transpile:
        return _PreparedNonNativeResult(
            primary_output=output_artifact,
            consumer_output=output_artifact,
            bundle_root=None,
            linked_output_path=linked_output_path,
            success_messages=[f"Successfully built {output_artifact}"],
            extra_fields={},
            artifacts={"luau": str(output_artifact)},
        ), None
    if is_wasm:
        output_wasm = output_artifact
        deployment: WasmDeploymentGeneration | None = None
        deployment_plan: WasmDeploymentPlan | None = None
        deployment_sources: WasmDeploymentSources | None = None
        link_skipped = False
        host_binary: str | None = None
        resolved_linked_output = linked_output_path
        bundle_root: Path | None = None
        artifacts: dict[str, str] = {"wasm": str(output_wasm)}
        # Browser execution has one canonical deploy shape: the sealed split
        # app/runtime bundle.  The old raw ``output.wasm`` lane had no manifest,
        # no owned-result adapters, and could not satisfy browser_host's strict
        # ABI/integrity contract.  Keep raw output only as a build intermediate.
        _split_runtime = split_runtime or os.environ.get("MOLT_SPLIT_RUNTIME") == "1"
        staged_runtime_wasm: Path | None = None
        runtime_wasm: Path | None = None
        runtime_reloc_wasm: Path | None = None
        runtime_wasm_generation: Path | None = None
        runtime_wasm_expected_identity: Path | None = None
        # The split-runtime lane reads the staged artifact set too, so the
        # empty defaults must exist on every path, not just `linked`.
        staged_external_native_artifacts: tuple[
            _StagedExternalPackageNativeArtifact, ...
        ] = ()
        external_native_fingerprint_inputs: tuple[Path, ...] = ()
        wasm_link_target = (
            "wasm32-unknown-unknown" if is_wasm_freestanding else "wasm32-wasip1"
        )
        wasm_link_requirements = SourceExtensionLinkRequirements(wasm_link_target)
        app_export_contract: dict[str, object] | None = None
        if linked or _split_runtime:
            if app_export_contract_path is None:
                return None, _fail(
                    "Linked WASM requires the frontend app export contract",
                    json_output,
                    command="build",
                )
            try:
                app_export_contract = load_app_export_contract(app_export_contract_path)
            except ValueError as exc:
                return None, _fail(
                    f"Invalid frontend app export contract: {exc}",
                    json_output,
                    command="build",
                )
            if native_artifact_plan is not None and native_artifact_plan.artifacts:
                try:
                    staged_external_native_artifacts = (
                        _stage_external_package_native_artifacts_for_build(
                            native_artifact_plan,
                            artifacts_root=artifacts_root or output_wasm.parent,
                        )
                    )
                    wasm_link_requirements = _external_native_link_requirements(
                        staged_external_native_artifacts,
                        target_triple=wasm_link_target,
                    )
                    provider_inputs: list[Path] = []
                    external_native_fingerprint_inputs = (
                        _external_native_artifact_fingerprint_inputs(
                            staged_external_native_artifacts
                        )
                    )
                    needs_wasm_libc_link = _staged_artifacts_need_wasm_libc_link(
                        staged_external_native_artifacts
                    )
                    needs_wasm_compiler_rt_link = (
                        _staged_artifacts_need_wasm_compiler_rt_link(
                            staged_external_native_artifacts
                        )
                    )
                    needs_wasm_libcxx_link = _staged_artifacts_need_wasm_libcxx_link(
                        staged_external_native_artifacts
                    )
                    if needs_wasm_libc_link:
                        libc_provider = wasm_link_inputs.wasm_wasi_libc_archive()
                        if libc_provider is None:
                            raise ValueError(
                                "wasm_libc_link_import symbols require Rust "
                                "wasm32-wasip1 self-contained libc.a"
                            )
                        libc_provider = libc_provider.resolve(strict=False)
                        provider_inputs.append(libc_provider)
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            libc_provider,
                        )
                    if needs_wasm_compiler_rt_link:
                        compiler_rt_provider = (
                            wasm_link_inputs.wasm_compiler_builtins_archive()
                        )
                        if compiler_rt_provider is None:
                            raise ValueError(
                                "wasm_compiler_rt_link_import symbols require Rust "
                                "wasm32-wasip1 libcompiler_builtins provider"
                            )
                        compiler_rt_provider = compiler_rt_provider.resolve(
                            strict=False
                        )
                        provider_inputs.append(compiler_rt_provider)
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            compiler_rt_provider,
                        )
                    if needs_wasm_libcxx_link:
                        cxx_runtime_providers = (
                            wasm_link_inputs.wasm_cxx_runtime_archives()
                        )
                        if cxx_runtime_providers is None:
                            raise ValueError(
                                "wasm_libcxx_link_import symbols require matching "
                                "WASI SDK eh/libc++.a, eh/libc++abi.a, and "
                                "eh/libunwind.a archives"
                            )
                        resolved_cxx_runtime_providers = tuple(
                            provider.resolve(strict=False)
                            for provider in cxx_runtime_providers
                        )
                        provider_inputs.extend(resolved_cxx_runtime_providers)
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            *resolved_cxx_runtime_providers,
                        )
                    wasm_link_requirements = merge_source_extension_link_requirements(
                        (
                            wasm_link_requirements,
                            SourceExtensionLinkRequirements(
                                wasm_link_target,
                                tuple(
                                    source_extension_link_file(path)
                                    for path in provider_inputs
                                ),
                            ),
                        ),
                        target_triple=wasm_link_target,
                    )
                except (OSError, ValueError) as exc:
                    return None, _fail(
                        f"Failed to stage external native artifacts for WASM link: {exc}",
                        json_output,
                        command="build",
                    )
                if staged_external_native_artifacts:
                    artifacts["external_static_packages_root"] = str(
                        staged_external_native_artifacts[0].runtime_root
                    )
                    for index, artifact in enumerate(staged_external_native_artifacts):
                        artifacts[f"external_native_artifact_{index}"] = str(
                            artifact.staged_path
                        )
                        artifacts[f"external_native_artifact_{index}_manifest"] = str(
                            artifact.staged_manifest_path
                        )
            required_runtime_exports = _collect_wasm_module_import_names(
                output_wasm, "molt_runtime"
            )
            if native_artifact_plan is not None:
                required_runtime_exports.update(
                    native_artifact_plan.runtime_export_symbols()
                )
            structural_error = _validate_wasm_structural(output_wasm)
            if structural_error is not None:
                return None, _fail(
                    "Generated wasm module failed structural validation before linking: "
                    + structural_error,
                    json_output,
                    command="build",
                )
            # Runtime bytes are admitted only as one shared+reloc generation.
            if ensure_runtime_wasm_both is None or not ensure_runtime_wasm_both(
                required_runtime_exports
            ):
                return None, _fail(
                    "Atomic runtime WASM pair build failed",
                    json_output,
                    command="build",
                )
            runtime_wasm = runtime_state.runtime_wasm_selected
            runtime_reloc_wasm = runtime_state.runtime_reloc_wasm_selected
            runtime_wasm_generation = runtime_state.runtime_wasm_generation
            runtime_wasm_expected_identity = (
                runtime_state.runtime_wasm_expected_identity
            )
            if runtime_reloc_wasm is None or not runtime_reloc_wasm.is_file():
                return None, _fail(
                    "Runtime WASM generation has no selected reloc member",
                    json_output,
                    command="build",
                )
            if resolved_linked_output is None:
                resolved_linked_output = output_wasm.with_name("output_linked.wasm")
            try:
                link_outputs = wasm_link_output_paths(
                    resolved_linked_output,
                    split_output_dir=output_wasm.parent if _split_runtime else None,
                    inputs=(
                        output_wasm,
                        runtime_reloc_wasm,
                        *((runtime_wasm,) if runtime_wasm is not None else ()),
                        *external_native_fingerprint_inputs,
                        *(
                            path
                            for artifact in staged_external_native_artifacts
                            for path in (
                                artifact.source_path,
                                artifact.source_manifest_path,
                            )
                        ),
                        app_export_contract_path,
                    ),
                )
            except (OSError, ValueError) as exc:
                return None, _fail(str(exc), json_output, command="build")
            if resolved_linked_output.parent != Path("."):
                resolved_linked_output.parent.mkdir(parents=True, exist_ok=True)
            if not is_wasm_freestanding:
                if runtime_wasm is None or not runtime_wasm.is_file():
                    return None, _fail(
                        "Runtime WASM generation has no selected shared member",
                        json_output,
                        command="build",
                    )
            tool = molt_root / "tools" / "wasm_link.py"
            if (
                runtime_wasm is None
                or runtime_wasm_generation is None
                or not runtime_wasm_generation.is_file()
                or runtime_wasm_expected_identity is None
                or not runtime_wasm_expected_identity.is_file()
            ):
                return None, _fail(
                    "Runtime WASM pair has no trusted expected identity",
                    json_output,
                    command="build",
                )
            link_cmd = [
                sys.executable,
                str(tool),
                "--runtime",
                str(runtime_reloc_wasm),
                "--runtime-shared",
                str(runtime_wasm),
                "--runtime-generation",
                str(runtime_wasm_generation),
                "--runtime-expected-identity",
                str(runtime_wasm_expected_identity),
                "--input",
                str(output_wasm),
                "--output",
                str(resolved_linked_output),
                "--wasm-facts-scanner",
                str(wasm_facts_scanner),
            ]
            link_cmd.extend(["--app-export-contract", str(app_export_contract_path)])
            native_link_plan_bytes = canonical_json_bytes(
                {
                    "link_requirements": wasm_link_requirements.manifest_payload(),
                }
            )
            if _split_runtime:
                if runtime_wasm is not None:
                    link_cmd.extend(["--deploy-runtime", str(runtime_wasm)])
                split_dir = output_wasm.parent
                link_cmd.extend(
                    [
                        "--split-runtime",
                        "--split-output-dir",
                        str(split_dir),
                    ]
                )
            if is_wasm_freestanding:
                link_cmd.append("--freestanding")
            if wasm_opt_enabled:
                link_cmd.extend(["--optimize", "--optimize-level", wasm_opt_level])
            if profile == "dev":
                link_cmd.append("--preserve-debug-sections")
            link_project_root = project_root or molt_root
            try:
                link_tool_closure = local_python_import_closure(molt_root, (tool,))
                deploy_asset_root = molt_root / "wasm"
                browser_asset_names = (
                    wasm_loader_asset_closure(
                        deploy_asset_root, BROWSER_WASM_ENTRY_ASSETS
                    )
                    if _split_runtime
                    else ()
                )
                browser_deploy_sources = (
                    (
                        deploy_asset_root / "browser_asset_graph.generated.json",
                        deploy_asset_root / TARGET_FEATURE_MANIFEST_ASSET_NAME,
                        *(
                            deploy_asset_root.joinpath(*Path(asset).parts)
                            for asset in browser_asset_names
                        ),
                    )
                    if _split_runtime
                    else ()
                )
                package_roots = tuple(
                    dict.fromkeys(
                        artifact.runtime_root.resolve()
                        for artifact in staged_external_native_artifacts
                    )
                )
                with artifact_publication.publication_payload_snapshot(
                    package_roots,
                    include=lambda root, path: (
                        _external_static_bundle_arcname(root, path) is not None
                    ),
                ) as package_snapshot:
                    package_payload_inputs = tuple(
                        path
                        for root, paths in package_snapshot.items()
                        for path in paths
                        if _external_static_bundle_arcname(root, path) is not None
                    )
                    deployment_sources = WasmDeploymentSources.capture(
                        (*package_payload_inputs, *browser_deploy_sources),
                        package_snapshot,
                        lambda root, path: (
                            _external_static_bundle_arcname(root, path) is not None
                        ),
                    )
                deployment_plan = WasmDeploymentPlan.create(
                    link_outputs,
                    output_root=output_wasm.parent,
                    split=_split_runtime,
                    loader_assets=tuple(browser_asset_names),
                    target_feature_asset=TARGET_FEATURE_MANIFEST_ASSET_NAME,
                    bundle=bool(package_payload_inputs),
                    precompile=precompile,
                )
                link_fingerprint_path = link_fingerprints._link_fingerprint_path(
                    deployment_plan.outputs["manifest"]
                )
                stored_link_fingerprint = link_fingerprints._read_link_fingerprint(
                    link_fingerprint_path
                )
                validate_link_output_paths(
                    deployment_plan.outputs,
                    inputs=(
                        output_wasm,
                        runtime_reloc_wasm,
                        runtime_wasm,
                        app_export_contract_path,
                        *external_native_fingerprint_inputs,
                        *package_payload_inputs,
                    ),
                )
                if precompile:
                    host_binary = resolve_molt_wasm_host_binary(
                        molt_root,
                        cargo_profile=runtime_cargo_profile,
                    )
                    if host_binary is None:
                        raise ValueError(
                            "--precompile requires a matching molt-wasm-host binary "
                            "(set MOLT_WASM_HOST_BIN or build the runtime profile)"
                        )
                deployment_closure = local_python_import_closure(
                    molt_root, (Path(__file__),)
                )
            except (OSError, ValueError) as exc:
                return None, _fail(
                    f"Failed to derive WASM linker fingerprint closure: {exc}",
                    json_output,
                    command="build",
                )
            link_fingerprint = link_fingerprints._link_fingerprint(
                project_root=link_project_root,
                inputs=[
                    output_wasm,
                    runtime_reloc_wasm,
                    *(
                        (runtime_wasm,)
                        if _split_runtime and runtime_wasm is not None
                        else ()
                    ),
                    *browser_deploy_sources,
                    *external_native_fingerprint_inputs,
                    *package_payload_inputs,
                    *((Path(host_binary),) if host_binary is not None else ()),
                    app_export_contract_path,
                ],
                link_cmd=link_cmd,
                tool_facts=(
                    {
                        "role": "wasm-link-source-closure",
                        "content_digest": link_tool_closure.content_digest,
                    },
                    {
                        "role": "wasm-native-link-plan",
                        "content_digest": hashlib.sha256(
                            native_link_plan_bytes
                        ).hexdigest(),
                    },
                    {
                        "role": "wasm-deployment",
                        "source_digest": deployment_closure.content_digest,
                        "capability_policy": resolved_capability_policy.canonical_payload(),
                        "wasm_table_base": wasm_table_base,
                        "precompile": precompile,
                        "host_environment": {
                            name: value
                            for name, value in sorted(os.environ.items())
                            if name.startswith("MOLT_WASM_")
                            or name == "MOLT_DETERMINISTIC"
                        }
                        if precompile
                        else None,
                        "outputs": {
                            role: str(path.resolve())
                            for role, path in deployment_plan.outputs.items()
                        },
                        "compatibility_date": WASM_WORKER_COMPATIBILITY_DATE
                        if _split_runtime
                        else None,
                    },
                ),
                stored_fingerprint=(
                    stored_link_fingerprint["fingerprint"]
                    if stored_link_fingerprint
                    else None
                ),
            )
            if link_fingerprint is None:
                return None, _fail(
                    "Unable to fingerprint the complete WASM deployment inputs",
                    json_output,
                    command="build",
                )
            try:
                assert deployment_sources is not None
                deployment_sources.verify_files()
            except (OSError, ValueError) as exc:
                return None, _fail(str(exc), json_output, command="build")
            link_skipped = link_fingerprints._link_outputs_match(
                outputs=deployment_plan.outputs,
                fingerprint=link_fingerprint,
                receipt_path=link_fingerprint_path,
            )
            if link_skipped:
                link_process = subprocess.CompletedProcess(link_cmd, 0, "", "")
                artifacts.update(deployment_plan.artifacts())
                if _split_runtime:
                    bundle_root = deployment_plan.root
            else:
                native_link_plan_path: Path | None = None
                link_timings_path: Path | None = None
                link_run_cmd = list(link_cmd)
                # The link is its own top-level build phase: without this
                # marker its whole wall time (wasm-ld, post-link passes,
                # wasm-opt, split-runtime processing) was charged to the last
                # phase started before it, backend_cache_write.
                if phase_starts is not None and "wasm_link" not in phase_starts:
                    phase_starts["wasm_link"] = time.perf_counter()
                try:
                    deployment = generation_custody.enter_context(
                        WasmDeploymentGeneration.prepare(deployment_plan)
                    )
                    link_run_cmd[link_run_cmd.index("--output") + 1] = str(
                        deployment.outputs["linked"]
                    )
                    if _split_runtime:
                        link_run_cmd[link_run_cmd.index("--split-output-dir") + 1] = (
                            str(deployment.root)
                        )
                    native_link_plan_path = staged_file_path(
                        deployment.outputs["linked"], purpose="native-link-plan"
                    )
                    native_link_plan_path.write_bytes(native_link_plan_bytes)
                    link_run_cmd.extend(
                        ["--native-link-plan", str(native_link_plan_path)]
                    )
                    if stage_timings_ms is not None:
                        link_timings_path = staged_file_path(
                            deployment.outputs["linked"], purpose="link-timings"
                        )
                        link_run_cmd.extend(
                            ["--phase-timings-file", str(link_timings_path)]
                        )
                    # The standalone tool retains its direct publisher, but its
                    # destinations here are private until deployment is complete.
                    link_process = _run_completed_command(
                        link_run_cmd,
                        cwd=molt_root,
                        env=None,
                        capture_output=True,
                        memory_guard_prefix="MOLT_WASM_LINK",
                    )
                    if link_process.returncode != 0:
                        err = link_process.stderr.strip() or link_process.stdout.strip()
                        msg = "Wasm link failed"
                        if err:
                            msg = f"{msg}: {err}"
                        return None, _fail(msg, json_output, command="build")
                    resolved_linked_output = deployment.outputs["linked"]
                except (OSError, ValueError) as exc:
                    return None, _fail(
                        f"Failed to prepare WASM link publication: {exc}",
                        json_output,
                        command="build",
                    )
                finally:
                    if stage_timings_ms is not None and link_timings_path is not None:
                        try:
                            link_timings = json.loads(
                                link_timings_path.read_text(encoding="utf-8")
                            )
                        except (OSError, ValueError, TypeError):
                            link_timings = {}
                        for name, value in link_timings.items():
                            if isinstance(name, str) and isinstance(
                                value, (int, float)
                            ):
                                stage_timings_ms[name] = round(
                                    max(0.0, float(value)), 6
                                )
                    for transport in (
                        native_link_plan_path,
                        link_timings_path,
                    ):
                        if transport is not None:
                            with contextlib.suppress(OSError):
                                transport.unlink()
                if phase_starts is not None and "wasm_publish" not in phase_starts:
                    phase_starts["wasm_publish"] = time.perf_counter()
        if not is_wasm_freestanding and not _split_runtime and not linked:
            required_runtime_exports = _collect_wasm_module_import_names(
                output_wasm, "molt_runtime"
            )
            if native_artifact_plan is not None:
                required_runtime_exports.update(
                    native_artifact_plan.runtime_export_symbols()
                )
            if ensure_runtime_wasm_both is None or not ensure_runtime_wasm_both(
                required_runtime_exports
            ):
                return None, _fail(
                    "Atomic runtime WASM pair build failed",
                    json_output,
                    command="build",
                )
            runtime_wasm = runtime_state.runtime_wasm_selected
            if runtime_wasm is None or not runtime_wasm.is_file():
                return None, _fail(
                    "Runtime WASM generation has no selected shared member",
                    json_output,
                    command="build",
                )
            staged_runtime_wasm = output_wasm.with_name("molt_runtime.wasm")
            if staged_runtime_wasm != runtime_wasm:
                try:
                    _atomic_copy_file(runtime_wasm, staged_runtime_wasm)
                except OSError as exc:
                    return None, _fail(
                        f"Failed to stage runtime wasm: {exc}",
                        json_output,
                        command="build",
                    )
            artifacts["runtime_wasm"] = str(staged_runtime_wasm)
        if resolved_linked_output is not None:
            artifacts["linked_wasm"] = str(resolved_linked_output)
        cwasm_path: str | None = artifacts.get("cwasm")
        runtime_cwasm_path: str | None = artifacts.get("runtime_cwasm")
        primary_output = output_wasm
        if require_linked and resolved_linked_output is not None:
            primary_output = resolved_linked_output
        consumer_output = resolved_linked_output or primary_output
        success_messages = (
            [f"Successfully built {primary_output}"]
            if require_linked
            else [f"Successfully built {output_wasm}"]
        )
        if resolved_linked_output is not None and not require_linked:
            success_messages.append(f"Successfully linked {resolved_linked_output}")
        if (
            linked
            and not _split_runtime
            and resolved_linked_output is not None
            and not link_skipped
        ):
            assert app_export_contract is not None
            try:
                app_exports_manifest = _app_export_manifest(
                    app_export_contract,
                    resolved_linked_output,
                )
            except ValueError as exc:
                return None, _fail(
                    f"Linked WASM app export manifest is invalid: {exc}",
                    json_output,
                    command="build",
                )
            linked_export_signatures = _wasm_export_function_signatures(
                resolved_linked_output,
                export_name_prefix="molt_",
            )
            linked_runtime_import_names = {
                import_name
                for export_name in linked_export_signatures
                if (
                    import_name := _runtime_import_name_for_export_from_manifest(
                        export_name
                    )
                )
                is not None
            }
            linked_env_import_names = _collect_wasm_module_import_names(
                resolved_linked_output,
                "env",
            )
            linked_self_imports = linked_env_import_names.intersection(
                linked_export_signatures
            )
            unexpected_linked_self_imports = linked_self_imports.difference(
                WASM_OUTPUT_RUNTIME_EXPORT_ALIASES
            )
            if unexpected_linked_self_imports:
                return None, _fail(
                    "Linked WASM has self-imports outside generated output export "
                    f"authority: {sorted(unexpected_linked_self_imports)!r}",
                    json_output,
                    command="build",
                )
            assert deployment is not None
            linked_manifest = deployment.outputs["manifest"]
            _atomic_write_json(
                linked_manifest,
                {
                    "version": 2,
                    "mode": "linked",
                    "abi": {
                        "runtime_imports": _runtime_import_abi_manifest(
                            resolved_linked_output,
                            linked_runtime_import_names,
                        ),
                        "linked_self_imports": sorted(linked_self_imports),
                        "app_exports": app_exports_manifest,
                    },
                    "modules": {
                        "linked": _file_asset(
                            resolved_linked_output,
                            resolved_linked_output.name,
                        )
                    },
                    "entry": {"module": "linked", "function": "molt_main"},
                },
                indent=2,
            )
            artifacts["manifest"] = str(linked_manifest)

        # --split-runtime: wasm_link.py produces app.wasm + molt_runtime.wasm;
        # generate manifest.json and worker.js shim here.
        if _split_runtime and runtime_reloc_wasm is not None and not link_skipped:
            assert app_export_contract is not None
            assert deployment is not None
            split_dir = deployment.root

            app_wasm = split_dir / "app.wasm"
            rt_wasm = split_dir / "molt_runtime.wasm"
            manifest = split_dir / "manifest.json"
            wasm_asset_root = molt_root / "wasm"
            target_feature_manifest_src = (
                wasm_asset_root / TARGET_FEATURE_MANIFEST_ASSET_NAME
            )
            try:
                browser_asset_names = wasm_loader_asset_closure(
                    wasm_asset_root,
                    BROWSER_WASM_ENTRY_ASSETS,
                )
                browser_asset_keys = browser_asset_manifest_keys(browser_asset_names)
                browser_asset_payloads = {
                    name: canonical_wasm_loader_asset_bytes(
                        wasm_asset_root.joinpath(*Path(name).parts)
                    )
                    for name in browser_asset_names
                }
                browser_assets = {
                    browser_asset_keys[name]: _bytes_asset(payload, name)
                    for name, payload in browser_asset_payloads.items()
                }
                target_feature_manifest_asset = _file_asset(
                    target_feature_manifest_src,
                    TARGET_FEATURE_MANIFEST_ASSET_NAME,
                )
            except (OSError, ValueError) as exc:
                return None, _fail(
                    f"Invalid split-runtime browser asset closure: {exc}",
                    json_output,
                    command="build",
                )

            if not app_wasm.exists() or not rt_wasm.exists():
                return None, _fail(
                    "Split-runtime link did not produce expected artifacts "
                    f"(app.wasm={app_wasm.exists()}, molt_runtime.wasm={rt_wasm.exists()})",
                    json_output,
                    command="build",
                )
            try:
                app_exports_manifest = _app_export_manifest(
                    app_export_contract,
                    app_wasm,
                )
            except ValueError as exc:
                return None, _fail(
                    f"Split-runtime app export manifest is invalid: {exc}",
                    json_output,
                    command="build",
                )

            app_size = app_wasm.stat().st_size
            rt_size = rt_wasm.stat().st_size
            app_memory_min, app_table_min = _wasm_import_minima(app_wasm)
            rt_memory_min, rt_table_min = _wasm_import_minima(rt_wasm)
            app_runtime_import_names = _collect_wasm_module_import_names(
                app_wasm, "molt_runtime"
            )
            runtime_abi_names = (
                set(app_runtime_import_names) | _runtime_host_abi_import_names()
            )
            app_native_callable_import_names = _collect_wasm_module_import_names(
                app_wasm, "molt_native"
            )
            app_env_import_names = _collect_wasm_module_import_names(app_wasm, "env")
            runtime_env_import_names = _collect_wasm_module_import_names(rt_wasm, "env")
            browser_host_import_names = app_env_import_names | runtime_env_import_names
            runtime_import_abi = _runtime_import_abi_manifest(
                rt_wasm,
                runtime_abi_names,
            )
            app_runtime_export_signatures = runtime_import_abi[
                "runtime_export_signatures"
            ]
            shared_memory_initial_pages = max(
                app_memory_min or 0,
                rt_memory_min or 0,
            )
            shared_table_initial = max(
                app_table_min or 0,
                rt_table_min or 0,
                8192,
            )
            try:
                app_callable_table = read_wasm_callable_table_attestation(app_wasm)
                runtime_callable_table = read_wasm_callable_table_attestation(rt_wasm)
                app_callable_table_summary = wasm_callable_table_manifest_summary(
                    app_callable_table
                )
                runtime_callable_table_summary = wasm_callable_table_manifest_summary(
                    runtime_callable_table
                )
            except (OSError, ValueError) as exc:
                return None, _fail(
                    f"Split-runtime callable-table attestation invalid: {exc}",
                    json_output,
                    command="build",
                )
            try:
                effective_wasm_table_base = _effective_split_worker_table_base(
                    wasm_table_base=wasm_table_base,
                    app_callable_table_slots=(
                        entry.slot for entry in app_callable_table
                    ),
                )
            except ValueError as exc:
                return None, _fail(
                    f"Split-runtime wasm_table_base metadata mismatch: {exc}",
                    json_output,
                    command="build",
                )

            try:
                native_callables_manifest = _browser_native_callable_manifest(
                    native_artifact_plan,
                    required_symbols=app_native_callable_import_names,
                )
            except ValueError as exc:
                return None, _fail(
                    f"Split-runtime native callable manifest invalid: {exc}",
                    json_output,
                    command="build",
                )
            browser_embed_abi = _split_runtime_browser_abi_from_manifest()
            browser_embed_abi["native_callables"] = native_callables_manifest
            bundle_manifest: BundleManifest | None = None
            bundle_tar = split_dir / "bundle.tar"
            with contextlib.suppress(FileNotFoundError):
                bundle_tar.unlink()
            if staged_external_native_artifacts:
                try:
                    bundle_manifest = _write_external_static_packages_bundle(
                        tuple(
                            artifact.runtime_root
                            for artifact in staged_external_native_artifacts
                        ),
                        bundle_tar,
                    )
                except (OSError, ValueError) as exc:
                    return None, _fail(
                        f"Failed to stage split-runtime external package bundle: {exc}",
                        json_output,
                        command="build",
                    )

            assets: dict[str, dict[str, object]] = {
                **browser_assets,
                "target_feature_manifest": target_feature_manifest_asset,
            }
            target_features = browser_target_feature_metadata(
                browser_host_import_names=browser_host_import_names,
                manifest_asset=target_feature_manifest_asset,
            )
            manifest_data: dict[str, Any] = {
                "version": 2,
                "mode": "split-runtime",
                "tree_shaken": True,
                "shared_memory_initial_pages": shared_memory_initial_pages,
                "shared_table_initial": shared_table_initial,
                "wasm_table_base": effective_wasm_table_base,
                "abi": {
                    "runtime_imports": runtime_import_abi,
                    "browser_embed": browser_embed_abi,
                    "callable_table": {
                        "app": app_callable_table_summary,
                        "runtime": runtime_callable_table_summary,
                    },
                    "app_exports": app_exports_manifest,
                },
                "modules": {
                    "runtime": _file_asset(rt_wasm, "molt_runtime.wasm"),
                    "app": _file_asset(app_wasm, "app.wasm"),
                },
                "assets": assets,
                "target_features": target_features,
                "total_size": app_size + rt_size,
                "instantiation_order": ["runtime", "app"],
                "entry": {"module": "app", "function": "molt_main"},
                "capability_policy": resolved_capability_policy.canonical_payload(),
                "capability_policy_digest": resolved_capability_policy.digest(),
            }
            if bundle_manifest is not None:
                bundle_size = bundle_tar.stat().st_size
                assets["bundle"] = {
                    **_file_asset(bundle_tar, "bundle.tar"),
                    "file_count": len(bundle_manifest["files"]),
                    "source_total_bytes": bundle_manifest["total_bytes"],
                }
                manifest_data["total_size"] = app_size + rt_size + bundle_size
            _atomic_write_json(manifest, manifest_data, indent=2)

            # Generate split-runtime Cloudflare Workers shim with full
            # WASI support and multi-module instantiation.
            worker_js = split_dir / "worker.js"
            _atomic_write_text(
                worker_js,
                _generate_split_worker_js(
                    resolved_capability_policy=resolved_capability_policy,
                    shared_memory_initial_pages=shared_memory_initial_pages,
                    shared_table_initial=shared_table_initial,
                    shared_table_base=effective_wasm_table_base,
                    runtime_import_names=runtime_abi_names,
                    runtime_export_signatures=app_runtime_export_signatures,
                ),
            )
            try:
                for name, payload in browser_asset_payloads.items():
                    _atomic_write_bytes(split_dir.joinpath(*Path(name).parts), payload)
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime browser asset closure: {exc}",
                    json_output,
                    command="build",
                )
            target_feature_manifest_dst = split_dir / TARGET_FEATURE_MANIFEST_ASSET_NAME
            try:
                _atomic_copy_file(
                    target_feature_manifest_src,
                    target_feature_manifest_dst,
                )
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime target feature manifest: {exc}",
                    json_output,
                    command="build",
                )

            # Generate wrangler.jsonc for Cloudflare Workers deployment.
            # JSONC is the modern Wrangler config shape and matches the
            # live-verification tooling contract.
            wrangler_jsonc = split_dir / "wrangler.jsonc"
            _atomic_write_text(
                wrangler_jsonc,
                _generate_split_wrangler_jsonc(
                    WASM_WORKER_COMPATIBILITY_DATE,
                    browser_asset_names,
                ),
            )
            legacy_wrangler_toml = split_dir / "wrangler.toml"
            if legacy_wrangler_toml.exists():
                legacy_wrangler_toml.unlink()
            bundle_root = split_dir
            artifacts.update(
                {
                    "app_wasm": str(app_wasm),
                    "runtime_wasm": str(rt_wasm),
                    "manifest": str(manifest),
                    "worker_js": str(worker_js),
                    "wrangler_config": str(wrangler_jsonc),
                }
            )
            if bundle_manifest is not None:
                artifacts["bundle_tar"] = str(bundle_tar)

            # Cloudflare Workers isolate memory limit: 128MB.
            # Warn if the combined WASM size exceeds a safe threshold.
            combined_mb = (app_size + rt_size) / (1024 * 1024)
            if combined_mb > 100:
                success_messages.append(
                    f"WARNING: Combined WASM size ({combined_mb:.1f}MB) approaches "
                    f"Cloudflare Workers 128MB isolate memory limit. "
                    f"Consider enabling --stdlib-profile micro for smaller builds."
                )
            success_messages.append(
                f"Split runtime: {app_wasm.name} ({app_size // 1024}KB) "
                f"+ {rt_wasm.name} ({rt_size // 1024}KB)"
            )

        if precompile and not link_skipped:
            manifest_value = artifacts.get("manifest")
            if not isinstance(manifest_value, str):
                return None, _fail(
                    "--precompile requires linked or split-runtime output with a canonical manifest",
                    json_output,
                    command="build",
                )
            if host_binary is None:
                return None, _fail(
                    "--precompile requires a matching molt-wasm-host binary "
                    "(set MOLT_WASM_HOST_BIN or build the runtime Cargo profile)",
                    json_output,
                    command="build",
                )
            try:
                precompile_proc = _run_completed_command(
                    [host_binary, "--precompile", manifest_value],
                    cwd=molt_root,
                    env=deployment.precompile_environment()
                    if deployment is not None
                    else None,
                    capture_output=True,
                    memory_guard_prefix="MOLT_WASM_LINK",
                    timeout=60,
                )
                if precompile_proc.returncode != 0:
                    detail = (
                        subprocess_output_text(precompile_proc.stderr).strip()
                        or subprocess_output_text(precompile_proc.stdout).strip()
                    )
                    raise ValueError(
                        "molt-wasm-host precompilation failed"
                        + (f": {detail}" if detail else "")
                    )
                receipt_artifacts = _precompile_receipt_artifacts(
                    subprocess_output_text(precompile_proc.stdout)
                )
                _validate_precompile_receipt_outputs(receipt_artifacts)
                assert deployment is not None
                expected_roles = {"main": "cwasm"}
                if _split_runtime:
                    expected_roles["runtime"] = "runtime_cwasm"
                if receipt_artifacts.keys() != expected_roles.keys():
                    raise ValueError(
                        "Host precompile receipt does not cover the deployment modules"
                    )
                for role, output_role in expected_roles.items():
                    artifact = receipt_artifacts[role]
                    source = deployment.outputs[
                        "runtime"
                        if role == "runtime"
                        else "app"
                        if _split_runtime
                        else "linked"
                    ]
                    if (
                        Path(str(artifact["path"])).resolve()
                        != deployment.outputs[output_role].resolve()
                        or Path(str(artifact["source"])).resolve() != source.resolve()
                        or artifact["source_sha256"]
                        != _file_asset(source, source.name)["sha256"]
                    ):
                        raise ValueError(
                            "Host precompile receipt escaped its private deployment generation"
                        )
            except subprocess.TimeoutExpired as exc:
                detail = (
                    subprocess_output_text(exc.stderr).strip()
                    or subprocess_output_text(exc.stdout).strip()
                )
                return None, _fail(
                    f"Precompilation timed out after {exc.timeout} seconds"
                    + (f": {detail}" if detail else ""),
                    json_output,
                    command="build",
                )
            except (OSError, ValueError) as exc:
                return None, _fail(
                    f"Precompilation failed: {exc}",
                    json_output,
                    command="build",
                )
            cwasm_path = str(receipt_artifacts["main"]["path"])
            artifacts["cwasm"] = cwasm_path
            if "runtime" in receipt_artifacts:
                runtime_cwasm_path = str(receipt_artifacts["runtime"]["path"])
                artifacts["runtime_cwasm"] = runtime_cwasm_path
            success_messages.append(f"Precompiled {cwasm_path}")

        prepared = _PreparedNonNativeResult(
            primary_output=primary_output,
            consumer_output=consumer_output,
            bundle_root=bundle_root,
            linked_output_path=resolved_linked_output,
            success_messages=success_messages,
            extra_fields={
                "linked": linked,
                "require_linked": require_linked,
                **(
                    {"linked_output": str(resolved_linked_output)}
                    if resolved_linked_output is not None
                    else {}
                ),
                **({"cwasm_output": cwasm_path} if cwasm_path is not None else {}),
                **(
                    {"runtime_cwasm_output": runtime_cwasm_path}
                    if runtime_cwasm_path is not None
                    else {}
                ),
            },
            artifacts=artifacts,
        )
        if deployment is not None:
            try:
                assert deployment_sources is not None
                deployment_sources.verify()
                deployment.publish(
                    link_fingerprints.FinalLinkReceiptRequest.from_fingerprint(
                        link_fingerprint_path, link_fingerprint
                    )
                )
            except (OSError, ValueError, RuntimeError) as exc:
                return None, _fail(
                    f"Failed to publish WASM deployment: {exc}",
                    json_output,
                    command="build",
                )
            prepared = deployment.public_result(prepared)
        return prepared, None
    return _PreparedNonNativeResult(
        primary_output=output_artifact,
        consumer_output=output_artifact,
        bundle_root=None,
        linked_output_path=linked_output_path,
        success_messages=[f"Successfully built {output_artifact}"],
        extra_fields={},
        artifacts={"object": str(output_artifact)},
    ), None
