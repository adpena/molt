from __future__ import annotations

import functools
import hashlib
import os
from pathlib import Path

from molt.cli.cargo_profiles import _CARGO_PROFILE_NAME_RE


def _runtime_build_profile_override() -> str:
    """Opt-in iteration-loop override for the runtime-wasm cargo profile.

    ``MOLT_RUNTIME_BUILD_PROFILE`` (e.g. ``dev-fast``) swaps the runtime-wasm
    cargo profile so a correctness-iteration loop (the E1 witness numpy-import
    debug loop) does not pay full ``release-output`` (fat-LTO, opt-``z``) codegen
    on every invalidated rebuild â€” opt level does not change the deterministic
    import outcome it is chasing.  DEFAULT UNCHANGED: when the knob is unset,
    acceptance / final-green still builds the shipped ``release-output`` runtime,
    which is the artifact parity is measured against (M05).  An invalid profile
    name is ignored so a typo cannot silently redirect the build; cargo would
    also reject a non-existent profile loudly.
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


def _runtime_wasm_incremental_enabled() -> bool:
    """Whether to build the runtime wasm into a stable, incremental target dir.

    The stable per-family target dir (``_runtime_wasm_incremental_target_root``)
    is session-independent, so a fresh session/worktree reuses already-compiled
    dependency crates instead of a cold recompile of the whole graph (V2 cold-dep
    burn-down).  ``CARGO_INCREMENTAL=1`` is turned on with it so consecutive
    same-family iterations recompile incrementally.

    Resolution (V2 doctrine "stable dep-cache default-ON for iteration contexts"):

    * ``MOLT_RUNTIME_WASM_INCREMENTAL`` explicitly set wins, both ways.
    * Otherwise DEFAULT ON in an explicit iteration context -- i.e. when
      ``MOLT_RUNTIME_BUILD_PROFILE`` pins a non-shipping profile (dev-fast) for
      the correctness loop -- so the iteration knob alone enables cross-session
      dep reuse (one knob, progressive disclosure).
    * Otherwise DEFAULT OFF: the shipped acceptance / final-green path
      (which never sets ``MOLT_RUNTIME_BUILD_PROFILE``) keeps the deterministic
      session-scoped target dir and publishes exact-identity artifacts to the
      shared cache (M05); incremental builds deliberately never publish.
    """
    raw = os.environ.get("MOLT_RUNTIME_WASM_INCREMENTAL", "").strip().lower()
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return bool(_runtime_build_profile_override())


def _runtime_wasm_incremental_family_key(
    *,
    cargo_profile: str,
    target_triple: str,
    features: tuple[str, ...],
    simd_enabled: bool,
    freestanding: bool,
) -> str:
    """Codegen-identity key for the incremental runtime-wasm target dir.

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


def _runtime_wasm_incremental_target_root(project_root: Path, family_key: str) -> Path:
    """Stable, session-independent cargo target dir for the runtime-wasm build.

    Session-independent by design: cross-iteration incremental reuse is the whole
    point (the per-session dir exists for agent isolation, but a fresh session id
    per proof-queue run means cargo incremental never engages â€” the M09 "stable
    target dir" lever).  Concurrency across sessions building the same family is
    made safe by cargo's own per-target build lock plus the ``_build_slot()``
    cross-process gate; two *divergent* source builds in one family serialise and
    may thrash each other's incremental state (slower, never incorrect), so this
    stays opt-in for the single-lane iteration loop.
    """
    override = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if override:
        base = Path(override).expanduser()
        if not base.is_absolute():
            base = (Path.cwd() / base).absolute()
    else:
        base = project_root / "target"
    return base / "runtime-wasm-incr" / family_key
