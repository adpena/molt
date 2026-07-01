from __future__ import annotations

import contextlib
import datetime as dt
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import uuid
from pathlib import Path
from typing import Any, Callable, Collection, TypedDict

from molt.cli import link_pipeline as _link_pipeline
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_json,
    _atomic_write_text,
    _remove_file_or_tree,
)
from molt.cli.build_results import _write_link_fingerprint_if_needed
from molt.cli.command_runtime import _run_completed_command
from molt.cli.external_native import _stage_external_package_native_artifacts_for_build
from molt.cli.models import (
    BuildProfile,
    _ExternalPackageNativeArtifactPlan,
    _PreparedNonNativeResult,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.output import CliFailure as _CliFailure, fail as _fail
from molt.cli.runtime_build import (
    _ensure_wasm_cpython_abi_staticlib,
)
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _read_runtime_fingerprint,
)
from molt.cli.runtime_wasm_validation import (
    _is_reusable_wasm_artifact,
    _validate_wasm_structural,
)
from molt.cli.wasm import (
    _effective_split_worker_table_base,
    _generate_split_worker_js,
    _generate_split_wrangler_jsonc,
    _runtime_import_result_kinds_from_manifest,
    _runtime_import_signatures_from_manifest,
    _split_runtime_browser_abi_from_manifest,
)
from molt.cli import wasm_toolchain
from molt.native_callable_abi import native_callable_browser_signature
from molt.wasm_artifact import (
    _collect_wasm_module_import_names,
    _wasm_export_function_signatures,
    _wasm_import_minima,
    wasm_table_ref_export_signatures,
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


class _ExternalStaticBundleFile(TypedDict):
    path: str
    size: int


class _ExternalStaticBundleManifest(TypedDict):
    files: list[_ExternalStaticBundleFile]
    roots: list[str]
    total_bytes: int


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
) -> _ExternalStaticBundleManifest | None:
    roots = tuple(
        dict.fromkeys(
            root.resolve(strict=False)
            for root in runtime_roots
            if root.exists() and root.is_dir()
        )
    )
    if not roots:
        return None

    files: list[_ExternalStaticBundleFile] = []
    seen: set[str] = set()
    tmp_output = output.with_name(f".{output.name}.{uuid.uuid4().hex}.tmp")
    try:
        with tarfile.open(tmp_output, "w") as tar:
            for root in sorted(roots, key=lambda path: str(path)):
                for path in sorted(root.rglob("*")):
                    arcname = _external_static_bundle_arcname(root, path)
                    if arcname is None:
                        continue
                    if arcname in seen:
                        raise ValueError(
                            "external static package bundle path collision: "
                            f"{arcname}"
                        )
                    seen.add(arcname)
                    payload = path.read_bytes()
                    info = tarfile.TarInfo(arcname)
                    info.size = len(payload)
                    info.mtime = 0
                    info.mode = 0o644
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    tar.addfile(info, io.BytesIO(payload))
                    files.append({"path": arcname, "size": len(payload)})

            if not files:
                return None

            manifest: _ExternalStaticBundleManifest = {
                "files": files,
                "roots": [str(root) for root in roots],
                "total_bytes": sum(file["size"] for file in files),
            }
            manifest_bytes = json.dumps(
                manifest,
                indent=2,
                sort_keys=True,
            ).encode("utf-8")
            manifest_info = tarfile.TarInfo("__manifest__.json")
            manifest_info.size = len(manifest_bytes)
            manifest_info.mtime = 0
            manifest_info.mode = 0o644
            manifest_info.uid = 0
            manifest_info.gid = 0
            manifest_info.uname = ""
            manifest_info.gname = ""
            tar.addfile(manifest_info, io.BytesIO(manifest_bytes))

        output.parent.mkdir(parents=True, exist_ok=True)
        os.replace(tmp_output, output)
        return manifest
    finally:
        with contextlib.suppress(FileNotFoundError):
            tmp_output.unlink()


def _runtime_export_signatures_for_imports(
    runtime_wasm: Path, import_names: set[str]
) -> dict[str, dict[str, object]]:
    import_to_export = {
        import_name: import_name
        if import_name.startswith("molt_")
        else f"molt_{import_name}"
        for import_name in import_names
    }
    export_signatures = _wasm_export_function_signatures(
        runtime_wasm,
        export_names=import_to_export.values(),
    )
    return {
        import_name: export_signatures[export_name]
        for import_name, export_name in sorted(import_to_export.items())
        if export_name in export_signatures
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


def _artifact_imports_module(path: Path, module_name: str) -> bool:
    try:
        return bool(_collect_wasm_module_import_names(path, module_name))
    except (OSError, ValueError):
        return True


def _is_reusable_static_native_link_artifact(path: Path) -> bool:
    return _is_reusable_wasm_artifact(path) and not _artifact_imports_module(
        path, "molt_native"
    )


def _is_reusable_split_runtime_artifacts(
    app_wasm: Path,
    runtime_wasm: Path,
    *,
    static_native_inputs: bool,
    wasm_table_base: int | None = None,
) -> bool:
    if not _is_reusable_wasm_artifact(app_wasm):
        return False
    if not _is_reusable_wasm_artifact(runtime_wasm):
        return False
    if static_native_inputs:
        if _artifact_imports_module(app_wasm, "molt_native"):
            return False
        if wasm_table_base is None:
            return False
        try:
            _effective_split_worker_table_base(
                wasm_table_base=wasm_table_base,
                app_table_ref_signatures=wasm_table_ref_export_signatures(app_wasm),
                app_wasm=app_wasm,
            )
        except (OSError, ValueError):
            return False
    return True


def _generate_snapshot_header(
    *,
    output_wasm: Path,
    target_profile: str,
    capabilities_list: list[str] | None,
    verbose: bool,
) -> None:
    """Generate a molt.snapshot.json header alongside the WASM output.

    The header captures mount plan, capability manifest, and module hash
    metadata needed by edge hosts to restore a post-init snapshot (Plan D).
    The binary memory blob capture is deferred to the wasmtime host
    integration.
    """
    snapshot_dir = output_wasm.parent
    snapshot_path = snapshot_dir / "molt.snapshot.json"

    # Compute module hash from the WASM binary.
    module_hash = "sha256:unknown"
    if output_wasm.exists():
        h = hashlib.sha256()
        with open(output_wasm, "rb") as f:
            for chunk in iter(lambda: f.read(65536), b""):
                h.update(chunk)
        module_hash = f"sha256:{h.hexdigest()}"

    # Default mount plan matching the spec Layer 4 snapshot format.
    mount_plan = [
        {"path": "/bundle", "mount_type": "bundle", "hash": module_hash},
        {"path": "/tmp", "mount_type": "tmp", "quota_mb": 32},
        {"path": "/dev", "mount_type": "dev"},
    ]

    caps = (
        list(capabilities_list)
        if capabilities_list
        else [
            "fs.bundle.read",
            "fs.tmp.read",
            "fs.tmp.write",
        ]
    )

    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": target_profile,
        "module_hash": module_hash,
        "mount_plan": mount_plan,
        "capability_manifest": caps,
        "determinism_stamp": dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "init_state_size": 0,
    }

    _atomic_write_json(snapshot_path, header, indent=2)
    if verbose:
        print(f"Wrote snapshot header: {snapshot_path}", file=sys.stderr)


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
    for artifact in native_artifact_plan.artifacts:
        artifact_payload = {
            "package": artifact.package,
            "module": artifact.module,
            "manifest_sha256": artifact.manifest_sha256,
            "extension_sha256": artifact.extension_sha256,
        }
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
            existing = symbols.get(export.symbol)
            if existing is None:
                symbols[export.symbol] = {
                    "abi": export.abi,
                    "binding": "direct_symbol",
                    "signature": native_callable_browser_signature(export.abi),
                    "exports": [export_payload],
                }
                continue
            if existing.get("abi") != export.abi:
                raise ValueError(
                    f"native symbol {export.symbol!r} has conflicting callable "
                    f"ABIs {existing.get('abi')!r} and {export.abi!r}"
                )
            existing_exports = existing.setdefault("exports", [])
            if not isinstance(existing_exports, list):
                raise ValueError(
                    f"native symbol {export.symbol!r} manifest exports were corrupted"
                )
            existing_exports.append(export_payload)
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


def _wasm_linkable_static_artifact_path(path: Path) -> bool:
    return path.suffix in {".a", ".o"} or path.name.endswith(".molt.wasm")


def _wasm_static_link_native_artifact_inputs(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> tuple[Path, ...]:
    out: list[Path] = []
    for artifact in artifacts:
        if artifact.runtime_linkage != "static_link" or artifact.artifact_kind not in {
            "wasm_relocatable_object",
            "static_archive",
        }:
            raise ValueError(
                "linked WASM external native artifacts must be wasm32 static_link "
                f"objects/archives; got {artifact.module}="
                f"{artifact.runtime_linkage}/{artifact.artifact_kind}"
            )
        out.append(artifact.staged_path)
        out.extend(
            path
            for path in artifact.staged_support_paths
            if _wasm_linkable_static_artifact_path(path)
        )
    return tuple(out)


def _staged_artifacts_need_cpython_abi_link(
    artifacts: tuple[_StagedExternalPackageNativeArtifact, ...],
) -> bool:
    return any(
        symbol.status == "cpython_abi_link"
        for artifact in artifacts
        for symbol in artifact.c_api_symbols
    ) or any(
        symbol.status == "external_link"
        and symbol.primitive_class == "molt_cpython_abi_link_import"
        for artifact in artifacts
        for symbol in artifact.abi_symbols
    )


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
    runtime_wasm: Path | None,
    runtime_reloc_wasm: Path | None,
    ensure_runtime_wasm_shared: Callable[[set[str] | frozenset[str] | None], bool],
    ensure_runtime_wasm_reloc: Callable[[set[str] | frozenset[str] | None], bool],
    runtime_cargo_profile: str,
    molt_root: Path,
    split_runtime: bool = False,
    precompile: bool = False,
    project_root: Path | None = None,
    profile: BuildProfile = "dev",
    warnings: list[str] | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan | None = None,
    artifacts_root: Path | None = None,
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
        resolved_linked_output = linked_output_path
        bundle_root: Path | None = None
        artifacts: dict[str, str] = {"wasm": str(output_wasm)}
        _split_runtime = split_runtime or os.environ.get("MOLT_SPLIT_RUNTIME") == "1"
        staged_runtime_wasm: Path | None = None
        if linked:
            staged_external_native_artifacts: tuple[
                _StagedExternalPackageNativeArtifact, ...
            ] = ()
            external_native_fingerprint_inputs: tuple[Path, ...] = ()
            wasm_static_link_native_inputs: tuple[Path, ...] = ()
            if native_artifact_plan is not None and native_artifact_plan.artifacts:
                try:
                    staged_external_native_artifacts = (
                        _stage_external_package_native_artifacts_for_build(
                            native_artifact_plan,
                            artifacts_root=artifacts_root or output_wasm.parent,
                        )
                    )
                    wasm_static_link_native_inputs = (
                        _wasm_static_link_native_artifact_inputs(
                            staged_external_native_artifacts
                        )
                    )
                    external_native_fingerprint_inputs = (
                        _external_native_artifact_fingerprint_inputs(
                            staged_external_native_artifacts
                        )
                    )
                    needs_cpython_abi_link = _staged_artifacts_need_cpython_abi_link(
                        staged_external_native_artifacts
                    )
                    needs_wasm_libc_link = _staged_artifacts_need_wasm_libc_link(
                        staged_external_native_artifacts
                    )
                    needs_wasm_compiler_rt_link = (
                        _staged_artifacts_need_wasm_compiler_rt_link(
                            staged_external_native_artifacts
                        )
                    )
                    if needs_cpython_abi_link:
                        cpython_abi_provider = _ensure_wasm_cpython_abi_staticlib(
                            project_root=molt_root,
                            json_output=json_output,
                            cargo_profile=runtime_cargo_profile,
                            cargo_timeout=None,
                        )
                        if cpython_abi_provider is None:
                            raise ValueError(
                                "cpython_abi_link symbols require a wasm32 "
                                "molt-cpython-abi staticlib provider"
                            )
                        wasm_static_link_native_inputs = (
                            *wasm_static_link_native_inputs,
                            cpython_abi_provider,
                        )
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            cpython_abi_provider,
                        )
                        needs_wasm_libc_link = True
                    if needs_wasm_libc_link:
                        libc_provider = wasm_toolchain.wasm_wasi_libc_archive()
                        if libc_provider is None:
                            raise ValueError(
                                "wasm_libc_link_import symbols require Rust "
                                "wasm32-wasip1 self-contained libc.a"
                            )
                        libc_provider = libc_provider.resolve(strict=False)
                        wasm_static_link_native_inputs = (
                            *wasm_static_link_native_inputs,
                            libc_provider,
                        )
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            libc_provider,
                        )
                    if needs_wasm_compiler_rt_link:
                        compiler_rt_provider = (
                            wasm_toolchain.wasm_compiler_builtins_archive()
                        )
                        if compiler_rt_provider is None:
                            raise ValueError(
                                "wasm_compiler_rt_link_import symbols require Rust "
                                "wasm32-wasip1 libcompiler_builtins provider"
                            )
                        compiler_rt_provider = compiler_rt_provider.resolve(
                            strict=False
                        )
                        wasm_static_link_native_inputs = (
                            *wasm_static_link_native_inputs,
                            compiler_rt_provider,
                        )
                        external_native_fingerprint_inputs = (
                            *external_native_fingerprint_inputs,
                            compiler_rt_provider,
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
            structural_error = _validate_wasm_structural(output_wasm)
            if structural_error is not None:
                return None, _fail(
                    "Generated wasm module failed structural validation before linking: "
                    + structural_error,
                    json_output,
                    command="build",
                )
            if not ensure_runtime_wasm_reloc(required_runtime_exports):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if runtime_reloc_wasm is None:
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if resolved_linked_output is None:
                resolved_linked_output = output_wasm.with_name("output_linked.wasm")
            if resolved_linked_output.parent != Path("."):
                resolved_linked_output.parent.mkdir(parents=True, exist_ok=True)
            if not is_wasm_freestanding:
                if runtime_wasm is None:
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if not ensure_runtime_wasm_shared(required_runtime_exports):
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if not runtime_wasm.exists():
                    return None, _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
            tool = molt_root / "tools" / "wasm_link.py"
            link_cmd = [
                sys.executable,
                str(tool),
                "--runtime",
                str(runtime_reloc_wasm),
                "--input",
                str(output_wasm),
                "--output",
                str(resolved_linked_output),
            ]
            for native_input in wasm_static_link_native_inputs:
                link_cmd.extend(["--native-object", str(native_input)])
            if _split_runtime:
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
            link_project_root = project_root or molt_root
            link_fingerprint_path = _link_pipeline._link_fingerprint_path(
                link_project_root,
                resolved_linked_output,
                profile,
                "wasm32-wasip1",
            )
            stored_link_fingerprint = _read_runtime_fingerprint(link_fingerprint_path)
            link_fingerprint = _link_pipeline._link_fingerprint(
                project_root=link_project_root,
                inputs=[
                    output_wasm,
                    runtime_reloc_wasm,
                    tool,
                    *external_native_fingerprint_inputs,
                ],
                link_cmd=link_cmd,
                stored_fingerprint=stored_link_fingerprint,
            )
            link_skipped = not _artifact_needs_rebuild(
                resolved_linked_output,
                link_fingerprint,
                stored_link_fingerprint,
            )
            if link_skipped and wasm_static_link_native_inputs:
                link_skipped = _is_reusable_static_native_link_artifact(
                    resolved_linked_output
                )
            if link_skipped and _split_runtime:
                split_dir = output_wasm.parent
                app_wasm = split_dir / "app.wasm"
                rt_wasm = split_dir / "molt_runtime.wasm"
                link_skipped = _is_reusable_split_runtime_artifacts(
                    app_wasm,
                    rt_wasm,
                    static_native_inputs=bool(wasm_static_link_native_inputs),
                    wasm_table_base=wasm_table_base,
                )
            if link_skipped:
                link_process = subprocess.CompletedProcess(link_cmd, 0, "", "")
            else:
                linked_tmp_output: Path | None = None
                link_run_cmd = list(link_cmd)
                if not _split_runtime:
                    linked_tmp_output = resolved_linked_output.with_name(
                        f".{resolved_linked_output.name}."
                        f"{os.getpid()}.{uuid.uuid4().hex}.tmp"
                    )
                    output_arg_index = link_run_cmd.index("--output") + 1
                    link_run_cmd[output_arg_index] = str(linked_tmp_output)
                try:
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
                    if linked_tmp_output is not None:
                        if not _is_reusable_wasm_artifact(linked_tmp_output):
                            return None, _fail(
                                f"Wasm link produced invalid artifact: {linked_tmp_output}",
                                json_output,
                                command="build",
                            )
                        os.replace(linked_tmp_output, resolved_linked_output)
                        if os.name == "posix":
                            with contextlib.suppress(OSError):
                                dir_fd = os.open(
                                    resolved_linked_output.parent,
                                    os.O_RDONLY,
                                )
                                try:
                                    os.fsync(dir_fd)
                                finally:
                                    os.close(dir_fd)
                finally:
                    if linked_tmp_output is not None:
                        with contextlib.suppress(OSError):
                            if linked_tmp_output.exists():
                                linked_tmp_output.unlink()
                link_fingerprint_warning = _write_link_fingerprint_if_needed(
                    link_skipped=False,
                    link_fingerprint=link_fingerprint,
                    link_fingerprint_path=link_fingerprint_path,
                    json_output=json_output,
                )
                if link_fingerprint_warning is not None:
                    if warnings is not None:
                        warnings.append(link_fingerprint_warning)
                    if not json_output:
                        print(f"Warning: {link_fingerprint_warning}", file=sys.stderr)
            if require_linked and resolved_linked_output is not None:
                if output_wasm != resolved_linked_output and output_wasm.exists():
                    try:
                        output_wasm.unlink()
                    except OSError as exc:
                        return None, _fail(
                            f"Failed to remove unlinked wasm: {exc}",
                            json_output,
                            command="build",
                        )
        if not is_wasm_freestanding and not _split_runtime and not linked:
            if runtime_wasm is None:
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            required_runtime_exports = _collect_wasm_module_import_names(
                output_wasm, "molt_runtime"
            )
            if not ensure_runtime_wasm_shared(required_runtime_exports):
                return None, _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if not runtime_wasm.exists():
                return None, _fail(
                    "Runtime wasm build failed",
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
        # -- Precompile step: produce .cwasm for faster startup -----------
        cwasm_path: Path | None = None
        if precompile:
            precompile_target = (
                resolved_linked_output
                if resolved_linked_output is not None
                else output_wasm
            )
            cwasm_path = precompile_target.with_suffix(".cwasm")
            wasmtime_bin = shutil.which("wasmtime")
            if wasmtime_bin:
                precompile_proc = _run_completed_command(
                    [
                        wasmtime_bin,
                        "compile",
                        str(precompile_target),
                        "-o",
                        str(cwasm_path),
                    ],
                    cwd=molt_root,
                    env=None,
                    capture_output=True,
                    memory_guard_prefix="MOLT_WASM_LINK",
                    timeout=60,
                )
                if precompile_proc.returncode == 0:
                    print(f"Precompiled to {cwasm_path}", file=sys.stderr)
                else:
                    print(
                        f"Precompilation failed (non-fatal): {precompile_proc.stderr.strip()}",
                        file=sys.stderr,
                    )
                    cwasm_path = None
            else:
                print("wasmtime not found; skipping precompilation", file=sys.stderr)
                cwasm_path = None
        # -- End precompile step -------------------------------------------
        if cwasm_path is not None:
            artifacts["cwasm"] = str(cwasm_path)

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
        if cwasm_path is not None:
            success_messages.append(f"Precompiled {cwasm_path}")

        # --split-runtime: wasm_link.py produces app.wasm + molt_runtime.wasm;
        # generate manifest.json and worker.js shim here.
        if _split_runtime and runtime_reloc_wasm is not None:
            split_dir = output_wasm.parent

            app_wasm = split_dir / "app.wasm"
            rt_wasm = split_dir / "molt_runtime.wasm"
            manifest = split_dir / "manifest.json"
            browser_embed_src = molt_root / "wasm" / "browser_embed.js"
            loader_bridge_src = molt_root / "wasm" / "loader_bridge.js"
            try:
                browser_embed_size = browser_embed_src.stat().st_size
                loader_bridge_size = loader_bridge_src.stat().st_size
            except OSError as exc:
                return None, _fail(
                    f"Missing split-runtime browser embed loader support: {exc}",
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

            app_size = app_wasm.stat().st_size
            rt_size = rt_wasm.stat().st_size
            app_memory_min, app_table_min = _wasm_import_minima(app_wasm)
            rt_memory_min, rt_table_min = _wasm_import_minima(rt_wasm)
            app_runtime_import_names = _collect_wasm_module_import_names(
                app_wasm, "molt_runtime"
            )
            app_native_callable_import_names = _collect_wasm_module_import_names(
                app_wasm, "molt_native"
            )
            app_runtime_import_result_kinds = (
                _runtime_import_result_kinds_from_manifest(app_runtime_import_names)
            )
            app_runtime_import_signatures = _runtime_import_signatures_from_manifest(
                app_runtime_import_names
            )
            app_runtime_export_signatures = _runtime_export_signatures_for_imports(
                rt_wasm, app_runtime_import_names
            )
            shared_memory_initial_pages = max(
                app_memory_min or 0,
                rt_memory_min or 0,
            )
            shared_table_initial = max(
                app_table_min or 0,
                rt_table_min or 0,
                8192,
            )
            app_table_ref_signatures = wasm_table_ref_export_signatures(app_wasm)
            runtime_table_ref_signatures = wasm_table_ref_export_signatures(rt_wasm)
            try:
                effective_wasm_table_base = _effective_split_worker_table_base(
                    wasm_table_base=wasm_table_base,
                    app_table_ref_signatures=app_table_ref_signatures,
                    app_wasm=app_wasm,
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
            bundle_manifest: _ExternalStaticBundleManifest | None = None
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
                "browser_embed": {
                    "path": "browser_embed.js",
                    "size": browser_embed_size,
                },
                "loader_bridge": {
                    "path": "loader_bridge.js",
                    "size": loader_bridge_size,
                },
            }
            manifest_data: dict[str, Any] = {
                "version": 2,
                "mode": "split-runtime",
                "tree_shaken": True,
                "shared_memory_initial_pages": shared_memory_initial_pages,
                "shared_table_initial": shared_table_initial,
                "wasm_table_base": effective_wasm_table_base,
                "abi": {
                    "runtime_imports": {
                        "module": "molt_runtime",
                        "names": sorted(app_runtime_import_names),
                        "signatures": app_runtime_import_signatures,
                        "runtime_export_signatures": app_runtime_export_signatures,
                        "result_kinds": app_runtime_import_result_kinds,
                    },
                    "browser_embed": browser_embed_abi,
                    "table_refs": {
                        "app": app_table_ref_signatures,
                        "runtime": runtime_table_ref_signatures,
                    },
                },
                "modules": {
                    "runtime": {
                        "path": "molt_runtime.wasm",
                        "size": rt_size,
                    },
                    "app": {
                        "path": "app.wasm",
                        "size": app_size,
                    },
                },
                "assets": assets,
                "total_size": app_size + rt_size,
                "instantiation_order": ["runtime", "app"],
                "entry": {"module": "app", "function": "molt_main"},
            }
            if bundle_manifest is not None:
                bundle_size = bundle_tar.stat().st_size
                assets["bundle"] = {
                    "path": "bundle.tar",
                    "size": bundle_size,
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
                    shared_memory_initial_pages=shared_memory_initial_pages,
                    shared_table_initial=shared_table_initial,
                    shared_table_base=effective_wasm_table_base,
                    runtime_import_names=app_runtime_import_names,
                    runtime_export_signatures=app_runtime_export_signatures,
                    app_table_ref_signatures=app_table_ref_signatures,
                    runtime_table_ref_signatures=runtime_table_ref_signatures,
                ),
            )
            vfs_support_src = molt_root / "wasm" / "molt_vfs_browser.js"
            vfs_support_dst = split_dir / "molt_vfs_browser.js"
            try:
                _atomic_copy_file(vfs_support_src, vfs_support_dst)
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime VFS support: {exc}",
                    json_output,
                    command="build",
                )
            browser_embed_dst = split_dir / "browser_embed.js"
            try:
                _atomic_copy_file(browser_embed_src, browser_embed_dst)
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime browser embed support: {exc}",
                    json_output,
                    command="build",
                )
            loader_bridge_dst = split_dir / "loader_bridge.js"
            try:
                _atomic_copy_file(loader_bridge_src, loader_bridge_dst)
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage split-runtime browser embed loader support: {exc}",
                    json_output,
                    command="build",
                )

            # Generate wrangler.jsonc for Cloudflare Workers deployment.
            # JSONC is the modern Wrangler config shape and matches the
            # live-verification tooling contract.
            wrangler_jsonc = split_dir / "wrangler.jsonc"
            _atomic_write_text(
                wrangler_jsonc,
                _generate_split_wrangler_jsonc(dt.date.today().isoformat()),
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

        return _PreparedNonNativeResult(
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
                **({"cwasm_output": str(cwasm_path)} if cwasm_path is not None else {}),
            },
            artifacts=artifacts,
        ), None
    return _PreparedNonNativeResult(
        primary_output=output_artifact,
        consumer_output=output_artifact,
        bundle_root=None,
        linked_output_path=linked_output_path,
        success_messages=[f"Successfully built {output_artifact}"],
        extra_fields={},
        artifacts={"object": str(output_artifact)},
    ), None
