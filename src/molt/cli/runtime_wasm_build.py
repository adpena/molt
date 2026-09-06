from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path

from molt._wasm_runtime_exports import (
    wasm_split_runtime_export_rename_map,
)
from molt.cli.artifact_state import (
    _build_state_root,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
)
from molt.cli.build_locks import _build_lock
from molt.cli.runtime_fingerprints import (
    _write_runtime_fingerprint,
)
from molt.cli.runtime_wasm_build_spec import (
    _RuntimeWasmBuildSpec,
)
from molt.cli.runtime_wasm_build_support import (
    _current_runtime_target_artifact,
    _link_runtime_staticlib_to_reloc_wasm,
    _runtime_missing_exports_for_mode,
    _wasm_runtime_staticlib_candidates,
    _wasm_runtime_wasm_candidates,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_shared_runtime_wasm_artifact,
)
from molt.wasm_artifact import (
    inspect_wasm_binary as _inspect_wasm_binary,
)
from molt.wasm_artifact import (
    transform_wasm_publication_file,
)


@dataclass(slots=True)
class _RuntimeWasmMemberFinalizer:
    runtime_wasm: Path
    reloc: bool
    json_output: bool
    cargo_timeout: float | None
    root: Path
    required_exports: set[str] | frozenset[str] | None
    spec: _RuntimeWasmBuildSpec

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
        started = time.perf_counter()
        try:
            if self.spec.cargo_plan is None:
                raise ValueError(
                    "runtime WASM publication requires its resolved Cargo plan"
                )
            preserve_debug = (
                self.reloc
                or self.spec.cargo_plan.preserve_debug_for_profile(
                    self.spec.cargo_profile
                )
            )
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
            raise ValueError(
                f"Runtime WASM publication transform failed: {exc}"
            ) from exc
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


def _reuse_target_runtime_wasm(
    ctx: _RuntimeWasmMemberFinalizer,
    *,
    persist_output_fingerprint: bool = True,
) -> bool | None:
    output_fingerprint = ctx.spec.fingerprint
    target_fingerprint = (
        ctx.spec.staticlib_fingerprint if ctx.reloc else output_fingerprint
    )
    if output_fingerprint is None or target_fingerprint is None:
        raise ValueError(
            "runtime WASM target admission requires its captured fingerprints"
        )
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
        fingerprint=target_fingerprint,
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
        if ctx.spec.cargo_plan is None or ctx.spec.link_inputs is None:
            raise ValueError("runtime WASM finalizer lost its resolved toolchain plan")
        if not _link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=artifact,
            output_path=ctx.runtime_wasm,
            json_output=ctx.json_output,
            link_timeout=ctx.cargo_timeout,
            export_link_args=ctx.spec.runtime_exports,
            cargo_plan=ctx.spec.cargo_plan,
            link_inputs=ctx.spec.link_inputs,
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
            raise ValueError(
                f"Copied runtime wasm artifact is invalid: {ctx.runtime_wasm}"
            )
    if not ctx.finalize_publication():
        return False
    if not ctx.reloc:
        missing = _runtime_missing_exports_for_mode(
            ctx.runtime_wasm, ctx.required_exports, reloc=False
        )
        if missing:
            raise ValueError(
                "Reused runtime wasm artifact missing required exports: "
                + ", ".join(sorted(missing))
            )
    try:
        target_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            target_fingerprint_path,
            target_fingerprint,
            artifact=artifact,
        )
        if persist_output_fingerprint:
            ctx.spec.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                ctx.spec.fingerprint_path,
                output_fingerprint,
                artifact=ctx.runtime_wasm,
            )
    except OSError as exc:
        raise ValueError(
            f"Failed to publish prebuilt runtime wasm metadata: {exc}"
        ) from exc
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

    ctx = _RuntimeWasmMemberFinalizer(
        runtime_wasm=destination,
        reloc=reloc,
        json_output=json_output,
        cargo_timeout=cargo_timeout,
        root=project_root,
        required_exports=required_exports,
        spec=spec,
    )
    if spec.fingerprint is None or spec.staticlib_fingerprint is None:
        raise ValueError(
            "runtime WASM member publication requires its captured fingerprints"
        )
    with _build_lock(ctx.root, ctx.lock_name):
        return bool(
            _reuse_target_runtime_wasm(
                ctx,
                persist_output_fingerprint=False,
            )
        )
