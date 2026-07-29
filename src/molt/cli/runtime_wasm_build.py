from __future__ import annotations

import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt._wasm_runtime_exports import (
    wasm_split_runtime_export_rename_map,
)
from molt.cli import wasm_toolchain
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_target_fingerprint_path,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
)
from molt.cli.build_locks import _build_lock
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.runtime_artifact_selection import (
    RuntimeCrateType,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_wasm_build_spec import (
    _compute_runtime_wasm_build_spec,
    _RuntimeWasmBuildSpec,
)
from molt.cli.runtime_wasm_build_support import (
    _configure_wasi_sysroot_env,
    _configure_wasm_cc_env,
    _configure_wasm_long_double_env,
    _current_runtime_target_artifact,
    _link_runtime_staticlib_to_reloc_wasm,
    _reloc_runtime_requires_long_double,
    _resolve_reloc_long_double_archives,
    _run_runtime_wasm_cargo_build,
    _runtime_exports_satisfy_for_mode,
    _runtime_missing_exports_for_mode,
    _wasm_runtime_staticlib_candidates,
    _wasm_runtime_wasm_candidates,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
    _shared_runtime_wasm_validation_error,
)
from molt.wasm_artifact import (
    inspect_wasm_binary as _inspect_wasm_binary,
)
from molt.wasm_artifact import (
    strip_wasm_publication_sections,
    transform_wasm_publication_file,
)


def _runtime_publication_bytes(
    data: bytes, *, reloc: bool, preserve_debug: bool
) -> bytes:
    if reloc:
        return data
    return strip_wasm_publication_sections(
        data, final_artifact=True, preserve_debug=preserve_debug
    )


@dataclass(slots=True)
class _RuntimeWasmMemberBuild:
    runtime_wasm: Path
    reloc: bool
    json_output: bool
    cargo_timeout: float | None
    root: Path
    required_exports: set[str] | frozenset[str] | None
    build_if_missing: bool
    spec: _RuntimeWasmBuildSpec
    fingerprint: dict[str, Any]
    staticlib_fingerprint: dict[str, Any]
    long_double_required: bool
    stored_fingerprint: dict[str, Any] | None

    @property
    def kind(self) -> str:
        return "reloc" if self.reloc else "shared"

    @property
    def lock_name(self) -> str:
        return f"runtime.{self.spec.cargo_profile}.wasm32-wasip1.{self.kind}"

    @property
    def target_build_state_root(self) -> Path:
        return _build_state_root(self.root)

    def finalize_publication(self) -> bool:
        # Relocatable objects retain every custom section until the final link.
        # Removing ``name``/debug sections shifts section ordinals while the
        # linking symbol table and reloc.* sections still reference the original
        # indices, producing an object that validates as core WASM but crashes
        # LLVM when consumed. Only final shared artifacts may strip them.
        preserve_debug = self.reloc or any(
            marker in self.spec.cargo_profile.lower() for marker in ("dev", "debug")
        )
        started = time.perf_counter()
        try:
            metrics = transform_wasm_publication_file(
                self.runtime_wasm,
                rename_map=(
                    {}
                    if self.reloc
                    else wasm_split_runtime_export_rename_map(self.required_exports)
                ),
                final_artifact=not self.reloc,
                preserve_debug=preserve_debug,
            )
        except (OSError, ValueError) as exc:
            if not self.json_output:
                print(
                    f"Runtime WASM publication transform failed: {exc}", file=sys.stderr
                )
            return False
        _record_runtime_wasm_build_phase(
            "publication_transform",
            time.perf_counter() - started,
            kind=self.kind,
            mode="bounded_mmap",
            detail=(
                f"input={metrics.input_bytes} output={metrics.output_bytes} "
                f"scanned={metrics.scanned_bytes} written={metrics.written_bytes} "
                f"max_buffer={metrics.max_buffer_bytes} changed={metrics.changed}"
            ),
        )
        return True


def _prepare_runtime_wasm_member_build(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path | None,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    build_if_missing: bool,
) -> _RuntimeWasmMemberBuild | None:
    root = project_root or _compiler_root()
    linker: wasm_toolchain.WasmLinkerIdentity | None = None
    if reloc:
        try:
            linker = wasm_toolchain.resolve_wasm_linker()
        except wasm_toolchain.WasmLinkerContractError as exc:
            if not json_output:
                print(f"Runtime wasm linker contract failed: {exc}", file=sys.stderr)
            return None
        if linker is None:
            if not json_output:
                print(
                    "Runtime wasm linker contract failed: wasm-ld not found.",
                    file=sys.stderr,
                )
            return None
    spec = _compute_runtime_wasm_build_spec(
        root,
        runtime_wasm,
        reloc=reloc,
        cargo_profile=cargo_profile,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        required_link_features=required_link_features,
        required_exports=required_exports,
        wasm_linker_identity=linker,
    )
    if spec.fingerprint is None:
        if not json_output:
            print("Failed to compute runtime wasm fingerprint.", file=sys.stderr)
        return None
    if spec.staticlib_fingerprint is None:
        if not json_output:
            print(
                "Failed to compute runtime wasm staticlib fingerprint.",
                file=sys.stderr,
            )
        return None
    long_double_required = reloc and _reloc_runtime_requires_long_double(
        resolved_modules=resolved_modules,
        required_exports=required_exports,
    )
    if long_double_required:
        archives = _resolve_reloc_long_double_archives(long_double_required=True)
        if archives.error is not None:
            if not json_output:
                print(archives.error, file=sys.stderr)
            return None
    return _RuntimeWasmMemberBuild(
        runtime_wasm=runtime_wasm,
        reloc=reloc,
        json_output=json_output,
        cargo_timeout=cargo_timeout,
        root=root,
        required_exports=required_exports,
        build_if_missing=build_if_missing,
        spec=spec,
        fingerprint=spec.fingerprint,
        staticlib_fingerprint=spec.staticlib_fingerprint,
        long_double_required=long_double_required,
        stored_fingerprint=spec.stored_fingerprint,
    )


def _reuse_published_runtime_wasm(ctx: _RuntimeWasmMemberBuild) -> bool | None:
    needs_rebuild = not _runtime_artifact_fingerprint_matches(
        ctx.runtime_wasm,
        ctx.fingerprint,
        ctx.spec.fingerprint_path,
        require_artifact_digest=True,
    )
    valid = (
        _is_valid_runtime_wasm_artifact(ctx.runtime_wasm)
        if ctx.reloc
        else _is_valid_shared_runtime_wasm_artifact(ctx.runtime_wasm)
    )
    exports_ready = ctx.reloc or _runtime_exports_satisfy_for_mode(
        ctx.runtime_wasm, ctx.required_exports, reloc=ctx.reloc
    )
    if needs_rebuild or not valid or not exports_ready:
        if not needs_rebuild and not ctx.json_output:
            message = (
                "Runtime wasm artifact missing required exports; forcing rebuild."
                if not exports_ready
                else "Runtime wasm artifact invalid/corrupt; forcing rebuild."
            )
            print(message, file=sys.stderr)
        return None
    try:
        if not ctx.finalize_publication():
            return False
        ctx.spec.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            ctx.spec.fingerprint_path,
            ctx.fingerprint,
            artifact=ctx.runtime_wasm,
        )
    except OSError:
        if not ctx.json_output:
            print(
                "Failed to update runtime wasm fingerprint metadata.", file=sys.stderr
            )
        return False
    return True


def _reuse_target_runtime_wasm(
    ctx: _RuntimeWasmMemberBuild,
    *,
    persist_output_fingerprint: bool = True,
) -> bool | None:
    target_label = "wasm32-wasip1"
    candidates = (
        _wasm_runtime_staticlib_candidates(ctx.spec.target_root, ctx.spec.profile_dir)
        if ctx.reloc
        else _wasm_runtime_wasm_candidates(ctx.spec.target_root, ctx.spec.profile_dir)
    )
    target = _current_runtime_target_artifact(
        candidates,
        build_state_root=ctx.target_build_state_root,
        cargo_profile=ctx.spec.cargo_profile,
        target_label=target_label,
        fingerprint=(ctx.staticlib_fingerprint if ctx.reloc else ctx.fingerprint),
    )
    if target is None:
        return None
    artifact, target_fingerprint_path = target
    if not ctx.reloc and (
        _inspect_wasm_binary(artifact) != "valid"
        or not _is_valid_shared_runtime_wasm_artifact(artifact)
    ):
        return None
    _record_runtime_wasm_build_phase(
        "cargo_compile",
        0.0,
        kind=ctx.kind,
        mode="target_reuse",
        detail=(
            "staticlib reused from cargo target dir"
            if ctx.reloc
            else "cdylib reused from cargo target dir"
        ),
    )
    if ctx.reloc:
        started = time.perf_counter()
        if not _link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=artifact,
            output_path=ctx.runtime_wasm,
            json_output=ctx.json_output,
            link_timeout=ctx.cargo_timeout,
            export_link_args=ctx.spec.runtime_exports,
            long_double_required=ctx.long_double_required,
        ):
            return False
        _record_runtime_wasm_build_phase(
            "reloc_link",
            time.perf_counter() - started,
            kind="reloc",
            mode="link",
        )
    else:
        ctx.runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
        _atomic_copy_file(artifact, ctx.runtime_wasm)
        if _inspect_wasm_binary(ctx.runtime_wasm) != "valid":
            if not ctx.json_output:
                print(
                    f"Copied runtime wasm artifact is invalid: {ctx.runtime_wasm}",
                    file=sys.stderr,
                )
            return False
    if not ctx.finalize_publication():
        return False
    if not ctx.reloc:
        missing = _runtime_missing_exports_for_mode(
            ctx.runtime_wasm, ctx.required_exports, reloc=False
        )
        if missing:
            if not ctx.json_output:
                print(
                    "Reused runtime wasm artifact missing required exports: "
                    + ", ".join(sorted(missing)),
                    file=sys.stderr,
                )
            return False
    try:
        target_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            target_fingerprint_path,
            ctx.staticlib_fingerprint if ctx.reloc else ctx.fingerprint,
            artifact=artifact,
        )
        if persist_output_fingerprint:
            ctx.spec.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                ctx.spec.fingerprint_path,
                ctx.fingerprint,
                artifact=ctx.runtime_wasm,
            )
    except OSError:
        if not ctx.json_output:
            print("Failed to publish prebuilt runtime wasm metadata.", file=sys.stderr)
        return False
    return True


def _materialize_runtime_wasm_member_from_target(
    destination: Path,
    *,
    reloc: bool,
    json_output: bool,
    cargo_timeout: float | None,
    project_root: Path,
    required_exports: set[str] | frozenset[str] | None,
    resolved_modules: set[str] | frozenset[str] | None,
    spec: _RuntimeWasmBuildSpec,
) -> bool:
    """Finalize one transient pair member from an exact canonical target spec.

    The combined pair producer has already built and fingerprinted both Cargo
    artifacts.  A unique publication destination is custody, not build
    identity, so it must not create a second UUID-keyed spec or fingerprint
    authority.
    """

    long_double_required = reloc and _reloc_runtime_requires_long_double(
        resolved_modules=resolved_modules,
        required_exports=required_exports,
    )
    if long_double_required:
        archives = _resolve_reloc_long_double_archives(long_double_required=True)
        if archives.error is not None:
            if not json_output:
                print(archives.error, file=sys.stderr)
            return False
    ctx = _RuntimeWasmMemberBuild(
        runtime_wasm=destination,
        reloc=reloc,
        json_output=json_output,
        cargo_timeout=cargo_timeout,
        root=project_root,
        required_exports=required_exports,
        build_if_missing=False,
        spec=spec,
        fingerprint=spec.fingerprint or {},
        staticlib_fingerprint=spec.staticlib_fingerprint or {},
        long_double_required=long_double_required,
        stored_fingerprint=None,
    )
    if spec.fingerprint is None or spec.staticlib_fingerprint is None:
        return False
    with _build_lock(ctx.root, ctx.lock_name):
        return bool(
            _reuse_target_runtime_wasm(
                ctx,
                persist_output_fingerprint=False,
            )
        )


def _runtime_wasm_cargo_command(ctx: _RuntimeWasmMemberBuild) -> list[str]:
    cmd = [
        "cargo",
        "rustc",
        "--package",
        "molt-runtime",
        "--profile",
        ctx.spec.cargo_profile,
        "--target",
        "wasm32-wasip1",
        "--lib",
    ]
    if ctx.spec.no_default_features:
        cmd.append("--no-default-features")
    if ctx.spec.wasm_cargo_features:
        cmd.extend(["--features", ",".join(ctx.spec.wasm_cargo_features)])
    ctx.spec.artifact_selection.select_in(cmd)
    if not ctx.reloc:
        cmd.append("--")
        if ctx.spec.cargo_link_response_path is not None:
            cmd.extend(["-C", f"link-arg=@{ctx.spec.cargo_link_response_path}"])
    return cmd


def _publish_built_reloc_runtime_wasm(ctx: _RuntimeWasmMemberBuild, src: Path) -> bool:
    if not src.exists():
        if not ctx.json_output:
            print(
                "Runtime wasm build succeeded but staticlib artifact is missing.",
                file=sys.stderr,
            )
        return False
    started = time.perf_counter()
    if not _link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=src,
        output_path=ctx.runtime_wasm,
        json_output=ctx.json_output,
        link_timeout=ctx.cargo_timeout,
        export_link_args=ctx.spec.runtime_exports,
        long_double_required=ctx.long_double_required,
    ):
        return False
    _record_runtime_wasm_build_phase(
        "reloc_link", time.perf_counter() - started, kind="reloc", mode="link"
    )
    if not ctx.finalize_publication():
        return False
    ctx.spec.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
    _write_runtime_fingerprint(
        ctx.spec.fingerprint_path, ctx.fingerprint, artifact=ctx.runtime_wasm
    )
    target_fingerprint_path = _runtime_target_fingerprint_path(
        ctx.target_build_state_root,
        src,
        cargo_profile=ctx.spec.cargo_profile,
        target_label="wasm32-wasip1",
    )
    target_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
    _write_runtime_fingerprint(
        target_fingerprint_path, ctx.staticlib_fingerprint, artifact=src
    )
    return True


def _publish_built_shared_runtime_wasm(ctx: _RuntimeWasmMemberBuild, src: Path) -> bool:
    src_state = _inspect_wasm_binary(src)
    if src_state != "valid":
        if not ctx.json_output:
            message = (
                "Runtime wasm build succeeded but artifact is missing."
                if src_state == "missing"
                else f"Runtime wasm build produced an invalid artifact and failed closed: {src}."
            )
            print(message, file=sys.stderr)
        return False
    if not _is_valid_shared_runtime_wasm_artifact(src):
        error = _shared_runtime_wasm_validation_error(src)
        print(
            "Runtime wasm build produced an unusable shared artifact: "
            f"{error or 'validation failed'}.",
            file=sys.stderr,
        )
        return False
    ctx.runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    _atomic_copy_file(src, ctx.runtime_wasm)
    if _inspect_wasm_binary(ctx.runtime_wasm) != "valid":
        if not ctx.json_output:
            print(
                f"Copied runtime wasm artifact is invalid: {ctx.runtime_wasm}",
                file=sys.stderr,
            )
        return False
    if not ctx.finalize_publication():
        return False
    try:
        missing = _runtime_missing_exports_for_mode(
            ctx.runtime_wasm, ctx.required_exports, reloc=False
        )
    except OSError:
        if not ctx.json_output:
            print(
                "Failed to update runtime wasm fingerprint metadata.",
                file=sys.stderr,
            )
        return False
    if missing:
        if not ctx.json_output:
            print(
                "Runtime wasm build produced artifact missing required exports: "
                + ", ".join(sorted(missing)),
                file=sys.stderr,
            )
        return False
    try:
        target_fingerprint_path = _runtime_target_fingerprint_path(
            ctx.target_build_state_root,
            src,
            cargo_profile=ctx.spec.cargo_profile,
            target_label="wasm32-wasip1",
        )
        target_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            target_fingerprint_path, ctx.fingerprint, artifact=src
        )
        ctx.spec.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            ctx.spec.fingerprint_path, ctx.fingerprint, artifact=ctx.runtime_wasm
        )
    except OSError:
        if not ctx.json_output:
            print(
                "Warning: failed to write runtime fingerprint metadata.",
                file=sys.stderr,
            )
    return True


def _build_runtime_wasm_member(ctx: _RuntimeWasmMemberBuild) -> bool:
    if not ctx.build_if_missing:
        if not ctx.json_output:
            print(
                "Combined runtime wasm artifacts could not be finalized; "
                "refusing a redundant per-artifact Cargo compile.",
                file=sys.stderr,
            )
        return False
    if wasm_toolchain.rust_target_libdir("wasm32-wasip1") is None:
        if not ctx.json_output:
            print(
                wasm_toolchain.rust_target_missing_message(
                    "wasm32-wasip1", root=ctx.root, context="Runtime wasm build"
                ),
                file=sys.stderr,
            )
        return False
    if not ctx.json_output:
        print("Runtime sources changed; rebuilding runtime...", file=sys.stderr)
    env = dict(ctx.spec.env)
    if ctx.spec.cargo_rustflags:
        env["RUSTFLAGS"] = ctx.spec.cargo_rustflags
    if os.environ.get("MOLT_WASM_FORCE_CC") == "1":
        _configure_wasm_cc_env(env)
    _configure_wasi_sysroot_env(env)
    _configure_wasm_long_double_env(env)
    started = time.perf_counter()
    try:
        build, src = _run_runtime_wasm_cargo_build(
            cmd=_runtime_wasm_cargo_command(ctx),
            root=ctx.root,
            env=env,
            cargo_timeout=ctx.cargo_timeout,
            profile_dir=ctx.spec.profile_dir,
            target_root_override=ctx.spec.target_root,
            json_output=ctx.json_output,
            artifact_kind=(
                RuntimeCrateType.STATICLIB if ctx.reloc else RuntimeCrateType.CDYLIB
            ),
        )
    except subprocess.TimeoutExpired:
        if not ctx.json_output:
            message = (
                f"Runtime wasm build timed out after {ctx.cargo_timeout:.1f}s."
                if ctx.cargo_timeout is not None
                else "Runtime wasm build timed out."
            )
            print(message, file=sys.stderr)
        return False
    if build.returncode != 0:
        detail = (build.stderr or build.stdout or "").strip()
        print(
            f"Runtime wasm build failed{f': {detail}' if detail else ''}",
            file=sys.stderr,
        )
        return False
    _record_runtime_wasm_build_phase(
        "cargo_compile",
        time.perf_counter() - started,
        kind=ctx.kind,
        mode="build",
        detail=(
            "target_dir=stable-incremental (cross-session dep cache)"
            if ctx.spec.incremental_enabled
            else "target_dir=session"
        ),
    )
    return (
        _publish_built_reloc_runtime_wasm(ctx, src)
        if ctx.reloc
        else _publish_built_shared_runtime_wasm(ctx, src)
    )


def _ensure_runtime_wasm(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path | None = None,
    simd_enabled: bool = True,
    freestanding: bool = False,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    required_link_features: frozenset[str] = frozenset(),
    required_exports: set[str] | frozenset[str] | None = None,
    build_if_missing: bool = True,
) -> bool:
    ctx = _prepare_runtime_wasm_member_build(
        runtime_wasm,
        reloc=reloc,
        json_output=json_output,
        cargo_profile=cargo_profile,
        cargo_timeout=cargo_timeout,
        project_root=project_root,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        required_link_features=required_link_features,
        required_exports=required_exports,
        build_if_missing=build_if_missing,
    )
    if ctx is None:
        return False
    with _build_lock(ctx.root, ctx.lock_name):
        if ctx.stored_fingerprint is None:
            ctx.stored_fingerprint = _read_runtime_fingerprint(
                ctx.spec.fingerprint_path
            )
        if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
            return False
        published = _reuse_published_runtime_wasm(ctx)
        if published is not None:
            return published
        target = _reuse_target_runtime_wasm(ctx)
        if target is not None:
            return target
        return _build_runtime_wasm_member(ctx)
