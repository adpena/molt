from __future__ import annotations

import functools
import hashlib
import os
from dataclasses import dataclass, replace
from enum import Enum
from typing import Mapping, TYPE_CHECKING
from pathlib import Path

from molt.cli.cargo_profiles import _CARGO_PROFILE_NAME_RE
from molt.dx import (
    PROOF_SCRATCH_ROOT_ENV,
    generated_session_id,
    project_cargo_target_dir,
)
from molt.cli.build_request import BuildProfile

if TYPE_CHECKING:
    from molt.cli.runtime_cargo_plan import RuntimeCargoPlan


def _runtime_build_profile_override() -> str:
    """Opt-in iteration-loop override for the runtime-wasm cargo profile.

    ``MOLT_RUNTIME_BUILD_PROFILE`` (e.g. ``dev-fast``) explicitly selects the
    runtime-WASM Cargo profile for an iteration loop. Without either WASM
    override, the public release request resolves to ``wasm-release``; an
    explicitly requested physical profile such as ``release-output`` remains
    selected. Invalid iteration profile names are ignored; a valid but unknown
    profile is rejected by Cargo.
    """
    raw = os.environ.get("MOLT_RUNTIME_BUILD_PROFILE", "").strip()
    if raw and _CARGO_PROFILE_NAME_RE.match(raw):
        return raw
    return ""


@functools.lru_cache(maxsize=32)
def _resolve_wasm_cargo_profile_cached(
    cargo_profile: str,
    override: str,
    runtime_build_profile: str,
) -> str:
    # Precedence: the explicit MOLT_WASM_CARGO_PROFILE override (pre-existing
    # contract) wins, then the MOLT_RUNTIME_BUILD_PROFILE iteration knob, then
    # the derived default. Keeping the iteration knob below the explicit
    # override means an operator who pinned MOLT_WASM_CARGO_PROFILE is never
    # surprised by it.
    if override:
        return override
    if runtime_build_profile:
        return runtime_build_profile
    if cargo_profile == "release":
        return "wasm-release"
    return cargo_profile


def runtime_wasm_simd_policy() -> bool:
    """Every WASM deployment uses the shipped SIMD128 runtime baseline.

    WASI import rewriting supports SIMD instructions, so freestanding selects
    host-service policy independently of the runtime's instruction set.
    """
    return True


def _resolve_wasm_cargo_profile(cargo_profile: str) -> str:
    """Map cargo profile for WASM targets.

    Uses the explicit ``wasm-release`` profile instead of generic ``release``
    so WASM artifact size/perf policy can move independently from native
    staticlib policy. Override with ``MOLT_WASM_CARGO_PROFILE`` (explicit) or the
    iteration-scoped ``MOLT_RUNTIME_BUILD_PROFILE`` (e.g. ``dev-fast``).
    """
    return _resolve_wasm_cargo_profile_cached(
        cargo_profile,
        os.environ.get("MOLT_WASM_CARGO_PROFILE", "").strip(),
        _runtime_build_profile_override(),
    )


class RuntimeWasmTargetLayout(Enum):
    FAMILY = "stable-family"
    CHECKOUT = "checkout"
    SESSION = "session"
    EXPLICIT = "explicit"


class RuntimeWasmIncrementality(Enum):
    PROFILE = "profile-pending"
    REQUESTED_ON = "enabled-requested"
    FORCED_OFF = "disabled-pending"
    ENABLED = "enabled"
    DISABLED = "disabled"


@dataclass(frozen=True, slots=True)
class RuntimeWasmBuildPolicy:
    """Request custody and actual Cargo execution are separate facts.

    Layout is fixed before Cargo capture. Incrementality binds from that exact
    plan before diagnostics or publication; neither can infer it from layout.
    """

    build_profile: BuildProfile
    layout: RuntimeWasmTargetLayout
    incrementality: RuntimeWasmIncrementality

    def bind(self, plan: RuntimeCargoPlan) -> RuntimeWasmBuildPolicy:
        enabled = plan.incremental_for_profile(
            plan.cargo_profile, package_name="molt-runtime", workspace_package=True
        )
        if self.incrementality is RuntimeWasmIncrementality.FORCED_OFF and enabled:
            raise ValueError(
                "runtime WASM nonincremental constraint changed in Cargo capture"
            )
        return replace(
            self,
            incrementality=(
                RuntimeWasmIncrementality.ENABLED
                if enabled
                else RuntimeWasmIncrementality.DISABLED
            ),
        )

    @property
    def publish_shared_cache(self) -> bool:
        if self.incrementality in {
            RuntimeWasmIncrementality.PROFILE,
            RuntimeWasmIncrementality.REQUESTED_ON,
            RuntimeWasmIncrementality.FORCED_OFF,
        }:
            raise ValueError(
                "runtime WASM publication requires resolved incrementality"
            )
        return self.incrementality is RuntimeWasmIncrementality.DISABLED

    @property
    def diagnostic_detail(self) -> str:
        if self.incrementality in {
            RuntimeWasmIncrementality.PROFILE,
            RuntimeWasmIncrementality.REQUESTED_ON,
            RuntimeWasmIncrementality.FORCED_OFF,
        }:
            raise ValueError("runtime WASM diagnostics require resolved incrementality")
        return (
            f"build_profile={self.build_profile} target_dir={self.layout.value} "
            f"incremental={self.incrementality.value} "
            f"shared_cache_publication={int(self.publish_shared_cache)}"
        )


def resolve_runtime_wasm_build_policy(
    build_profile: BuildProfile, environ: Mapping[str, str]
) -> RuntimeWasmBuildPolicy:
    """Preserve explicit opt-outs and proof custody without guessing profile names.

    Cargo owns effective profile incrementality after sccache normalization:
    https://doc.rust-lang.org/cargo/reference/profiles.html#incremental
    https://github.com/mozilla/sccache/blob/main/docs/Rust.md
    """
    raw = environ.get("MOLT_RUNTIME_WASM_INCREMENTAL", "").strip().lower()
    explicit_on = raw in {"1", "true", "yes", "on"}
    explicit_off = raw in {"0", "false", "no", "off"}
    override = environ.get("MOLT_RUNTIME_BUILD_PROFILE", "").strip()
    explicit_iteration = (
        bool(override and _CARGO_PROFILE_NAME_RE.fullmatch(override)) or explicit_on
    )
    isolated = bool(environ.get(PROOF_SCRATCH_ROOT_ENV))
    reproducible = "SOURCE_DATE_EPOCH" in environ
    stable = explicit_iteration and not (explicit_off or isolated or reproducible)
    # Exact caller targets and deliberately pinned sessions keep their custody.
    # A generated run label is telemetry, not a request for cold build state.
    pinned_session = bool(environ.get("MOLT_SESSION_ID")) and not generated_session_id(
        environ
    )
    layout = (
        RuntimeWasmTargetLayout.EXPLICIT
        if environ.get("CARGO_TARGET_DIR")
        else RuntimeWasmTargetLayout.SESSION
        if pinned_session
        else RuntimeWasmTargetLayout.FAMILY
        if stable
        else RuntimeWasmTargetLayout.CHECKOUT
    )
    disabled = (
        explicit_off
        or isolated
        or reproducible
        or environ.get("CARGO_INCREMENTAL") == "0"
        or (not explicit_iteration and "CARGO_INCREMENTAL" not in environ)
    )
    return RuntimeWasmBuildPolicy(
        build_profile,
        layout,
        RuntimeWasmIncrementality.FORCED_OFF
        if disabled
        else RuntimeWasmIncrementality.REQUESTED_ON
        if explicit_iteration
        else RuntimeWasmIncrementality.PROFILE,
    )


def _runtime_wasm_family_key(
    *,
    cargo_profile: str,
    target_triple: str,
    features: tuple[str, ...],
    simd_enabled: bool,
    freestanding: bool,
) -> str:
    """Codegen-family key for the stable runtime-wasm target dir.

    Deliberately EXCLUDES link-args (export allowlist, ``--import-memory`` /
    ``--import-table`` / ``--growable-table``) which are the *only* thing that
    differs between the reloc/staticlib and shared/cdylib passes.  The combined
    producer selects exactly ``staticlib,cdylib`` with Cargo's crate-type option;
    link-args only re-drive the final cdylib link.  Keying the shared incremental
    dir on the codegen family therefore lets the second pass reuse the first pass's
    object code (near-pure re-link) and
    lets consecutive same-config iterations recompile incrementally instead of
    from scratch.
    """
    payload = "\n".join(
        [
            f"profile:{cargo_profile}",
            f"target:{target_triple}",
            f"simd:{int(simd_enabled)}",
            f"freestanding:{int(freestanding)}",
            "features:" + ",".join(sorted(features)),
        ]
    )
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16]
    return f"{cargo_profile}-{digest}"


def _runtime_wasm_family_target_root(project_root: Path, family_key: str) -> Path:
    """Stable, session-independent Cargo target, including nonincremental reuse.

    The existing runtime-wasm-incr directory spelling is retained to reuse warm
    Cargo state. It does not claim that rustc incrementality is enabled.

    Session-independent by design: ordinary iteration reuses dependencies and,
    when Cargo enables it, incremental state. Proof requests retain their own
    target custody instead. Concurrency across sessions building the same family is
    made safe by cargo's own per-target build lock plus the ``_build_slot()``
    cross-process gate. The canonical target selector separates source checkouts:
    serializing divergent sources in one Cargo target does not establish correct
    freshness. Explicit shared target overrides remain caller-owned.
    """
    base = project_cargo_target_dir(project_root, os.environ, session_scoped=False)
    return base / "runtime-wasm-incr" / family_key
