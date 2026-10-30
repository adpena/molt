#!/usr/bin/env python3
"""Deterministic, preemptive, AGENT-SAFE disk guard for the Molt artifact root.

WHY THIS EXISTS. On 2026-07-11 the canonical NVMe (``C:\\Molt``) filled to
**0 bytes** mid-session and failed several build lanes. The former generic
age/LRU janitor could sweep unrelated artifact classes without consumer
custody, so that authority is retired. This guard owns only its explicit,
consumer-aware build-artifact allow-set; it is not a replacement generic
``tmp`` sweeper.

This guard imports no process-control API, signals nothing, and kills nothing.
The coordination-owned evidence inspector queries Git's object store through
one batch-check stream; guard custody observes a read-only native snapshot,
and discovery otherwise reads filesystem metadata. The paired
``tests/tools/test_disk_guard.py::test_agent_safety_source_scan`` asserts this
module's source contains zero process-actuation tokens (the same discipline as
pact's ``test_no_actuation_capability``). ``MOLT_DISABLE_DISK_GUARD`` stops
only the daily collection that ``molt.dx`` starts; an explicit invocation runs.

DESIGN (deterministic + idempotent reclamation, separate from admission):

  * ``molt.disk_capacity`` owns synchronous build admission. This module owns
    explicit artifact reclamation only. Neither successful planning nor an
    unsuccessful reclaim authorizes a build; the build consumer measures its
    actual output filesystem again before launch. Cleanup failure is visible
    in the result, never converted into build permission.

  * ``ensure_free(min_gb)`` -- preemptive high-water reclaim. Reads free space on
    the artifact volume; if it is at/above the HIGH-WATER threshold (default
    ~25 GB) it is a fast no-op. Below it, the guard reclaims RECLAIMABLE dirs in
    age order (oldest first, largest tie-break) until free reaches the TARGET
    (default ~40 GB) or it runs out of eligible candidates or a time budget
    expires. It re-reads REAL free space after each reclaim (never trusts an
    estimate for the stop condition), so it is self-correcting.

  * The RECLAIMABLE set is EXPLICIT and narrow -- never "the largest dir under
    target" (that would nuke the shared ``target/debug`` / ``target/release`` /
    target-triple build that every lane shares). Only:
        - checkout state roots (``molt.checkout_state``), and pinned session
          targets ``<target>/sessions/<session>`` below a checkout target or
          ``<root>/target``; their last use decides when they retire;
        - ``<root>/target/<lane-glob>``         (``codex-*`` / ``lane-*`` / ``sess-*``);
        - Cargo-incremental quarantine dirs (``.molt_state/quarantine/...``);
        - flat legacy ``<root>/cargo-target-*``.

  * An ACTIVE directory is NEVER reclaimed. "Active" == modified within
    ``min_idle`` (a running build touches its target continuously), OR holds a
    Cargo or build-control lock, OR is the current ``MOLT_SESSION_ID`` target
    or ``CARGO_TARGET_DIR``, OR is a last-use root used within the protection
    window.

  * One invocation covers the canonical artifact root and every Git-registered
    worktree inside that scope. Registration is read directly from Git metadata
    without spawning a process. Nonterminal memory-guard markers protect the
    owning worktree, while unregistered sibling directories confer no deletion
    authority.

  * ``gc()`` / ``--gc`` -- retirement by last use (design 56, section 2.6):
    ``molt.checkout_state.checkout_state_decision`` selects checkout state
    past its max age or with a removed owner, and pinned session targets idle
    past their limit; the GC reclaims each one that no lock or protected path
    keeps, so no one ``rm``-s a finished session's target by hand. Inside the
    roots that stay, it retires the layout the product store replaced
    (``molt.checkout_state.superseded_layout_paths``, design 56 section 2.8)
    under the same custody.
    Lane-owned and pre-constructor scratch under ``<root>/tmp`` uses
    ``MOLT_SCRATCH_IDLE_DAYS`` (default three days) and the same soft
    retirement. A lane root's last-use stamp decides it without a walk; a
    pre-constructor root needs a complete walk, and complete walks resume
    across passes from a persisted cursor (HF-528). Existing control
    namespaces keep their rules; an open guard marker keeps only its own
    checkout and resource coordinates.

  * ``--retire-cargo-incremental`` is a separate explicit review operation for
    successful pinned-session profile caches. It is never discovered by GC or
    pressure reclaim; its own exact coordinate, terminal guard and inventory
    checks retain the rest of the profile. Plan first, then apply its digest.

Pure-stdlib (no third-party imports), so it runs under the host ``python`` a
Claude Code hook invokes as well as the molt venv. Windows-first (``msvcrt``
lock probe) with a POSIX (``fcntl``) path; explicit UTF-8 everywhere (M43).
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack
import hashlib
import re
import stat
import tomllib
import json
import os
import shutil
import sys
import tempfile
import time
from dataclasses import dataclass, field, replace
from functools import partial
from itertools import groupby
from pathlib import Path
from typing import Callable, Iterable, Literal, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.disk_capacity import (  # noqa: E402
    DISK_GUARD_HIGH_WATER_ENV,
    minimum_headroom_bytes,
)
from molt.custody_layout import (  # noqa: E402
    SCRATCH_DIRNAME,
    _read_git_path,
    custody_root,
    disk_scratch_roots_of,
    git_directories,
)
from molt.dx import (  # noqa: E402
    # noqa: E402,
    checkout_component,
    configured_artifact_root,
    session_artifact_component,
)
from molt.checkout_state import (  # noqa: E402
    PROTECTION_WINDOW_S as CHECKOUT_STATE_WINDOW_S,
    RetentionLimits,
    PROTECTED_TARGET_NAMES,
    RETIRED_DIR,
    RETIREMENT_RECORD_SUFFIX,
    RETIREMENT_WINDOW_S,
    agent_evidence_idle_s,
    bound_checkout_owner,
    checkout_target_of,
    checkout_state_containers,
    checkout_state_decision,
    checkout_state_kind_of,
    checkout_state_last_use,
    checkout_state_lock_paths,
    is_session_target,
    legacy_target_kind_of,
    newest_activity_mtime as _newest_activity_mtime,
    superseded_layout_paths,
    owner_is_absent,
    read_checkout_owner,
    read_soft_retirement,
    retention_limits,
    soft_retire,
    session_target_dirs,
    target_build_control_dirs,
    WalkIncomplete,
    WalkRotation,
)
from molt.file_deletion import delete_path  # noqa: E402
from molt.memory_guard_paths import active_guard_marker_dirs_of  # noqa: E402
from tools.memory_guard_core.active_custody import (  # noqa: E402
    ArtifactGuard,
    artifact_guard_records,
    has_active_guard_marker,
)

_GB = 1024**3

# --- thresholds (env-overridable; deterministic defaults) -------------------
DEFAULT_TARGET_GB = 40.0  # reclaim until free reaches this
DEFAULT_MIN_IDLE_MIN = 15.0  # never touch a dir modified within this
DEFAULT_ENSURE_BUDGET_S = 20.0  # wall-clock ceiling on one ensure_free reclaim
DEFAULT_LANE_GLOBS = ("codex-*", "lane-*", "sess-*")

# Env keys. Reclamation policy is independent of process custody.
ENV_TARGET = "MOLT_DISK_GUARD_TARGET_GB"
ENV_MIN_IDLE = "MOLT_DISK_GUARD_MIN_IDLE_MIN"
ENV_LANE_GLOBS = "MOLT_DISK_GUARD_LANE_GLOBS"

# The reclaimable classes (`_is_reclaimable`). The retention policy of
# `molt.checkout_state` judges the last-use classes by their last use, so
# pressure reclaim spares them within its protection window.
CandidateKind = Literal[
    "checkout-state", "session-target", "lane", "quarantine", "cargo-target", "scratch"
]
ReclaimOperation = Literal["gc", "pressure", "checkout-retirement"]
ReclaimPhase = Literal[
    "setup",
    "discovery",
    "planning",
    "retirement",
    "superseded-layouts",
    "evidence",
    "retired-roots",
    "scratch",
]
_LAST_USE_KINDS: frozenset[CandidateKind] = frozenset(
    {"checkout-state", "session-target", "lane", "cargo-target"}
)

_LEGACY_TARGET_KINDS: frozenset[CandidateKind] = frozenset(
    {"lane", "cargo-target", "quarantine"}
)


LOG_RELDIR = Path("logs") / "disk_guard"


def _eprint(*a: object) -> None:
    print(*a, file=sys.stderr, flush=True)


def _human(n: int) -> str:
    f = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if f < 1024 or unit == "TB":
            return f"{f:.1f}{unit}"
        f /= 1024
    return f"{f:.1f}TB"


def _env_float(env: Mapping[str, str], key: str, default: float) -> float:
    raw = str(env.get(key, "")).strip()
    if not raw:
        return default
    try:
        val = float(raw)
    except ValueError:
        return default
    return val if val > 0 else default


# --- config -----------------------------------------------------------------


@dataclass(frozen=True)
class GuardConfig:
    high_water_bytes: int
    target_bytes: int
    min_idle_s: float
    lane_globs: tuple[str, ...]

    @classmethod
    def from_env(
        cls,
        env: Mapping[str, str] | None = None,
        *,
        high_water_gb: float | None = None,
        target_gb: float | None = None,
    ) -> "GuardConfig":
        env = os.environ if env is None else env
        high_water_bytes = minimum_headroom_bytes(
            env
            if high_water_gb is None
            else {DISK_GUARD_HIGH_WATER_ENV: str(high_water_gb)}
        )
        tgt = (
            target_gb
            if target_gb is not None
            else _env_float(env, ENV_TARGET, DEFAULT_TARGET_GB)
        )
        # Target must be >= high-water or the loop can never satisfy itself.
        target_bytes = max(int(tgt * _GB), high_water_bytes)
        min_idle = _env_float(env, ENV_MIN_IDLE, DEFAULT_MIN_IDLE_MIN) * 60.0
        raw_globs = str(env.get(ENV_LANE_GLOBS, "")).strip()
        globs = (
            tuple(g.strip() for g in raw_globs.split(",") if g.strip())
            or DEFAULT_LANE_GLOBS
        )
        return cls(
            high_water_bytes=high_water_bytes,
            target_bytes=target_bytes,
            min_idle_s=min_idle,
            lane_globs=globs,
        )


# --- candidate model + PURE decision surface --------------------------------


@dataclass
class Candidate:
    """A reclaimable directory. All fields are facts, injected for testability."""

    path: Path
    kind: CandidateKind
    mtime: float  # newest activity signal (dir + immediate children)
    size: int = -1  # -1 == not measured
    lock_held: bool = False
    owner_root: Path | None = None
    guard_active: bool = False

    def age_s(self, now: float) -> float:
        return max(0.0, now - self.mtime)


@dataclass
class ReclaimPlan:
    triggered: bool
    free_bytes: int
    target_bytes: int
    selected: list[Candidate] = field(default_factory=list)
    skipped: list[tuple[Candidate, str]] = field(default_factory=list)

    @property
    def projected_free(self) -> int:
        return self.free_bytes + sum(max(0, c.size) for c in self.selected)


def _order_key(c: Candidate) -> tuple[float, int]:
    # Oldest first (LRU/age), largest as a tie-break so a reclaim frees the most
    # from the stalest dirs first. Deterministic total order.
    return (c.mtime, -max(0, c.size))


def _deletion_contains(path: Path, protected: Path) -> bool:
    """Whether deleting ``path`` would remove the protected path."""
    try:
        path = path.resolve(strict=False)
        protected = protected.resolve(strict=False)
    except OSError:
        return _norm(path) == _norm(protected)
    return path == protected or path in protected.parents


def _is_protected(path: Path, protected: Sequence[Path]) -> bool:
    return any(_deletion_contains(path, protected_path) for protected_path in protected)


def plan_reclaim(
    candidates: Iterable[Candidate],
    *,
    free_bytes: int,
    high_water_bytes: int,
    target_bytes: int,
    now: float,
    min_idle_s: float,
    protected: Iterable[Path] = (),
) -> ReclaimPlan:
    """PURE: decide which candidates to reclaim. No I/O, fully unit-testable.

    Fires only when ``free_bytes < high_water_bytes`` (idempotent no-op above the
    threshold). When it fires it selects candidates in age order until the
    PROJECTED free reaches ``target_bytes``, skipping any that are ACTIVE:
    modified within ``min_idle_s``, lock-held, protected, or a last-use root
    used within the protection window.
    The orchestrator re-reads REAL free space between deletes, so an inexact
    ``size`` only affects how many candidates are pre-selected, never safety.
    """
    protected_paths = tuple(Path(p) for p in protected)
    plan = ReclaimPlan(
        triggered=free_bytes < high_water_bytes,
        free_bytes=free_bytes,
        target_bytes=target_bytes,
    )
    if not plan.triggered:
        return plan

    running_free = free_bytes
    selected_paths: set[Path] = set()
    selected_bytes_below: dict[Path, int] = {}
    for c in sorted(candidates, key=_order_key):
        if _is_protected(c.path, protected_paths):
            plan.skipped.append((c, "protected"))
            continue
        if c.lock_held:
            plan.skipped.append((c, "lock-held"))
            continue
        if c.guard_active:
            plan.skipped.append((c, "active-guard"))
            continue
        if c.age_s(now) < min_idle_s:
            plan.skipped.append((c, f"modified <{min_idle_s / 60:.0f}min ago"))
            continue
        if c.kind in _LAST_USE_KINDS and c.age_s(now) < CHECKOUT_STATE_WINDOW_S:
            plan.skipped.append((c, "used within the protection window"))
            continue
        key = Path(_norm(c.path))
        if any(parent in selected_paths for parent in key.parents):
            plan.skipped.append((c, "covered by selected parent"))
            continue
        if running_free >= target_bytes:
            plan.skipped.append((c, "target already met"))
            continue
        plan.selected.append(c)
        selected_paths.add(key)
        # Replacing selected children with their enclosing root must not count
        # the same bytes twice, or stop a dry-run plan before its free target.
        delta = max(0, c.size) - selected_bytes_below.get(key, 0)
        running_free += delta
        for parent in key.parents:
            selected_bytes_below[parent] = selected_bytes_below.get(parent, 0) + delta
    plan.selected = _without_covered_candidates(plan.selected)
    return plan


def _without_covered_candidates(candidates: list[Candidate]) -> list[Candidate]:
    """Select each byte tree once when both a target and a session qualify."""
    paths = {Path(_norm(candidate.path)) for candidate in candidates}
    return [
        candidate
        for candidate in candidates
        if not any(parent in paths for parent in Path(_norm(candidate.path)).parents)
    ]


def decide_owner_reclaim(
    candidate: Candidate, *, protected: Iterable[Path] = ()
) -> tuple[bool, str]:
    """PURE: an owner's retirement skips the idle window, never lock or protection."""
    if candidate.lock_held:
        return False, "lock-held"
    if _is_protected(candidate.path, tuple(Path(path) for path in protected)):
        return False, "protected"
    if candidate.guard_active:
        return False, "active-guard"
    return True, "owner-retired"


def gc_plan(
    candidates: Iterable[Candidate],
    *,
    now: float,
    min_idle_s: float,
    protected: Iterable[Path] = (),
) -> list[Candidate]:
    """PURE: the retirement candidates that no live use keeps, oldest first.

    The retention policy (`checkout_state_decision`) selected each one by its
    last use; this keeps any that is protected, lock-held, or modified within
    ``min_idle_s``.
    """
    protected_paths = tuple(Path(p) for p in protected)
    out: list[Candidate] = []
    for c in sorted(candidates, key=_order_key):
        if _is_protected(c.path, protected_paths):
            continue
        if c.lock_held or c.guard_active:
            continue
        if c.age_s(now) < min_idle_s:
            continue
        out.append(c)
    return _without_covered_candidates(out)


def _norm(p: Path) -> str:
    try:
        return os.path.normcase(str(Path(p).resolve()))
    except OSError:
        return os.path.normcase(str(p))


# --- root resolution + hard safety rails ------------------------------------


def resolve_root(
    raw: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> Path:
    """Resolve the artifact root and REFUSE dangerous targets.

    Never operates on a drive/filesystem root or a one-component path. An
    explicit ``--root`` or ``MOLT_EXT_ROOT`` is required on every host and
    must identify a real, multi-component directory.
    """
    env = os.environ if env is None else env
    configured = configured_artifact_root(env, relative_to=Path.cwd())
    if raw:
        root = Path(raw)
    elif configured is not None:
        root = configured
    else:
        raise SystemExit(
            "disk_guard: could not resolve the artifact root; pass --root or set "
            "MOLT_EXT_ROOT to the intended artifact directory."
        )
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise SystemExit(f"disk_guard: root {root} is not a directory")
    if root.parent == root or len(root.parts) <= 1:
        raise SystemExit(f"disk_guard: refusing drive/filesystem root {root}")
    return root


def _path_within(path: Path, parent: Path) -> bool:
    try:
        return path == parent or parent in path.parents
    except OSError:
        return False


def _registered_worktrees_from_common(common: Path) -> set[Path]:
    roots: set[Path] = set()
    # A separate Git directory does not identify its main source checkout.
    if common.name == ".git":
        main = common.parent.resolve(strict=False)
        if git_directories(main) == (common, common):
            roots.add(main)
    for admin in _iter_dir(common / "worktrees"):
        if not admin.is_dir():
            continue
        try:
            # This reverse registration is an unprefixed path record, not a
            # checkout's `gitdir: ...` pointer. Share only its bounded decoding.
            raw = _read_git_path(admin / "gitdir")
            if not raw:
                continue
            gitfile = Path(raw)
            if not gitfile.is_absolute():
                gitfile = admin / gitfile
            gitfile = gitfile.resolve(strict=False)
            if gitfile.name != ".git":
                continue
            checkout = gitfile.parent
            directories = git_directories(checkout)
        except (OSError, ValueError, RuntimeError):
            continue
        if directories == (admin.resolve(strict=False), common):
            roots.add(checkout)
    return roots


def registered_worktree_roots(root: Path) -> tuple[Path, ...]:
    """Enumerate registered worktrees without spawning Git or any process.

    Only registrations inside the canonical artifact scope are returned. This
    prevents a stale registration outside ``C:\\Molt`` (or the corresponding
    cross-platform scope) from widening deletion authority.
    """
    root = root.resolve()
    scope = custody_root(root)
    probes: list[Path] = [root, scope, scope / "molt-src"]
    common_dirs = {
        directories[1]
        for probe in probes
        if (directories := git_directories(probe)) is not None
    }
    registered: set[Path] = set()
    for common in common_dirs:
        registered.update(_registered_worktrees_from_common(common))
    return tuple(
        sorted(
            (path for path in registered if _path_within(path, scope)),
            key=_norm,
        )
    )


def reclaim_roots(root: Path) -> tuple[Path, ...]:
    """All artifact roots governed by one canonical disk-guard invocation."""
    resolved = root.resolve()
    scope = custody_root(resolved)
    roots = {resolved, scope, *registered_worktree_roots(resolved)}
    return tuple(sorted((path for path in roots if path.is_dir()), key=_norm))


def _is_reclaimable(
    path: Path, root: Path, lane_globs: Sequence[str]
) -> CandidateKind | None:
    """Classify ``path`` as reclaimable, returning its kind, or None if not.

    This is the ALLOW-list authority: a path is reclaimable ONLY if it is one of
    the narrow classes below AND passes the hard denylist. Anything else --
    including the shared ``target/debug`` build and worktree checkouts under
    ``C:\\Molt\\wt-*`` -- returns None and is therefore never deleted.
    """
    try:
        rp = path.resolve()
    except OSError:
        return None
    root = root.resolve()
    if root not in rp.parents:
        return None
    if rp.name.casefold() in PROTECTED_TARGET_NAMES:
        return None
    parts = rp.relative_to(root).parts
    # A pinned session's target, below a checkout target or the root's own
    # target, and a checkout's state root in its managed container under a
    # component name (HF-313); `checkout_state_decision` decides when each
    # retires.
    if is_session_target(rp, root):
        return "session-target"
    if checkout_state_kind_of(rp, root) == "scratch":
        return "scratch"
    if checkout_state_kind_of(rp, root) is not None:
        return "checkout-state"
    # cargo-incremental quarantine anywhere under a target tree
    if ".molt_state" in parts and "quarantine" in parts:
        return "quarantine"
    return legacy_target_kind_of(rp, root, lane_globs)


def _target_root(owner_root: Path) -> Path:
    """The artifact target container, including persistent checkout targets."""
    return owner_root / "target"


def assert_safe_to_delete(path: Path, root: Path, lane_globs: Sequence[str]) -> None:
    """Raise unless ``path`` is provably a reclaimable artifact dir under root.

    The choke point for automatic and whole-root retirement. It fails CLOSED: any
    ambiguity -> refusal. This is what makes a mis-computed candidate list
    incapable of deleting the wrong thing.
    """
    rp = Path(path).resolve()
    if not rp.is_dir():
        raise ValueError(f"disk_guard: refuse to delete non-directory {rp}")
    root = Path(root).resolve()
    if rp == root or root not in rp.parents:
        raise ValueError(f"disk_guard: refuse to delete outside root: {rp}")
    if len(rp.parts) <= len(root.parts):
        raise ValueError(f"disk_guard: refuse to delete at/above root depth: {rp}")
    if _is_reclaimable(rp, root, lane_globs) is None:
        raise ValueError(f"disk_guard: {rp} is not in the reclaimable allow-set")


def _retire_candidate(
    candidate: Candidate,
    owner_root: Path,
    config: GuardConfig,
    *,
    scope_roots: Sequence[Path],
    now_fn: Callable[[], float],
    eligible: Callable[[Candidate, float], str | None],
    result: ReclaimResult,
) -> tuple[bool, str | None, str | None]:
    """Reobserve under exclusive mutation, then soft-retire the root.

    Ordinary cached candidates remain selection hints. Cooperating managed
    readers hold the same external lock; raw Cargo's profile locks are an
    additional conservative observation, not universal start serialization.
    """
    from molt.checkout_state import checkout_state_retirement

    with checkout_state_retirement(candidate.path) as exclusive:
        if not exclusive:
            return False, None, "state-in-use"
        try:
            assert_safe_to_delete(
                candidate.path,
                owner_root,
                config.lane_globs,
            )
        except ValueError as error:
            return False, str(error), None
        guards = _guard_protection(scope_roots)
        current = replace(
            candidate,
            mtime=(
                checkout_state_last_use(candidate.path)
                if candidate.kind in _LAST_USE_KINDS
                else _newest_activity_mtime(candidate.path)
            ),
            lock_held=_root_lock_held(candidate.path, scope_roots),
            guard_active=guards.protects(candidate.path, owner_root),
        )
        if current.guard_active:
            result.custody = guards.report()
        reason = eligible(current, now_fn())
        if reason is not None:
            return False, None, reason
        owner = read_checkout_owner(current.path)
        ok, error = _timed_retire(
            current.path,
            custody_root(owner_root),
            reason=result.operation,
            owner=str(owner.path if owner is not None else owner_root),
            now=now_fn(),
            result=result,
        )
        return ok, None if ok else error, None


# --- I/O helpers (stat / lock probe / size) ---------------------------------


# Locks a live user holds under a reclaimable root. Cargo locks each profile
# directory (`<target>/debug/.cargo-lock`, `<target>/<triple>/<profile>/...`),
# not the target root, and a pinned session builds below its checkout target;
# build control holds its build, diff and daemon-suite owner locks.
# RuntimeCargoPlan pins intermediates under .molt-toolchains/<digest>.
# Supported Cargo releases hold .cargo-lock or .cargo-build-lock there; a
# check can hold its intermediate lock without a final-output exclusive lock.
_LOCK_PATTERNS = (
    ".cargo-lock",
    "*/.cargo-lock",
    "*/*/.cargo-lock",
    "sessions/*/*/.cargo-lock",
    "sessions/*/*/*/.cargo-lock",
    ".molt-toolchains/*/*/.cargo-lock",
    ".molt-toolchains/*/*/*/.cargo-lock",
    "sessions/*/.molt-toolchains/*/*/.cargo-lock",
    "sessions/*/.molt-toolchains/*/*/*/.cargo-lock",
    ".cargo-build-lock",
    "*/.cargo-build-lock",
    "*/*/.cargo-build-lock",
    "sessions/*/*/.cargo-build-lock",
    "sessions/*/*/*/.cargo-build-lock",
    ".molt-toolchains/*/*/.cargo-build-lock",
    ".molt-toolchains/*/*/*/.cargo-build-lock",
    "sessions/*/.molt-toolchains/*/*/.cargo-build-lock",
    "sessions/*/.molt-toolchains/*/*/*/.cargo-build-lock",
    "diff_run.lock",
    "build_locks/*.lock",
    "backend_daemon/suite-leases/*/owner.lock",
    "backend_daemon/suite-leases/v3/*/owner.lock",
)


def _lock_held(path: Path) -> bool:
    """True if a process now HOLDS a build lock under ``path``.

    We probe by attempting a NON-BLOCKING lock ourselves; if the OS says the
    region is already locked, a live user owns the dir. This reads a lock -- it
    never touches any process. Absent/unlocked/unprobeable -> False (the idle
    gate and last use remain the primary liveness signal).
    """
    for pattern in _LOCK_PATTERNS:
        for lock in path.glob(pattern):
            if lock.is_file() and not lock.is_symlink() and _region_locked(lock):
                return True
    return False


def _root_lock_held(path: Path, scope_roots: Sequence[Path]) -> bool:
    """True if a live user holds a lock under ``path`` or in its build control.

    A Cargo target's builds also lock the build control that it and each of
    its pinned sessions own, which lives outside the target.
    """
    return _lock_held(path) or any(
        _lock_held(control) for control in target_build_control_dirs(path, scope_roots)
    )


def _region_locked(lock: Path) -> bool:
    if not lock.is_file() or lock.is_symlink():
        return False
    try:
        handle = open(lock, "rb")
    except OSError:
        return True  # Unreadable custody cannot authorize retirement.
    try:
        if os.name == "nt":
            import msvcrt

            try:
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                return True  # already locked by a live build
            else:
                # We got it -> nobody held it. Release immediately.
                try:
                    handle.seek(0)
                    msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
                except OSError:
                    pass
                return False
        else:
            import fcntl

            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                return True
            else:
                try:
                    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
                except OSError:
                    pass
                return False
    finally:
        try:
            handle.close()
        except OSError:
            pass


def _dir_size(path: Path, deadline: float) -> int:
    """Best-effort recursive size; bail at ``deadline`` (returns partial)."""
    total = 0
    stack = [path]
    while stack:
        if time.monotonic() > deadline:
            return total
        current = stack.pop()
        try:
            with os.scandir(current) as it:
                for entry in it:
                    try:
                        if entry.is_symlink():
                            continue
                        if entry.is_dir(follow_symlinks=False):
                            stack.append(Path(entry.path))
                        else:
                            total += entry.stat(follow_symlinks=False).st_size
                    except OSError:
                        continue
        except OSError:
            continue
    return total


# --- discovery --------------------------------------------------------------


@dataclass(frozen=True)
class _GuardScope:
    guard: ArtifactGuard
    checkout: Path
    paths: tuple[Path, ...]

    @classmethod
    def of(cls, guard: ArtifactGuard, namespace_owner: Path) -> _GuardScope:
        scope = guard.record.artifact_scope
        return cls(
            guard,
            Path(_norm(scope.checkout or namespace_owner)),
            tuple(Path(_norm(path)) for path in scope.paths),
        )

    @property
    def coordinate_free(self) -> bool:
        return not self.guard.record.artifact_scope.paths


@dataclass
class _GuardProtection:
    scopes: tuple[_GuardScope, ...] = ()
    _implied: dict[tuple[Path, Path], tuple[Path, Path]] = field(default_factory=dict)

    def implied_roots(self, root: Path, checkout: Path) -> tuple[Path, Path]:
        from molt.checkout_state import scratch_state_root
        from molt.dx import cargo_target_dir_for_artifact_root

        key = root, checkout
        if key not in self._implied:
            self._implied[key] = (
                cargo_target_dir_for_artifact_root(root, None, source_root=checkout),
                scratch_state_root(
                    disk_scratch_roots_of(root)[0], owner=checkout, create=False
                ),
            )
        return self._implied[key]

    def protects(
        self, path: Path, owner_root: Path, *, recorded_owner: Path | None = None
    ) -> bool:
        """Deletion containment plus bound ownership, never root-wide liveness.

        A target coordinate keeps its enclosing target, not idle sibling
        sessions below it. Only a coordinate-free record needs the more
        conservative whole-owner rule, including that owner's sessions.
        """
        if not self.scopes:
            return False
        path = Path(_norm(path))
        session = is_session_target(path, owner_root)
        owner = bound_checkout_owner(
            path.parent.parent if session else path, owner_root
        )
        owned = owner is not None or recorded_owner is not None
        checkout = recorded_owner
        used_target = (
            owner.path
            if owner is not None and owner.kind == "build-control"
            else recorded_owner
            if checkout_state_kind_of(path, owner_root) == "build-control"
            else None
        )
        if used_target is not None:
            session = is_session_target(used_target, owner_root)
            target_owner = bound_checkout_owner(
                used_target.parent.parent if session else used_target, owner_root
            )
            checkout = None if target_owner is None else target_owner.path
        elif owner is not None:
            checkout = owner.path
        for scope in self.scopes:
            coordinates = (*scope.paths, scope.guard.record.path)
            if any(path == used or path in used.parents for used in coordinates):
                return True
            if used_target is not None and any(
                used_target == used or used_target in used.parents
                for used in scope.paths
            ):
                return True
            if session and not scope.coordinate_free:
                continue
            if checkout is not None:
                if checkout == scope.checkout or any(
                    checkout == used or checkout in used.parents for used in scope.paths
                ):
                    return True
                continue
            # Pre-owner-record targets/scratch still have the canonical
            # checkout address. Do not infer ownership from arbitrary names.
            target, scratch = self.implied_roots(owner_root, scope.checkout)
            if path in {target, scratch} or used_target == target:
                return True
            if (
                session
                and scope.coordinate_free
                and (
                    path.parent.parent == target
                    or (used_target is not None and used_target.parent.parent == target)
                )
            ):
                return True
            # With no coordinates and no attributable checkout, retain only
            # unowned state in the marker's own namespace. Bound foreign
            # checkout/session state above never enters this branch.
            if not owned and scope.coordinate_free and scope.checkout == owner_root:
                return True
        return False

    def report(self) -> list[dict]:
        directories: dict[Path, list[_GuardScope]] = {}
        for scope in self.scopes:
            directories.setdefault(scope.guard.active_dir, []).append(scope)
        return [
            {
                "active_dir": str(directory),
                "blocking_records": len(scopes),
                "examples": [
                    f"{scope.guard.record.path} ({scope.guard.reason})"
                    for scope in scopes[:5]
                ],
                "records": [
                    {
                        "marker": str(scope.guard.record.path),
                        "state": scope.guard.state,
                        "reason": scope.guard.reason,
                        "checkout": str(scope.checkout),
                        "owner_source": "marker"
                        if scope.guard.record.artifact_scope.checkout
                        else "namespace",
                        "paths": [str(path) for path in scope.paths],
                        "coordinate_free": scope.coordinate_free,
                        "operator_action_required": scope.guard.state == "ambiguous",
                        "release_command": scope.guard.release_command,
                    }
                    for scope in scopes
                ],
                "next_step": scopes[0].guard.release_command,
            }
            for directory, scopes in sorted(
                directories.items(), key=lambda item: str(item[0])
            )
        ]


def _guard_protection(roots: Iterable[Path]) -> _GuardProtection:
    owners = {
        markers: root for root in roots for markers in active_guard_marker_dirs_of(root)
    }
    scopes = []
    for guard in artifact_guard_records(tuple(owners)):
        namespace = owners[guard.active_dir]
        scopes.append(_GuardScope.of(guard, namespace))
    return _GuardProtection(tuple(scopes))


def _include_guard_custody(
    result: ReclaimResult, guards: Sequence[ArtifactGuard], namespace_owner: Path
) -> None:
    """Report nested marker namespaces found by the existing scratch walk."""
    if guards:
        reports = {item["active_dir"]: item for item in result.custody}
        projection = _GuardProtection(
            tuple(_GuardScope.of(guard, namespace_owner) for guard in guards)
        )
        reports.update({item["active_dir"]: item for item in projection.report()})
        result.custody = list(reports.values())


def discover_candidates(
    root: Path,
    config: GuardConfig,
    *,
    roots: Sequence[Path] | None = None,
    measure_sizes: bool = True,
    size_budget_s: float = 30.0,
    guards: _GuardProtection | None = None,
) -> list[Candidate]:
    """Scan every registered worktree's allow-set into one candidate family."""
    root = root.resolve()
    scan_roots = tuple(path.resolve() for path in (roots or reclaim_roots(root)))
    seen: set[str] = set()
    candidates: list[Candidate] = []
    size_deadline = time.monotonic() + size_budget_s
    guards = _guard_protection(scan_roots) if guards is None else guards

    def add(path: Path, kind: CandidateKind, owner_root: Path) -> None:
        norm = _norm(path)
        if norm in seen or not path.is_dir():
            return
        if _is_reclaimable(path, owner_root, config.lane_globs) is None:
            return
        seen.add(norm)
        size = -1
        if measure_sizes:
            size = _dir_size(path, min(time.monotonic() + 20.0, size_deadline))
        candidates.append(
            Candidate(
                path=path,
                kind=kind,
                # A last-use root's stamp is one of these children.
                mtime=_newest_activity_mtime(path),
                size=size,
                lock_held=_root_lock_held(path, scan_roots),
                owner_root=owner_root,
                guard_active=guards.protects(path, owner_root),
            )
        )

    for owner_root in scan_roots:
        # pinned session targets, evicted by last use under pressure
        for child in session_target_dirs(owner_root):
            add(child, "session-target", owner_root)
        # per-lane target dirs under target/
        for child in _iter_dir(_target_root(owner_root)):
            if (
                child.is_dir()
                and _is_reclaimable(child, owner_root, config.lane_globs) == "lane"
            ):
                add(child, "lane", owner_root)
        # cargo-incremental quarantine dirs
        for qdir in _iter_quarantine_dirs(owner_root):
            add(qdir, "quarantine", owner_root)
        # flat legacy cargo-target-*
        for child in _iter_dir(owner_root):
            if child.is_dir() and child.name.startswith("cargo-target-"):
                add(child, "cargo-target", owner_root)
        # checkout state roots, evicted by last use under pressure
        for child in _checkout_state_dirs(owner_root):
            add(child, "checkout-state", owner_root)
    return candidates


def _iter_dir(path: Path) -> list[Path]:
    try:
        return sorted(path.iterdir())
    except OSError:
        return []


def _checkout_state_dirs(root: Path) -> list[Path]:
    """Checkout state roots in their managed containers (HF-313)."""
    found = []
    for container in checkout_state_containers(root):
        if container == root / SCRATCH_DIRNAME:
            continue  # The scratch collector owns this inventory, once.
        try:
            with os.scandir(container) as entries:
                children = [
                    Path(entry.path)
                    for entry in entries
                    if entry.is_dir(follow_symlinks=False)
                ]
        except OSError:
            continue
        found.extend(
            child
            for child in sorted(children)
            if checkout_state_kind_of(child, root) is not None
        )
    return found


def _iter_quarantine_dirs(root: Path) -> list[Path]:
    out: list[Path] = []
    target = _target_root(root)
    for state_parent in (target, *[p for p in _iter_dir(target) if p.is_dir()]):
        qroot = state_parent / ".molt_state" / "quarantine" / "cargo_incremental"
        if qroot.is_dir():
            for child in _iter_dir(qroot):
                if child.is_dir():
                    out.append(child)
    return out


# --- reclamation orchestrator -----------------------------------------------


@dataclass
class ReclaimResult:
    operation: ReclaimOperation
    root: str
    triggered: bool
    free_before: int
    free_after: int
    scope_roots: list[str] = field(default_factory=list)
    reclaimed: list[dict] = field(default_factory=list)
    skipped: list[dict] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    mode: str = "apply"
    # Active guard records that blocked a candidate, with the next step.
    custody: list[dict] = field(default_factory=list)
    # Monotonic collection time excludes result-log publication. Phase totals
    # include their existing eligibility checks; deletion time is nested.
    elapsed_s: float = 0.0
    phase_elapsed_s: dict[ReclaimPhase, float] = field(default_factory=dict)
    # Sum repeated attempts at the same exact path without another inventory.
    deletion_elapsed_s: dict[str, float] = field(default_factory=dict)
    retirement_elapsed_s: dict[str, float] = field(default_factory=dict)
    retirements: dict[str, str] = field(default_factory=dict)
    # Per walk family: where its complete walks started and resume.
    walk_rotation: dict[str, dict] = field(default_factory=dict)

    @property
    def reclaimed_bytes(self) -> int:
        return self.free_after - self.free_before

    def to_dict(self) -> dict:
        return {
            "schema_version": 2,
            "tool": "disk_guard",
            "operation": self.operation,
            "elapsed_s": self.elapsed_s,
            "phase_elapsed_s": self.phase_elapsed_s,
            "deletion_elapsed_s": self.deletion_elapsed_s,
            "retirement_elapsed_s": self.retirement_elapsed_s,
            "retirements": self.retirements,
            "walk_rotation": self.walk_rotation,
            "root": self.root,
            "scope_roots": self.scope_roots,
            "mode": self.mode,
            "triggered": self.triggered,
            "free_before": self.free_before,
            "free_after": self.free_after,
            "reclaimed_bytes": self.reclaimed_bytes,
            "reclaimed": self.reclaimed,
            "skipped": self.skipped,
            "errors": self.errors,
            "custody": self.custody,
        }


def _default_free_fn(root: Path) -> Callable[[], int]:
    return lambda: shutil.disk_usage(root).free


def _elapsed_since(started: float) -> float:
    return max(0.0, time.monotonic() - started)


def _timed_delete(
    path: Path, *, owning_root: Path, result: ReclaimResult
) -> tuple[bool, str | None]:
    """Measure the existing deletion primitive without changing its custody."""
    started = time.monotonic()
    try:
        return delete_path(path, owning_root=owning_root)
    finally:
        key = str(path)
        result.deletion_elapsed_s[key] = result.deletion_elapsed_s.get(
            key, 0.0
        ) + _elapsed_since(started)


def _timed_retire(
    path: Path,
    owning_root: Path,
    *,
    reason: str,
    owner: str,
    now: float,
    result: ReclaimResult,
) -> tuple[bool, str | None]:
    started = time.monotonic()
    try:
        retired = soft_retire(path, owning_root, reason=reason, owner=owner, now=now)
        result.retirements[str(path)] = str(retired.path)
        return True, None
    except (OSError, ValueError) as exc:
        return False, str(exc)
    finally:
        key = str(path)
        result.retirement_elapsed_s[key] = result.retirement_elapsed_s.get(
            key, 0.0
        ) + _elapsed_since(started)


def ensure_free(
    min_gb: float | None = None,
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    config: GuardConfig | None = None,
    free_bytes_fn: Callable[[], int] | None = None,
    now_fn: Callable[[], float] = time.time,
    apply: bool = True,
    budget_s: float = DEFAULT_ENSURE_BUDGET_S,
    log: bool = True,
) -> ReclaimResult:
    """Preemptive high-water reclaim: keep free space above the threshold.

    Fires only when free < high-water (a fast no-op otherwise, so calling it
    before every build is cheap and idempotent). Reclaims RECLAIMABLE dirs in
    age order until REAL free space (re-read after each delete) reaches the
    target, eligible candidates run out, or ``budget_s`` elapses. When
    ``min_gb`` is given the effective target is ``max(target, min_gb)`` and the
    trigger is ``max(high_water, min_gb)`` -- a caller can demand more headroom.

    Deterministic and idempotent. Does NOT raise on a per-candidate delete
    error (recorded in ``errors``). This result describes reclamation only;
    build admission belongs to ``molt.disk_capacity.require_build_capacity``.
    """
    started = time.monotonic()
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
    scope_roots = reclaim_roots(resolved_root)
    cfg = config or GuardConfig.from_env(env)
    high_water = cfg.high_water_bytes
    target = cfg.target_bytes
    if min_gb is not None:
        floor = int(min_gb * _GB)
        high_water = max(high_water, floor)
        target = max(target, floor)
    free_fn = free_bytes_fn or _default_free_fn(resolved_root)
    now = now_fn()

    free_before = free_fn()
    result = ReclaimResult(
        operation="pressure",
        root=str(resolved_root),
        triggered=free_before < high_water,
        free_before=free_before,
        free_after=free_before,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    result.phase_elapsed_s["setup"] = _elapsed_since(started)
    if not result.triggered:
        result.elapsed_s = _elapsed_since(started)
        if log:
            _write_log(resolved_root, result)
        return result

    phase_started = time.monotonic()
    guards = _guard_protection(scope_roots)
    candidates = discover_candidates(
        resolved_root, cfg, roots=scope_roots, guards=guards
    )
    result.phase_elapsed_s["discovery"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    plan = plan_reclaim(
        candidates,
        free_bytes=free_before,
        high_water_bytes=high_water,
        target_bytes=target,
        now=now,
        min_idle_s=cfg.min_idle_s,
        protected=_protected_paths(scope_roots, env),
    )
    for cand, why in plan.skipped:
        result.skipped.append(
            {"path": str(cand.path), "reason": why, "kind": cand.kind}
        )
    result.custody = guards.report()

    result.phase_elapsed_s["planning"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    deadline = time.monotonic() + budget_s
    free_now = free_before
    for cand in plan.selected:
        if free_now >= target:
            break
        if time.monotonic() > deadline:
            result.errors.append(f"time budget {budget_s}s exceeded; stopping")
            break
        if apply:
            if not cand.path.exists():
                continue  # A session target went with its checkout target.
            owner_root = cand.owner_root or resolved_root

            def still_pressure_eligible(
                current: Candidate, current_now: float
            ) -> str | None:
                checked = plan_reclaim(
                    [current],
                    free_bytes=0,
                    high_water_bytes=1,
                    target_bytes=1,
                    now=current_now,
                    min_idle_s=cfg.min_idle_s,
                    protected=_protected_paths(scope_roots, env),
                )
                return None if checked.selected else checked.skipped[0][1]

            ok, error, skipped = _retire_candidate(
                cand,
                owner_root,
                cfg,
                scope_roots=scope_roots,
                now_fn=now_fn,
                eligible=still_pressure_eligible,
                result=result,
            )
            if skipped is not None:
                result.skipped.append({"path": str(cand.path), "reason": skipped})
            if error is not None:
                result.errors.append(f"{cand.path}: {error}")
            if not ok:
                continue
        result.reclaimed.append(
            {"path": str(cand.path), "kind": cand.kind, "est_bytes": max(0, cand.size)}
        )
        free_now = free_fn() if apply else free_before
    result.phase_elapsed_s["retirement"] = _elapsed_since(phase_started)
    result.free_after = free_fn() if apply else free_now
    result.elapsed_s = _elapsed_since(started)
    if log:
        _write_log(resolved_root, result)
    return result


def gc(
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    config: GuardConfig | None = None,
    now_fn: Callable[[], float] = time.time,
    apply: bool = True,
    log: bool = True,
) -> ReclaimResult:
    """Retire every root that the retention policy selects (design 56, 2.6).

    `checkout_state_decision` selects checkout state and scratch past their
    idle limits or with a removed owner, and idle pinned session targets; a held
    lock or a protected path keeps a root. A second pass retires the build
    control of the targets this pass reclaimed.
    """
    started = time.monotonic()
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
    scope_roots = reclaim_roots(resolved_root)
    cfg = config or GuardConfig.from_env(env)
    now = now_fn()
    limits = retention_limits(env)
    agent_evidence_idle_s(env)
    free_fn = _default_free_fn(resolved_root)
    free_before = free_fn()
    result = ReclaimResult(
        operation="gc",
        root=str(resolved_root),
        triggered=True,
        free_before=free_before,
        free_after=free_before,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    result.phase_elapsed_s["setup"] = _elapsed_since(started)
    guards = _guard_protection(scope_roots)
    result.custody = guards.report()
    # Byte estimates share one 30-second deadline; a partial size decides
    # nothing.
    size_deadline = time.monotonic() + 30.0
    # Each family of complete walks (scratch, retired scratch, agent
    # evidence, integration receipts) has its own walk budget and resumes
    # where the last applied pass stopped (HF-528).
    walks = WalkRotation.load(resolved_root)
    phase_started = time.monotonic()
    _collect_retired_roots(
        scope_roots,
        result,
        env=env,
        now=now,
        apply=apply,
        walks=walks,
        guards=guards,
    )
    result.phase_elapsed_s["retired-roots"] = _elapsed_since(phase_started)
    protected = _protected_paths(scope_roots, env)
    phase_started = time.monotonic()
    _collect_evidence(
        scope_roots,
        result,
        env=env,
        now=now,
        apply=apply,
        walks=walks,
        protected=protected,
    )
    result.phase_elapsed_s["evidence"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    _collect_scratch(
        scope_roots,
        result,
        env=env,
        now_fn=now_fn,
        limits=limits,
        apply=apply,
        walks=walks,
        guards=guards,
    )
    result.walk_rotation = walks.report()
    if apply:
        try:
            walks.publish()
        except (OSError, ValueError) as error:
            # The previous cursor stays; the next pass repeats this window.
            result.errors.append(f"{walks.record}: {error}")
    result.phase_elapsed_s["scratch"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    candidates = [
        cand
        for owner_root in scope_roots
        for cand in _retention_candidates(
            owner_root, scope_roots, now=now, limits=limits, config=cfg, guards=guards
        )
    ]
    result.phase_elapsed_s["discovery"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    collectable = gc_plan(
        candidates,
        now=now,
        min_idle_s=cfg.min_idle_s,
        protected=protected,
    )
    for cand in candidates:
        if cand.guard_active:
            result.skipped.append(
                {"path": str(cand.path), "kind": cand.kind, "reason": "active-guard"}
            )
    result.phase_elapsed_s["planning"] = _elapsed_since(phase_started)
    collected_norms: set[str] = set()

    def reclaim(planned: list[Candidate]) -> None:
        for cand in planned:
            size = _dir_size(cand.path, size_deadline)
            if apply:
                if not cand.path.exists():
                    continue  # A session target went with its checkout target.
                owner_root = cand.owner_root or resolved_root

                def still_gc_eligible(
                    current: Candidate, current_now: float
                ) -> str | None:
                    if (
                        current.kind in _LAST_USE_KINDS
                        and checkout_state_decision(
                            current.path,
                            owner_root,
                            now=current_now,
                            limits=limits,
                        )
                        is None
                    ):
                        return "checkout-state-policy-changed"
                    return (
                        None
                        if gc_plan(
                            [current],
                            now=current_now,
                            min_idle_s=cfg.min_idle_s,
                            protected=_protected_paths(scope_roots, env),
                        )
                        else "activity-or-protection-changed"
                    )

                ok, error, skipped = _retire_candidate(
                    cand,
                    owner_root,
                    cfg,
                    scope_roots=scope_roots,
                    now_fn=now_fn,
                    eligible=still_gc_eligible,
                    result=result,
                )
                if skipped is not None:
                    result.skipped.append({"path": str(cand.path), "reason": skipped})
                if error is not None:
                    result.errors.append(f"{cand.path}: {error}")
                if not ok:
                    continue
            collected_norms.add(_norm(cand.path))
            result.reclaimed.append(
                {"path": str(cand.path), "kind": cand.kind, "est_bytes": size}
            )

    phase_started = time.monotonic()
    reclaim(collectable)
    # A build-control root's owner is its Cargo target, so it is orphaned only
    # once that target is gone: a second pass retires the build control of
    # the targets this GC reclaimed.
    orphaning = sorted(
        {
            cand.owner_root
            for cand in collectable
            if cand.owner_root is not None and not cand.path.exists()
        },
        key=_norm,
    )
    if apply and orphaning:
        reclaim(
            [
                cand
                for cand in gc_plan(
                    [
                        cand
                        for owner_root in orphaning
                        for cand in _retention_candidates(
                            owner_root,
                            scope_roots,
                            now=now,
                            limits=limits,
                            build_control=True,
                            config=cfg,
                            guards=guards,
                        )
                    ],
                    now=now,
                    min_idle_s=cfg.min_idle_s,
                    protected=protected,
                )
                if _norm(cand.path) not in collected_norms
            ]
        )
    result.phase_elapsed_s["retirement"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    for owner_root in scope_roots:
        _retire_superseded_layouts(
            owner_root,
            scope_roots,
            result,
            config=cfg,
            now=now,
            apply=apply,
            retired=collected_norms,
            protected=protected,
            deadline=size_deadline,
            guards=guards,
        )
    result.phase_elapsed_s["superseded-layouts"] = _elapsed_since(phase_started)
    result.free_after = free_fn()
    result.elapsed_s = _elapsed_since(started)
    if log:
        _write_log(resolved_root, result)
    return result


@dataclass(frozen=True)
class _ScratchInventory:
    size: int
    mtime: float
    lock_held: bool
    guards: tuple[ArtifactGuard, ...]


def _scratch_inventory(path: Path, deadline: float) -> _ScratchInventory:
    """Complete recursive activity, size and lock observation in one walk.

    Old scratch has no cooperating stamp, so immediate children cannot prove
    idleness. Never use a partial walk to authorize its retirement. Payload
    bytes are not read; symlinks are observed but not followed.
    """
    from molt.memory_guard_paths import STATE_DIRNAME

    newest = path.lstat().st_mtime
    size = 0
    locked = False
    markers: list[Path] = []
    stack = [path]
    while stack:
        if time.monotonic() >= deadline:
            raise TimeoutError("scratch inventory budget exhausted")
        directory = stack.pop()
        if directory.name == "active" and directory.parent.name == STATE_DIRNAME:
            markers.append(directory)
        with os.scandir(directory) as entries:
            for entry in entries:
                if time.monotonic() >= deadline:
                    raise TimeoutError("scratch inventory budget exhausted")
                metadata = entry.stat(follow_symlinks=False)
                newest = max(newest, metadata.st_mtime)
                child = Path(entry.path)
                if stat.S_ISDIR(metadata.st_mode):
                    stack.append(child)
                elif stat.S_ISREG(metadata.st_mode):
                    size += metadata.st_size
                    if entry.name.endswith(".lock") or entry.name in {
                        ".cargo-lock",
                        ".cargo-build-lock",
                        "lock",
                    }:
                        locked |= _region_locked(child)
                elif not stat.S_ISLNK(metadata.st_mode):
                    # Sockets/devices have a different lifetime protocol.
                    locked = True
    return _ScratchInventory(size, newest, locked, artifact_guard_records(markers))


def _collect_scratch(
    scope_roots: Sequence[Path],
    result: ReclaimResult,
    *,
    env: Mapping[str, str],
    now_fn: Callable[[], float],
    limits: RetentionLimits,
    apply: bool,
    walks: WalkRotation,
    guards: _GuardProtection,
) -> None:
    """Retire idle scratch: stamped roots by their stamp, legacy roots by a walk.

    A root whose owner record binds and whose last-use stamp is a regular
    file follows the HF-435 use protocol: every managed use holds its
    external use lock for its lifetime and refreshes the stamp when it
    starts and ends (at most once an hour). Its stamp decides it, as Go and
    Cargo trust a recorded last use: a fixed number of metadata calls, no
    walk and no byte estimate. A legacy root has no cooperating writer, so
    only a complete walk (`molt.checkout_state.WalkRotation`) can prove it idle and size it.
    """
    from molt.checkout_state import (
        LAST_USE_STAMP,
        OWNER_RECORD,
        checkout_state_retirement,
    )
    from molt.file_publication import resolve_owned_path

    protected = _protected_paths(scope_roots, env)
    children: dict[str, tuple[Path, Path]] = {}
    for root in scope_roots:
        container = disk_scratch_roots_of(root)[0]
        if container.is_symlink():
            continue
        try:
            with os.scandir(container) as entries:
                children.update(
                    (entry.path, (root, Path(entry.path)))
                    for entry in entries
                    if entry.is_dir(follow_symlinks=False)
                )
        except OSError:
            continue
    for key in walks.order("scratch", children):
        root, path = children[key]
        if (
            path.name.casefold() in PROTECTED_TARGET_NAMES
            or checkout_state_kind_of(path, root) != "scratch"
        ):
            continue

        def observe(*, recheck: bool = False) -> tuple[int | None, str | None]:
            resolve_owned_path(path)
            owner = read_checkout_owner(path)
            if os.path.lexists(path / OWNER_RECORD) and owner is None:
                return None, "invalid-owner"
            if owner is not None and owner.kind != "scratch":
                return None, "other-state-owner"
            if _is_protected(
                path, _protected_paths(scope_roots, env) if recheck else protected
            ):
                return None, "protected"
            protection = _guard_protection(scope_roots) if recheck else guards
            if protection.protects(path, root):
                if recheck:
                    result.custody = protection.report()
                return None, "active-guard"
            if not recheck and any(
                _region_locked(lock) for lock in checkout_state_lock_paths(path)
            ):
                return None, "state-in-use"
            # Constant work: even an immediate-child listing can be huge.
            newest = path.lstat().st_mtime
            stamped = False
            try:
                stamp = (path / LAST_USE_STAMP).lstat()
            except FileNotFoundError:
                pass
            else:
                newest = max(newest, stamp.st_mtime)
                stamped = (
                    owner is not None
                    and stat.S_ISREG(stamp.st_mode)
                    and bound_checkout_owner(path, root) == owner
                )
            if (
                checkout_state_decision(
                    path, root, now=now_fn(), limits=limits, last_use=newest
                )
                is None
            ):
                return None, "recent-use"
            if stamped:
                return None, None
            inventory = walks.spend("scratch", key, partial(_scratch_inventory, path))
            size, newest = inventory.size, inventory.mtime
            _include_guard_custody(result, inventory.guards, path)
            if (
                inventory.lock_held
                or inventory.guards
                or _root_lock_held(path, scope_roots)
            ):
                return size, "lock-or-guard-held"
            if (
                checkout_state_decision(
                    path, root, now=now_fn(), limits=limits, last_use=newest
                )
                is None
            ):
                return size, "recent-use"
            return size, None

        try:
            size, kept = observe()
            if kept is None and apply:
                with checkout_state_retirement(path) as exclusive:
                    if not exclusive:
                        kept = "state-in-use"
                    else:
                        assert_safe_to_delete(path, root, ())
                        size, kept = observe(recheck=True)
                        if kept is None:
                            owner = bound_checkout_owner(path, root)
                            ok, error = _timed_retire(
                                path,
                                custody_root(root),
                                reason="scratch",
                                owner=str(owner.path if owner else root),
                                now=now_fn(),
                                result=result,
                            )
                            if not ok:
                                result.errors.append(f"{path}: {error}")
                                continue
        except WalkIncomplete as incomplete:
            size, kept = None, incomplete.reason
        except (OSError, ValueError) as error:
            result.skipped.append({"path": str(path), "reason": str(error)})
            continue
        if kept is not None:
            result.skipped.append(
                {"path": str(path), "kind": "scratch", "reason": kept}
            )
        else:
            # A stamped root is retired without a walk, so its bytes are
            # unknown, as for an expired retirement's final deletion.
            result.reclaimed.append(
                {"path": str(path), "kind": "scratch"}
                | ({} if size is None else {"est_bytes": size})
            )


def _collect_retired_roots(
    scope_roots: Sequence[Path],
    result: ReclaimResult,
    *,
    env: Mapping[str, str],
    now: float,
    apply: bool,
    walks: WalkRotation,
    guards: _GuardProtection,
) -> None:
    """Only a validated retirement's clock can authorize final deletion."""
    from molt.checkout_state import checkout_state_retirement
    from molt.file_locks import _try_acquire_file_lock, _release_file_lock

    records: dict[str, tuple[Path, Path]] = {}
    for owning_root in {custody_root(root) for root in scope_roots}:
        retired_dir = owning_root / RETIRED_DIR
        if retired_dir.is_symlink():
            continue
        records.update(
            (str(record), (owning_root, record))
            for date in _iter_dir(retired_dir)
            if not date.is_symlink() and date.is_dir()
            for record in date.glob("*" + RETIREMENT_RECORD_SUFFIX)
        )
    for key in walks.order("retired-scratch", records):
        owning_root, record = records[key]
        retirement = read_soft_retirement(record, owning_root)
        if retirement is None or now - retirement.retired_at < RETIREMENT_WINDOW_S:
            continue

        def kept(
            *,
            integration_lock_owned: bool = False,
            state_lock_owned: bool = False,
        ) -> str | None:
            protected = _protected_paths(scope_roots, env)
            if retirement.reason == "scratch":
                try:
                    inventory = walks.spend(
                        "retired-scratch",
                        key,
                        partial(_scratch_inventory, retirement.path),
                    )
                    _include_guard_custody(result, inventory.guards, retirement.path)
                except WalkIncomplete as incomplete:
                    return incomplete.reason
                except (OSError, ValueError):
                    return "lock-or-protection"
                if (
                    inventory.lock_held
                    or inventory.guards
                    or now - inventory.mtime < CHECKOUT_STATE_WINDOW_S
                    or (
                        not state_lock_owned
                        and any(
                            _region_locked(lock)
                            for lock in checkout_state_lock_paths(retirement.original)
                        )
                    )
                ):
                    return "lock-or-protection"
            held = (
                (
                    guards if not state_lock_owned else _guard_protection(scope_roots)
                ).protects(
                    retirement.original,
                    owning_root,
                    recorded_owner=Path(retirement.owner),
                )
                or _is_protected(retirement.path, protected)
                or _is_protected(retirement.original, protected)
                or _root_lock_held(retirement.path, scope_roots)
                or _root_lock_held(retirement.original, scope_roots)
                or (
                    retirement.reason == "integration-receipt"
                    and not integration_lock_owned
                    and _region_locked(Path(retirement.owner) / "molt-integrate.lock")
                )
            )
            return "lock-or-protection" if held else None

        reason = kept()
        if reason is not None:
            result.skipped.append({"path": str(retirement.path), "reason": reason})
            continue
        if apply:
            with (
                checkout_state_retirement(
                    retirement.original, purged_root=retirement.path
                ) as exclusive,
                ExitStack() as locks,
            ):
                if retirement.reason == "integration-receipt":
                    handle = _try_acquire_file_lock(
                        Path(retirement.owner) / "molt-integrate.lock"
                    )
                    if handle is None:
                        result.skipped.append(
                            {
                                "path": str(retirement.path),
                                "reason": "integration-in-use",
                            }
                        )
                        continue
                    locks.callback(_release_file_lock, handle)
                reason = (
                    "retirement-custody-changed"
                    if not exclusive
                    or read_soft_retirement(record, owning_root) != retirement
                    else kept(
                        integration_lock_owned=retirement.reason
                        == "integration-receipt",
                        state_lock_owned=True,
                    )
                )
                if reason is not None:
                    result.skipped.append(
                        {
                            "path": str(retirement.path),
                            "reason": "retirement-custody-changed"
                            if reason == "lock-or-protection"
                            else reason,
                        }
                    )
                    continue
                ok, error = _timed_delete(
                    retirement.path, owning_root=owning_root, result=result
                )
                if not ok:
                    result.errors.append(f"{retirement.path}: {error}")
                    continue
                try:
                    record.unlink()
                except OSError as exc:
                    result.errors.append(f"{record}: {exc}")
        result.reclaimed.append(
            {"path": str(retirement.path), "kind": "expired-retirement"}
        )


def _receipt_inventory(receipt: Path, deadline: float) -> tuple[int, float]:
    from tools.coordination_records import _evidence_inventory

    size, newest = (
        _evidence_inventory(receipt, deadline)[:2]
        if receipt.is_dir()
        else (receipt.stat().st_size, receipt.stat().st_mtime)
    )
    companion = receipt.with_suffix(".d" if receipt.suffix == ".json" else ".json")
    if companion.is_symlink():
        raise ValueError(f"receipt companion must not be a symlink: {companion}")
    if companion.exists():
        newest = max(
            newest,
            _evidence_inventory(companion, deadline)[1]
            if companion.is_dir()
            else companion.stat().st_mtime,
        )
    return size, newest


def _collect_evidence(
    scope_roots: Sequence[Path],
    result: ReclaimResult,
    *,
    env: Mapping[str, str],
    now: float,
    apply: bool,
    walks: WalkRotation,
    protected: Sequence[Path],
) -> None:
    """Harvest terminal agent evidence and retire idle integration receipts.

    Each family's complete walks rotate across the whole scope, so a large
    task or receipt early in path order cannot starve the rest (HF-528).
    """
    # Git inspection belongs to the coordination owner. The disk guard retains
    # no process-control capability; the helper only queries the object store.
    from tools.coordination_records import (
        LOG_ROOT,
        active_task_references,
        harvest_agent_evidence,
        load_records,
    )

    # Coordination is family-wide: an active task may refer to evidence in
    # another registered checkout. Snapshot each record once for this pass.
    checkout_records = {}
    registry_stamps = {}
    for root in scope_roots:
        if git_directories(root) is None:
            continue
        registry = root / LOG_ROOT
        if registry.is_symlink():
            continue
        registry_stamps[registry] = (
            registry.stat().st_mtime_ns if registry.exists() else None
        )
        checkout_records[root] = load_records(root)
    records = [record for group in checkout_records.values() for record in group]
    references = active_task_references(records)
    record_stamps = {
        record.path: record.signature
        for record in records
        if record.signature is not None
    }
    tasks: dict[str, tuple[Path, Path]] = {}
    for root in checkout_records:
        log_root = root / LOG_ROOT
        if log_root.is_dir() and not log_root.is_symlink():
            tasks.update((str(task), (root, task)) for task in log_root.iterdir())
    # One rotation across every checkout: consecutive keys of one checkout
    # share one harvest and its one repository object stream.
    ordered = walks.order("agent-evidence", tasks)
    for root, run in groupby(ordered, key=lambda key: tasks[key][0]):
        for item in harvest_agent_evidence(
            root,
            environment=env,
            records=checkout_records[root],
            references=references,
            record_stamps=record_stamps,
            registry_stamps=registry_stamps,
            now=now,
            apply=apply,
            tasks=[tasks[key][1] for key in run],
            walks=walks,
            protected=protected,
        ):
            if item.get("error"):
                result.errors.append(f"{item['path']}: {item['reason']}")
            elif "reason" in item:
                result.skipped.append(item)
            else:
                result.reclaimed.append(item)
                if "retired_path" in item:
                    result.retirements[item["path"]] = item["retired_path"]

    receipts: dict[str, tuple[Path, Path, Path]] = {}
    seen_receipts: set[Path] = set()
    for root in checkout_records:
        directories = git_directories(root)
        assert directories is not None
        common = directories[1]
        container = common / "molt-integrate" / "receipts"
        if container in seen_receipts:
            continue
        seen_receipts.add(container)
        if not container.is_dir() or _region_locked(common / "molt-integrate.lock"):
            continue
        receipts.update(
            (str(receipt), (root, common, receipt))
            for receipt in _iter_dir(container)
            if not receipt.is_symlink()
            and (receipt.suffix == ".json" or receipt.suffix == ".d")
        )
    for key in walks.order("integration-receipts", receipts):
        root, common, receipt = receipts[key]
        try:
            size, newest = walks.spend(
                "integration-receipts", key, partial(_receipt_inventory, receipt)
            )
        except WalkIncomplete as incomplete:
            result.skipped.append(
                {
                    "path": str(receipt),
                    "kind": "integration-receipt",
                    "reason": incomplete.reason,
                }
            )
            continue
        except (OSError, ValueError) as exc:
            result.errors.append(f"{receipt}: {exc}")
            continue
        if now - newest < RETIREMENT_WINDOW_S or _is_protected(receipt, protected):
            continue
        if apply:
            from molt.file_locks import _try_acquire_file_lock, _release_file_lock

            handle = _try_acquire_file_lock(common / "molt-integrate.lock")
            if handle is None:
                result.skipped.append(
                    {"path": str(receipt), "reason": "integration-in-use"}
                )
                continue
            try:
                _, current_use = walks.spend(
                    "integration-receipts", key, partial(_receipt_inventory, receipt)
                )
                if now - current_use < RETIREMENT_WINDOW_S or _is_protected(
                    receipt, _protected_paths(scope_roots, env)
                ):
                    result.skipped.append(
                        {"path": str(receipt), "reason": "receipt-use-changed"}
                    )
                    continue
                ok, error = _timed_retire(
                    receipt,
                    custody_root(root),
                    reason="integration-receipt",
                    owner=str(common),
                    now=now,
                    result=result,
                )
                if not ok:
                    result.errors.append(f"{receipt}: {error}")
                    continue
            except WalkIncomplete as incomplete:
                result.skipped.append(
                    {
                        "path": str(receipt),
                        "kind": "integration-receipt",
                        "reason": incomplete.reason,
                    }
                )
                continue
            except (OSError, ValueError) as exc:
                result.errors.append(f"{receipt}: {exc}")
                continue
            finally:
                _release_file_lock(handle)
        result.reclaimed.append(
            {"path": str(receipt), "kind": "integration-receipt", "est_bytes": size}
        )

    # The guard's canonical incident roots are finite, external control state.
    # Explicit outputs elsewhere remain operator-owned, never a generic sweep.
    for root in scope_roots:
        for active in active_guard_marker_dirs_of(root):
            if has_active_guard_marker(active):
                continue
            for incidents in (
                active.parent / "incidents",
                active.parent.parent / "harness_memory_guard" / "incidents",
            ):
                for record in _iter_dir(incidents):
                    if (
                        record.is_symlink()
                        or not record.is_file()
                        or not (
                            record.name.endswith(".json")
                            or record.name.endswith(".json.tmp")
                        )
                    ):
                        continue
                    found = record.stat()
                    if now - found.st_mtime < RETIREMENT_WINDOW_S or _is_protected(
                        record, protected
                    ):
                        continue
                    if not record.is_relative_to(custody_root(root)):
                        result.skipped.append(
                            {
                                "path": str(record),
                                "reason": "outside-retirement-custody",
                            }
                        )
                        continue
                    if apply:
                        ok, error = _timed_retire(
                            record,
                            custody_root(root),
                            reason="guard-incident",
                            owner=str(incidents),
                            now=now,
                            result=result,
                        )
                        if not ok:
                            result.errors.append(f"{record}: {error}")
                            continue
                    result.reclaimed.append(
                        {
                            "path": str(record),
                            "kind": "guard-incident",
                            "est_bytes": found.st_size,
                        }
                    )


def _superseded_layout_roots(
    owner_root: Path, config: GuardConfig
) -> list[tuple[Path, Literal["cargo-target", "build-control"]]]:
    """The live roots of ``owner_root`` that can hold a superseded layout.

    Every Cargo target the guard knows (checkout targets, pinned session
    targets, legacy lane and flat targets) and every build control.
    """
    roots: list[tuple[Path, Literal["cargo-target", "build-control"]]] = []
    for child in _checkout_state_dirs(owner_root):
        kind = checkout_state_kind_of(child, owner_root)
        if kind == "build-control":
            roots.append((child, "build-control"))
        elif kind == "cargo-target":
            roots.append((child, "cargo-target"))
    roots.extend((child, "cargo-target") for child in session_target_dirs(owner_root))
    for parent in (owner_root, _target_root(owner_root)):
        for child in _iter_dir(parent):
            if child.is_dir() and _is_reclaimable(
                child, owner_root, config.lane_globs
            ) in {"lane", "cargo-target"}:
                roots.append((child, "cargo-target"))
    return roots


def assert_safe_to_retire_layout(
    path: Path,
    root: Path,
    owner_root: Path,
    kind: Literal["cargo-target", "build-control"],
    lane_globs: Sequence[str],
) -> None:
    """Raise unless ``path`` is a superseded layout entry of an allow-set root.

    The layout choke point beside ``assert_safe_to_delete``: the root must be
    one the guard may reclaim, and ``path`` one of the frozen names that
    ``molt.checkout_state.superseded_layout_paths`` lists inside it now.
    """
    owner_root = Path(owner_root).resolve()
    root = Path(root).resolve()
    rp = Path(path).resolve()
    if _is_reclaimable(root, owner_root, lane_globs) is None:
        raise ValueError(f"disk_guard: {root} is not in the reclaimable allow-set")
    if root not in rp.parents:
        raise ValueError(f"disk_guard: refuse a layout entry outside its root: {rp}")
    if rp not in {entry.resolve() for entry in superseded_layout_paths(root, kind)}:
        raise ValueError(f"disk_guard: {rp} is not a superseded layout entry")


def _retire_superseded_layouts(
    owner_root: Path,
    scope_roots: Sequence[Path],
    result: ReclaimResult,
    *,
    config: GuardConfig,
    now: float,
    apply: bool,
    retired: set[str],
    protected: Sequence[Path],
    deadline: float,
    guards: _GuardProtection,
) -> None:
    """Retire layouts a later design replaced inside live roots (design 56, 2.8).

    The per-target runtime and backend generations, their receipts and probe
    evidence have no reader on current code. Old code may still run in a
    checkout that has not taken the change, so an entry retires only under
    the same custody a root does: idle past the protection window, no build
    lock held below its root, not protected, and exclusive retirement of the
    root, which every managed build holds shared for its whole run. An old
    build that runs later misses and rebuilds; it never reads a partial entry.
    """
    for root, kind in _superseded_layout_roots(owner_root, config):
        if _norm(root) in retired or not root.is_dir():
            continue

        def idle(path: Path) -> bool:
            return now - _newest_activity_mtime(path) >= CHECKOUT_STATE_WINDOW_S

        candidates = [
            path
            for path in superseded_layout_paths(root, kind)
            if idle(path) and not _is_protected(path, protected)
        ]
        if not candidates:
            continue
        if guards.protects(root, owner_root):
            result.skipped.append({"path": str(root), "reason": "active-guard"})
            continue
        if _root_lock_held(root, scope_roots):
            result.skipped.append({"path": str(root), "reason": "lock-held"})
            continue
        if not apply:
            result.reclaimed.extend(
                {
                    "path": str(path),
                    "kind": "superseded-layout",
                    "est_bytes": _dir_size(path, deadline)
                    if path.is_dir()
                    else path.stat().st_size,
                }
                for path in candidates
            )
            continue
        from molt.checkout_state import checkout_state_retirement

        with checkout_state_retirement(root) as exclusive:
            if not exclusive:
                result.skipped.append({"path": str(root), "reason": "state-in-use"})
                continue
            if _guard_protection(scope_roots).protects(root, owner_root):
                result.skipped.append({"path": str(root), "reason": "active-guard"})
                continue
            for path in candidates:
                size = (
                    _dir_size(path, deadline) if path.is_dir() else path.stat().st_size
                )
                if not idle(path):
                    result.skipped.append(
                        {
                            "path": str(path),
                            "reason": "used within the protection window",
                        }
                    )
                    continue
                try:
                    assert_safe_to_retire_layout(
                        path, root, owner_root, kind, config.lane_globs
                    )
                except ValueError as error:
                    result.errors.append(str(error))
                    continue
                ok, error = _timed_retire(
                    path,
                    custody_root(owner_root),
                    reason="superseded-layout",
                    owner=str(root),
                    now=now,
                    result=result,
                )
                if not ok:
                    result.errors.append(f"{path}: {error}")
                    continue
                result.reclaimed.append(
                    {"path": str(path), "kind": "superseded-layout", "est_bytes": size}
                )


def collect(
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
) -> list[ReclaimResult]:
    """The daily collection: retirement by policy, then reclaim under pressure.

    The GC retires checkout state and pinned session targets by last use and
    checkout state by owner. The pressure pass is a no-op above the high-water
    mark and otherwise evicts the least recently used reclaimable roots until
    the free target is met, as BuildKit keeps a minimum of free space.
    """
    return [gc(root=root, env=env), ensure_free(root=root, env=env)]


def collect_due(
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
) -> list[ReclaimResult]:
    """Collect one due daily attempt, retaining admission through completion.

    The existing context owns the nonblocking lock, due recheck and start
    stamp. A recent or competing attempt does no collection. Exceptions stay
    visible to the owning worker; a returned result has left this context.
    """
    from molt.checkout_state import checkout_state_gc

    environment = os.environ if env is None else env
    resolved_root = resolve_root(root, env=environment)
    with checkout_state_gc(resolved_root) as admitted:
        if not admitted:
            return []
        return collect(root=resolved_root, env=environment)


def _retention_candidates(
    owner_root: Path,
    scope_roots: Sequence[Path],
    *,
    now: float,
    limits: RetentionLimits,
    config: GuardConfig,
    build_control: bool = False,
    guards: _GuardProtection | None = None,
) -> list[Candidate]:
    """The last-use roots of ``owner_root`` that the retention policy selects.

    With ``build_control`` it selects only build-control roots, for the pass
    that follows a GC's reclaimed targets.
    """
    roots: list[tuple[Path, CandidateKind]] = [
        (child, "checkout-state")
        for child in _checkout_state_dirs(owner_root)
        if not is_session_target(child, owner_root)
        and (not build_control or child.parent.name == "build-control")
    ]
    if not build_control:
        roots.extend(
            (child, "session-target") for child in session_target_dirs(owner_root)
        )
        # Keep one layout/glob authority for both pressure and timed retention.
        for parent in (owner_root, _target_root(owner_root)):
            try:
                children = tuple(parent.iterdir())
            except OSError:
                continue
            for child in children:
                kind = _is_reclaimable(child, owner_root, config.lane_globs)
                if kind in _LEGACY_TARGET_KINDS and child.is_dir():
                    roots.append((child, kind))
    guards = _guard_protection(scope_roots) if guards is None else guards
    return [
        Candidate(
            path=child,
            kind=kind,
            guard_active=guards.protects(child, owner_root),
            mtime=checkout_state_last_use(child),
            lock_held=_root_lock_held(child, scope_roots),
            owner_root=owner_root,
        )
        for child, kind in roots
        if checkout_state_decision(child, owner_root, now=now, limits=limits)
        is not None
    ]


def reclaim_checkout_state(
    checkout: Path,
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    apply: bool = True,
) -> ReclaimResult:
    """Immediately reclaim the state roots that a removed checkout owned (HF-313).

    Each root's owner record must name ``checkout``, and the checkout must be
    absent; a build-control root goes with the checkout target (or pinned
    session) that owns it, once that target is gone or retired in this pass.
    The checkout target takes its pinned sessions along. Retirement is the
    owner's explicit act, so it skips the protection window; a held lock, in
    the target, its sessions or their build control, or a protected path
    still keeps a root.
    """
    started = time.monotonic()
    env = os.environ if env is None else env
    checkout = checkout.expanduser().resolve()
    resolved_root = resolve_root(root, env=env)
    scope_roots = reclaim_roots(resolved_root)
    cfg = GuardConfig.from_env(env)
    result = ReclaimResult(
        operation="checkout-retirement",
        root=str(resolved_root),
        triggered=True,
        free_before=0,
        free_after=0,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    result.phase_elapsed_s["setup"] = _elapsed_since(started)
    phase_started = time.monotonic()
    guards = _guard_protection(scope_roots)
    result.custody = guards.report()
    protected = _protected_paths(scope_roots, env)
    roots_by_owner = {
        owner_root: sorted(
            _checkout_state_dirs(owner_root),
            key=lambda child: child.parent.name == "build-control",
        )
        for owner_root in scope_roots
    }
    result.phase_elapsed_s["discovery"] = _elapsed_since(phase_started)
    phase_started = time.monotonic()
    component = checkout_component(checkout)
    retired_targets: set[Path] = set()
    for owner_root, children in roots_by_owner.items():
        # Build control follows its target, so the targets go first.
        for child in children:
            # This record only rejects unrelated roots. Binding every recorded
            # target first resolves its ancestors, even for another checkout.
            # Matching records still need full binding and the locked reread.
            recorded_owner = read_checkout_owner(child)
            if recorded_owner is None:
                continue
            recorded_target = checkout_target_of(recorded_owner)
            if recorded_target is None:
                if recorded_owner.path != checkout:
                    continue
            elif recorded_target.name != component:
                continue
            owner = bound_checkout_owner(child.resolve(), owner_root.resolve())
            if owner is None:
                continue
            target = checkout_target_of(owner)
            if target is None:
                if owner.path != checkout or not owner_is_absent(owner):
                    continue
            elif target.name != component or not (
                target in retired_targets or owner_is_absent(owner)
            ):
                continue
            candidate = Candidate(
                path=child.resolve(),
                kind="checkout-state",
                mtime=checkout_state_last_use(child),
                lock_held=_root_lock_held(child, scope_roots),
                owner_root=owner_root,
                guard_active=guards.protects(child, owner_root),
            )
            allowed, reason = decide_owner_reclaim(candidate, protected=protected)
            if not allowed:
                result.skipped.append({"path": str(candidate.path), "reason": reason})
                continue
            assert_safe_to_delete(candidate.path, owner_root, cfg.lane_globs)
            if apply:

                def still_retired(current: Candidate, current_now: float) -> str | None:
                    del current_now
                    current_owner = bound_checkout_owner(current.path, owner_root)
                    if current_owner is None or current_owner != owner:
                        return "checkout-owner-changed"
                    current_target = checkout_target_of(current_owner)
                    if current_target is None:
                        if current_owner.path != checkout or not owner_is_absent(
                            current_owner
                        ):
                            return "checkout-owner-live"
                    elif current_target.name != component or not (
                        current_target in retired_targets
                        or owner_is_absent(current_owner)
                    ):
                        return "checkout-target-live"
                    allowed, reason = decide_owner_reclaim(
                        current, protected=_protected_paths(scope_roots, env)
                    )
                    return None if allowed else reason

                ok, error, skipped = _retire_candidate(
                    candidate,
                    owner_root,
                    cfg,
                    scope_roots=scope_roots,
                    now_fn=time.time,
                    eligible=still_retired,
                    result=result,
                )
                if skipped is not None:
                    result.skipped.append(
                        {"path": str(candidate.path), "reason": skipped}
                    )
                if error is not None:
                    result.errors.append(f"{candidate.path}: {error}")
                if not ok:
                    continue
            result.reclaimed.append(
                {"path": str(candidate.path), "kind": "checkout-state"}
            )
            if owner.kind == "cargo-target":
                retired_targets.add(candidate.path)
    result.phase_elapsed_s["retirement"] = _elapsed_since(phase_started)
    result.elapsed_s = _elapsed_since(started)
    return result


def _protected_paths(
    roots: Path | Sequence[Path], env: Mapping[str, str]
) -> list[Path]:
    """Paths that must never be reclaimed even if they match the allow-set.

    Protect explicit Cargo targets, this interpreter's environment, and the
    targets of the current pinned session: with no source context, every
    session target that carries its component, in either layout, stays.
    """
    scan_roots = (roots,) if isinstance(roots, Path) else tuple(roots)
    protected: list[Path] = []
    session = str(env.get("MOLT_SESSION_ID", "")).strip()
    if session:
        component = session_artifact_component(session)
        protected.extend(
            target
            for root in scan_roots
            for target in session_target_dirs(root)
            if target.name == component
        )
    for name in (
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "MOLT_PROOF_SCRATCH_ROOT",
        "MOLT_GUARD_SCRATCH_ROOT",
        "MOLT_MEMORY_GUARD_STATE_ROOT",
    ):
        raw = str(env.get(name, "")).strip()
        if raw:
            protected.append(Path(raw).expanduser().resolve(strict=False))
    # This interpreter's own environment, which may be a removed checkout's.
    protected.append(Path(sys.prefix).resolve(strict=False))
    return protected


def _write_log(root: Path, result: ReclaimResult) -> Path | None:
    try:
        log_dir = root / LOG_RELDIR
        log_dir.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%dT%H%M%S", time.localtime())
        # Exclusive creation preserves distinct phases and repeated invocations
        # even when the same process finishes them in one clock second.
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=log_dir,
            prefix=f"disk_guard-{stamp}-{os.getpid()}-{result.operation}-",
            suffix=".json",
            delete=False,
        ) as stream:
            json.dump(result.to_dict(), stream, indent=2)
            return Path(stream.name)
    except OSError:
        return None


# --- self-test (gate-liveness canary; proves teeth without a live volume) ---


def selftest() -> list[tuple[str, bool]]:
    """Deterministic canaries over the PURE surfaces. Each returns (name, fired).

    A gate that cannot fire certifies nothing (M34/M42): these feed known inputs
    and assert the guard fires on danger and stands down when safe.
    """
    now = 1_000_000.0
    min_idle = 900.0  # 15 min
    stale = Candidate(
        Path("/x/target/sessions/old"),
        "session-target",
        mtime=now - 10_000,
        size=5 * _GB,
    )
    fresh = Candidate(
        Path("/x/target/sessions/live"), "session-target", mtime=now - 5, size=9 * _GB
    )
    locked = Candidate(
        Path("/x/target/codex-held"),
        "lane",
        mtime=now - 10_000,
        size=9 * _GB,
        lock_held=True,
    )

    results: list[tuple[str, bool]] = []

    # 1) below high-water with a stale dir -> reclaim fires.
    p1 = plan_reclaim(
        [stale],
        free_bytes=10 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(
        ("below-highwater-reclaims-stale", p1.triggered and stale in p1.selected)
    )

    # 2) above high-water -> no-op (idempotent).
    p2 = plan_reclaim(
        [stale],
        free_bytes=30 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("above-highwater-noop", (not p2.triggered) and not p2.selected))

    # 3) a freshly-modified (ACTIVE) dir is NEVER selected.
    p3 = plan_reclaim(
        [fresh],
        free_bytes=1 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("active-dir-never-reclaimed", fresh not in p3.selected))

    # 4) a lock-held dir is NEVER selected.
    p4 = plan_reclaim(
        [locked],
        free_bytes=1 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("lock-held-never-reclaimed", locked not in p4.selected))

    # 5) reclaim STOPS once the target is met (does not over-delete).
    small = Candidate(
        Path("/x/target/sessions/a"),
        "session-target",
        mtime=now - 10_000,
        size=40 * _GB,
    )
    extra = Candidate(
        Path("/x/target/sessions/b"),
        "session-target",
        mtime=now - 9_000,
        size=40 * _GB,
    )
    p5 = plan_reclaim(
        [small, extra],
        free_bytes=1 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("stops-at-target", p5.selected == [small]))

    # 6) the GC retires an idle root, never a locked or protected one.
    idle = Candidate(
        Path("/x/target/sessions/idle"), "session-target", mtime=now - 100_000
    )
    held = Candidate(
        Path("/x/target/sessions/held"),
        "session-target",
        mtime=now - 100_000,
        lock_held=True,
    )
    mine = Candidate(
        Path("/x/target/sessions/mine"), "session-target", mtime=now - 100_000
    )
    collect = gc_plan(
        [idle, held, mine], now=now, min_idle_s=min_idle, protected=[mine.path]
    )
    results.append(("gc-keeps-locked-and-protected-roots", collect == [idle]))

    # 7) a nonterminal memory guard is stronger than age or free-space pressure.
    guarded = Candidate(
        Path("/x/target/codex-guarded"),
        "lane",
        mtime=now - 100_000,
        size=100 * _GB,
        guard_active=True,
    )
    p7 = plan_reclaim(
        [guarded],
        free_bytes=1 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("active-guard-never-reclaimed", guarded not in p7.selected))

    # 8) a last-use root used within the protection window survives pressure.
    recent = Candidate(
        Path("/x/target/sessions/recent"),
        "session-target",
        mtime=now - 3600,
        size=100 * _GB,
    )
    p8 = plan_reclaim(
        [recent],
        free_bytes=1 * _GB,
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        now=now,
        min_idle_s=min_idle,
    )
    results.append(("recent-use-never-reclaimed", recent not in p8.selected))

    return results


# --- CLI --------------------------------------------------------------------


def _print_result(result: ReclaimResult, *, as_json: bool) -> None:
    if as_json:
        print(json.dumps(result.to_dict(), indent=2))
        return
    verb = "retired" if result.mode == "apply" else "would retire"
    print(
        f"disk_guard [{result.mode}] root={result.root} "
        f"free={_human(result.free_before)} triggered={result.triggered}"
    )
    for item in result.reclaimed:
        size = f", {item['est_bytes']} bytes" if "est_bytes" in item else ""
        print(f"  {verb}: {item['path']} ({item['kind']}{size})")
    if result.mode == "apply" and result.triggered:
        print(
            f"  free {_human(result.free_before)} -> {_human(result.free_after)} "
            f"(+{_human(max(0, result.reclaimed_bytes))})"
        )
    for family, walks in result.walk_rotation.items():
        print(
            f"  {family} walks: {walks['walks']} complete in {walks['walk_s']:.1f}s, "
            f"{walks['deferred']} deferred, {walks['exceeded']} over budget; "
            f"cursor {walks['cursor'] or 'start'} -> {walks['next_cursor'] or 'start'}"
        )
    for skip in result.skipped[:10]:
        print(f"  skip: {skip['path']} ({skip['reason']})")
    for err in result.errors[:10]:
        _eprint(f"  ERROR {err}")
    for item in result.custody:
        print(
            f"  active guard records: {item['blocking_records']} in "
            f"{item['active_dir']}; next step: {item['next_step']}"
        )
        for record in item["records"]:
            if record["operator_action_required"] or record["coordinate_free"]:
                print(
                    f"    {record['marker']}: {record['reason']}; "
                    f"owner={record['checkout']}; coordinate-free={record['coordinate_free']}; "
                    f"next step: {record['release_command']}"
                )


def _incremental_coordinate(path: Path, root: Path) -> tuple[Path, Path]:
    from molt.file_publication import resolve_owned_path

    root, path = resolve_owned_path(root), resolve_owned_path(path)
    # RuntimeCargoPlan.with_build_directory owns this toolchain partition.
    relative = path.relative_to(root).parts
    if (
        len(relative) not in {9, 10}
        or relative[:2] != ("target", "checkouts")
        or relative[3] != "sessions"
        or relative[5] != ".molt-toolchains"
        or re.fullmatch(r"[0-9a-f]{64}", relative[6]) is None
        or relative[-1] != "incremental"
    ):
        raise ValueError(
            "requires a pinned session's toolchain/profile incremental root"
        )
    session = root.joinpath(*relative[:5])
    owner = bound_checkout_owner(session, root)
    if owner is None or owner.kind != "cargo-target":
        raise ValueError("incremental session lacks its bound checkout owner")
    if not owner.path.is_dir() and not owner_is_absent(owner):
        raise ValueError(
            "incremental checkout owner is inaccessible or not a directory"
        )
    return session, owner.path


def _incremental_successful_guard(
    receipt: Path, *, root: Path, session: Path, owner: Path, path: Path
) -> str:
    from molt.cargo_execution_policy import (
        cargo_command_options,
        cargo_profile_directory,
    )
    from molt.file_publication import resolve_owned_path
    from molt.memory_guard_paths import retired_guard_marker_dir
    from tools.memory_guard_core.active_custody import read_marker_record

    receipt = resolve_owned_path(receipt)
    if receipt.parent not in {
        retired_guard_marker_dir(active) for active in active_guard_marker_dirs_of(root)
    }:
        raise ValueError("requires the existing retired guard receipt authority")
    record = read_marker_record(receipt)
    value = record.payload or {}
    if (
        not record.terminal
        or record.status not in {"completed", "finalizer_completed"}
        or record.descendants_closed is not True
        or type(value.get("returncode")) is not int
        or value["returncode"] != 0
        or type(value.get("child_returncode")) is not int
        or value["child_returncode"] != 0
        or value.get("cwd") != str(owner)
    ):
        raise ValueError("requires a successful, descendant-closed checkout guard")
    argv = value.get("command")
    if (
        not isinstance(argv, list)
        or len(argv) < 2
        or any(type(arg) is not str for arg in argv)
        or Path(argv[0]).name not in {"cargo", "cargo.exe"}
        or not Path(argv[0]).is_absolute()
        or argv[1] not in {"build", "check"}
        or argv != value.get("launch_command")
        or argv != (value.get("child_process") or {}).get("command")
        or "--" in argv
        or any(arg == "--target-dir" or arg.startswith("--target-dir=") for arg in argv)
        or argv.count("--profile") != 1
        or any(
            arg in {"-r", "--release"} or arg.startswith("--profile=") for arg in argv
        )
    ):
        raise ValueError("guard must record the directly launched pinned Cargo command")
    target, configs = cargo_command_options(argv)
    outputs: dict[str, str] = {}
    for raw in configs:
        parsed = tomllib.loads(raw)
        build = parsed.get("build", {})
        if not isinstance(build, dict):
            raise ValueError("malformed Cargo build configuration")
        for key in ("build-dir", "target-dir"):
            if key in build:
                if key in outputs or type(build[key]) is not str:
                    raise ValueError("ambiguous Cargo output coordinate")
                outputs[key] = build[key]
    build_root = session / ".molt-toolchains" / path.relative_to(session).parts[1]
    if outputs != {"build-dir": str(build_root), "target-dir": str(session)}:
        raise ValueError("guard command does not bind this session's Cargo roots")
    profile_index = argv.index("--profile") + 1
    if profile_index >= len(argv):
        raise ValueError("missing Cargo profile")
    expected = build_root
    if target is not None:
        expected /= target
    expected /= cargo_profile_directory(argv[profile_index])
    if expected / "incremental" != path:
        raise ValueError("guard command does not bind this Cargo target/profile")
    assert record.identity is not None
    return record.identity.sha256


def _incremental_inventory(path: Path) -> dict[str, object]:
    from molt.file_publication import metadata_is_link_like, resolve_owned_path

    from molt.durable_file_identity import file_identity

    entries = []
    inodes: dict[tuple[int, int], list[int]] = {}
    root_device = path.lstat().st_dev
    pending = [path]
    deadline = time.monotonic() + 10.0
    while pending:
        current = pending.pop()
        if len(entries) >= 100_000 or time.monotonic() > deadline:
            raise ValueError("incremental inventory exceeds its bounded review budget")
        metadata = current.lstat()
        regular = stat.S_ISREG(metadata.st_mode)
        if (
            metadata_is_link_like(metadata)
            or metadata.st_dev != root_device
            or not metadata.st_ino
            or (not regular and not stat.S_ISDIR(metadata.st_mode))
            or os.path.ismount(current)
            or resolve_owned_path(current) != current
        ):
            raise ValueError(
                "incremental inventory contains an indirect or special entry"
            )
        if regular:
            if current.suffix not in {".bin", ".o", ".rmeta", ".bc", ".lock"}:
                raise ValueError(
                    "incremental inventory contains an unknown payload kind"
                )
            inode = inodes.setdefault(
                (metadata.st_dev, metadata.st_ino),
                [0, metadata.st_nlink, getattr(metadata, "st_blocks", 0) * 512],
            )
            if inode[1] != metadata.st_nlink:
                raise ValueError(
                    "incremental hardlink identity changed during inventory"
                )
            inode[0] += 1
        else:
            for child in current.iterdir():
                if (
                    len(entries) + len(pending) >= 100_000
                    or time.monotonic() > deadline
                ):
                    raise ValueError(
                        "incremental inventory exceeds its bounded review budget"
                    )
                pending.append(child)
        entries.append(
            [
                str(current.relative_to(path)),
                metadata.st_mode,
                *file_identity(current, expected=metadata).stat_prefix(),
                metadata.st_size,
                metadata.st_nlink,
                metadata.st_mtime_ns,
                metadata.st_ctime_ns,
                getattr(metadata, "st_blocks", 0),
            ]
        )
    return {
        "schema": "molt.cargo-incremental-inventory.v2",
        "entries": sorted(entries),
        "regular_files": sum(item[0] for item in inodes.values()),
        # Unlinking a cache hardlink never changes bytes/attributes of deps.
        # Count only inodes whose every link is inside this deletion scope.
        "reclaimable_allocated_bytes": sum(
            blocks for links, count, blocks in inodes.values() if links == count
        ),
        "externally_linked_bytes_retained": sum(
            blocks for links, count, blocks in inodes.values() if links != count
        ),
    }


def retire_cargo_incremental(
    path: Path,
    *,
    root: Path,
    guard_receipt: Path,
    expected_inventory_sha256: str | None = None,
    apply: bool = False,
    env: Mapping[str, str] | None = None,
) -> dict[str, object]:
    """Explicit successful-session cache retirement; never pressure discovery.

    The checkout lease excludes managed users. The build and output Cargo profile locks remain
    held across review and durable retirement, preserving their original names
    for waiting Cargo. Raw Rustc writers bypassing Cargo locks are unsupported,
    just as in interrupted-Cargo recovery. No process is launched or stopped.
    """
    from contextlib import ExitStack
    from molt.checkout_state import checkout_state_retirement
    from molt.file_locks import (
        _try_acquire_file_lock,
        _release_file_lock,
        _file_lock_owned_operation,
    )
    from molt.file_publication import resolve_owned_path
    from tools.memory_guard_core.cargo_quarantine import _local_cargo_lock_filesystem

    environment = os.environ if env is None else env
    if apply and (
        type(expected_inventory_sha256) is not str
        or re.fullmatch(r"[0-9a-f]{64}", expected_inventory_sha256) is None
    ):
        raise ValueError("apply requires the independently reviewed inventory SHA256")
    root = resolve_owned_path(root)
    path = resolve_owned_path(path)
    session, owner = _incremental_coordinate(path, root)
    if _is_protected(session, _protected_paths((root,), environment)):
        raise ValueError(
            "incremental session is protected by this execution environment"
        )
    profile = path.parent
    build_root = session / ".molt-toolchains" / path.relative_to(session).parts[1]
    output_profile = resolve_owned_path(session / profile.relative_to(build_root))
    if not all(_local_cargo_lock_filesystem(p) for p in (profile, output_profile)):
        raise ValueError("Cargo-compatible local filesystem authority unavailable")
    with checkout_state_retirement(session) as exclusive, ExitStack() as stack:
        if not exclusive:
            raise ValueError("incremental session has a live managed reader")
        if _root_lock_held(session, (root,)):
            raise ValueError("incremental session has a live build-control/Cargo owner")
        handles = []
        # Cargo Layout::new locks intermediate build state, then final output.
        # They are distinct when build.build-dir and target-dir are split.
        for raw_lock in (profile / ".cargo-build-lock", output_profile / ".cargo-lock"):
            lock = resolve_owned_path(raw_lock)
            metadata = lock.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
                raise ValueError(
                    "Cargo profile lock is not a direct unshared regular file"
                )
            handle = _try_acquire_file_lock(lock, create=False)
            if handle is None:
                raise ValueError("incremental Cargo profile is active")
            stack.callback(_release_file_lock, handle)
            held, named = os.fstat(handle.file.fileno()), lock.lstat()
            if not stat.S_ISREG(named.st_mode) or not os.path.samestat(held, named):
                raise ValueError("Cargo lock path identity changed")
            stack.enter_context(
                _file_lock_owned_operation(handle, expected_lock_path=lock)
            )
            handles.append((lock, handle))
        if _incremental_coordinate(path, root) != (session, owner):
            raise ValueError("incremental checkout ownership changed")
        receipt_sha256 = _incremental_successful_guard(
            guard_receipt, root=root, session=session, owner=owner, path=path
        )
        inventory = _incremental_inventory(path)
        reviewed = {
            "path": str(path),
            "root": str(root),
            "session": str(session),
            "owner": str(owner),
            "guard_receipt": str(guard_receipt),
            "guard_sha256": receipt_sha256,
            **inventory,
        }
        digest = hashlib.sha256(
            json.dumps(reviewed, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if (
            expected_inventory_sha256 is not None
            and digest != expected_inventory_sha256
        ):
            raise ValueError("incremental inventory or custody changed since review")
        free_before = shutil.disk_usage(path).free
        retirement = None
        if apply:
            # Recheck the exact open lock objects and namespace under the same
            # lease immediately before handing off to canonical durable removal.
            if _incremental_inventory(path) != inventory:
                raise ValueError("incremental inventory changed before retirement")
            if _incremental_coordinate(path, root) != (session, owner):
                raise ValueError("incremental checkout ownership changed")
            if (
                _incremental_successful_guard(
                    guard_receipt, root=root, session=session, owner=owner, path=path
                )
                != receipt_sha256
            ):
                raise ValueError("incremental guard receipt changed")
            for lock, handle in handles:
                held, named = os.fstat(handle.file.fileno()), lock.lstat()
                if not stat.S_ISREG(named.st_mode) or not os.path.samestat(held, named):
                    raise ValueError("Cargo lock path identity changed")
            metadata = path.lstat()
            root_entry = next(row for row in inventory["entries"] if row[0] == ".")
            from molt.durable_file_identity import file_identity

            from molt.durable_file_identity import DurableFileIdentity

            if metadata.st_mode != root_entry[1] or file_identity(
                path, expected=metadata
            ) != DurableFileIdentity.from_stat_prefix(*root_entry[2:4]):
                raise ValueError("incremental root identity changed before retirement")
            retirement = soft_retire(
                path, root, reason="cargo-incremental:" + digest, owner=str(owner)
            )
        return {
            "operation": "retire-cargo-incremental",
            "applied": apply,
            "retired_path": str(retirement.path) if retirement is not None else None,
            "inventory_sha256": digest,
            **reviewed,
            "free_before": free_before,
            "free_after": shutil.disk_usage(profile).free,
        }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--root", help="artifact root (required unless $MOLT_EXT_ROOT is set)"
    )
    ap.add_argument(
        "--ensure-free",
        nargs="?",
        type=float,
        const=-1.0,
        metavar="GB",
        help="preemptive reclaim to keep free space above the target "
        "(optional GB overrides the high-water floor)",
    )
    ap.add_argument(
        "--gc",
        action="store_true",
        help="retire checkout state roots by last use "
        "(MOLT_CHECKOUT_STATE_MAX_AGE_DAYS) or a removed owner, and pinned session "
        "targets by last use (MOLT_SESSION_TARGET_IDLE_HOURS); scratch uses "
        "MOLT_SCRATCH_IDLE_DAYS, a lane root's stamp and a legacy root's "
        "complete walk",
    )
    ap.add_argument("--dry-run", action="store_true", help="report only; never delete")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument(
        "--check",
        action="store_true",
        help="run self-test canaries; exit 1 if any is dead",
    )
    ap.add_argument("--high-water-gb", type=float, default=None)
    ap.add_argument("--target-gb", type=float, default=None)
    ap.add_argument("--retire-cargo-incremental", type=Path)
    ap.add_argument("--guard-receipt", type=Path)
    ap.add_argument("--inventory-sha256")
    ap.add_argument(
        "--apply", action="store_true", help="apply reviewed incremental retirement"
    )
    args = ap.parse_args(argv)
    if args.retire_cargo_incremental is not None:
        if (
            not args.root
            or not args.guard_receipt
            or args.gc
            or args.check
            or args.ensure_free is not None
            or (args.apply and args.dry_run)
        ):
            ap.error(
                "incremental retirement requires --root and --guard-receipt, without other operations"
            )
        try:
            report = retire_cargo_incremental(
                args.retire_cargo_incremental,
                root=Path(args.root),
                guard_receipt=args.guard_receipt,
                expected_inventory_sha256=args.inventory_sha256,
                apply=args.apply,
            )
        except (OSError, ValueError, RuntimeError) as error:
            print(
                json.dumps(
                    {"operation": "retire-cargo-incremental", "error": str(error)}
                )
            )
            return 1
        print(json.dumps(report, indent=2))
        return 0
    if args.apply or args.guard_receipt or args.inventory_sha256:
        ap.error("incremental options require --retire-cargo-incremental")

    if args.check:
        results = selftest()
        for name, ok in results:
            print(f"  [{'LIVE' if ok else 'DEAD'}] disk_guard:{name}")
        dead = [n for n, ok in results if not ok]
        if dead:
            print(f"\n{len(dead)} disk_guard canary/canaries FAILED to fire (M34/M42).")
            return 1
        print(f"\nAll {len(results)} disk_guard canaries live.")
        return 0

    config = GuardConfig.from_env(
        os.environ, high_water_gb=args.high_water_gb, target_gb=args.target_gb
    )
    apply = not args.dry_run

    if args.gc:
        result = gc(root=args.root, config=config, apply=apply)
        _print_result(result, as_json=args.json)
        return 0

    if args.ensure_free is not None:
        min_gb = None if args.ensure_free < 0 else args.ensure_free
        result = ensure_free(min_gb, root=args.root, config=config, apply=apply)
        _print_result(result, as_json=args.json)
        return 0

    ap.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
