from __future__ import annotations

import contextlib
import json
import os
import sys
from pathlib import Path
from typing import (
    Literal,
    Sequence,
)

from molt.cli.atomic_io import (
    _atomic_write_text,
)
from molt.cli.cargo_profiles import _resolve_cargo_profile_name
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
    DEFAULT_STDLIB_PROFILE,
)
from molt.cli.models import (
    BuildProfile,
    _RuntimeArtifactState,
)
from molt.cli.runtime_features import (
    runtime_stdlib_profile_for_required_features,
)
from molt.cli.runtime_paths import (
    _runtime_lib_path,
    _runtime_wasm_artifact_path,
)
from molt.cli.runtime_wasm_build_timings import (
    _runtime_wasm_build_timings_snapshot,
)
from molt.cli.runtime_wasm_pair_build import _ensure_runtime_wasm_both


def _initialize_runtime_artifact_state(
    *,
    is_rust_transpile: bool,
    is_wasm: bool,
    emit_mode: str,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    extra_runtime_features: Sequence[str] | None = None,
) -> _RuntimeArtifactState:
    state = _RuntimeArtifactState(
        extra_runtime_features=tuple(
            dict.fromkeys(
                feature.strip()
                for feature in (extra_runtime_features or ())
                if feature and feature.strip()
            )
        )
    )
    if is_rust_transpile:
        return state
    if is_wasm:
        state.runtime_wasm = _runtime_wasm_artifact_path(molt_root, "molt_runtime.wasm")
        state.runtime_reloc_wasm = _runtime_wasm_artifact_path(
            molt_root, "molt_runtime_reloc.wasm"
        )
        return state
    if emit_mode in {"bin", "obj"}:
        state.runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
            stdlib_profile=stdlib_profile,
        )
    return state


def _prebuild_runtime_wasm(
    *,
    project_root: Path,
    kind: Literal["shared", "reloc", "both"],
    json_output: bool,
    build_profile: BuildProfile,
    cargo_timeout: float | None,
    simd_enabled: bool = True,
    freestanding: bool = False,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    verbose: bool = False,
) -> int:
    def fail(
        message: str,
        *,
        runtime_state: _RuntimeArtifactState | None = None,
    ) -> int:
        # JSON mode owns stdout framing, not error suppression. Keep a stable
        # machine-readable failure envelope while the lower build/validation
        # boundary emits its precise diagnostic on stderr for CI and operators.
        if json_output:
            payload: dict[str, object] = {"status": "error", "error": message}
            if (
                runtime_state is not None
                and runtime_state.runtime_wasm_build_failure is not None
            ):
                payload["failure"] = (
                    runtime_state.runtime_wasm_build_failure.json_payload()
                )
            print(json.dumps(payload, sort_keys=True))
        else:
            print(message, file=sys.stderr)
        return 1

    cargo_profile, profile_error = _resolve_cargo_profile_name(build_profile)
    if profile_error is not None:
        return fail(profile_error)
    concrete_stdlib_profile = runtime_stdlib_profile_for_required_features(
        stdlib_profile,
        frozenset(),
        target_triple="wasm32-wasip1",
    )
    runtime_state = _initialize_runtime_artifact_state(
        is_rust_transpile=False,
        is_wasm=True,
        emit_mode="wasm",
        molt_root=project_root,
        runtime_cargo_profile=cargo_profile,
        target_triple=None,
        stdlib_profile=concrete_stdlib_profile,
    )
    if runtime_state.runtime_wasm is None or runtime_state.runtime_reloc_wasm is None:
        return fail("Runtime wasm shared/reloc artifact path is unavailable.")
    if verbose and not json_output:
        print(
            "Prebuilding atomic runtime wasm shared+reloc generation: "
            f"{runtime_state.runtime_wasm}",
            file=sys.stderr,
        )
    if not _ensure_runtime_wasm_both(
        runtime_state,
        json_output=json_output,
        cargo_profile=cargo_profile,
        cargo_timeout=cargo_timeout,
        project_root=project_root,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=concrete_stdlib_profile,
        resolved_modules=None,
        required_exports=None,
    ):
        return fail("Runtime wasm pair prebuild failed.", runtime_state=runtime_state)
    if (
        runtime_state.runtime_wasm_selected is None
        or runtime_state.runtime_reloc_wasm_selected is None
        or runtime_state.runtime_wasm_generation is None
    ):
        return fail("Runtime wasm pair publication selected no immutable generation.")
    all_artifacts = {
        "shared": os.fspath(runtime_state.runtime_wasm_selected),
        "reloc": os.fspath(runtime_state.runtime_reloc_wasm_selected),
        "generation": os.fspath(runtime_state.runtime_wasm_generation),
    }
    artifacts = (
        all_artifacts
        if kind == "both"
        else {kind: all_artifacts[kind], "generation": all_artifacts["generation"]}
    )
    _emit_runtime_wasm_build_timings(json_output=json_output)
    if json_output:
        print(
            json.dumps(
                {"status": "ok", "artifacts": artifacts},
                sort_keys=True,
            )
        )
    elif verbose:
        for label, path in artifacts.items():
            print(f"Runtime wasm {label}: {path}", file=sys.stderr)
    return 0


def _emit_runtime_wasm_build_timings(*, json_output: bool) -> None:
    """Emit the per-phase runtime-wasm build timings under MOLT_BUILD_DIAGNOSTICS.

    Extends the task #21 diagnostics surface to the standalone
    ``internal-runtime-wasm-build`` entry (which does not assemble the full
    build-diagnostics payload).  ``MOLT_BUILD_DIAGNOSTICS`` gates a compact,
    machine-readable JSON line on stderr and an optional JSON file at
    ``MOLT_BUILD_DIAGNOSTICS_FILE`` so the ``--kind both`` single-compile
    acceptance is a measured number (doctrine 74 law 4).
    """
    if os.environ.get("MOLT_BUILD_DIAGNOSTICS", "").strip().lower() not in {
        "1",
        "true",
        "yes",
        "on",
    }:
        return
    snapshot = _runtime_wasm_build_timings_snapshot()
    if snapshot is None:
        return
    line = json.dumps({"runtime_wasm_build": snapshot}, sort_keys=True)
    print(f"MOLT_BUILD_DIAGNOSTICS runtime_wasm_build: {line}", file=sys.stderr)
    out_spec = os.environ.get("MOLT_BUILD_DIAGNOSTICS_FILE", "").strip()
    if out_spec:
        with contextlib.suppress(OSError):
            _atomic_write_text(Path(out_spec), line + "\n")
