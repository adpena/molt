#!/usr/bin/env python3
"""Deterministic, preemptive, AGENT-SAFE disk guard for the Molt artifact root.

WHY THIS EXISTS. On 2026-07-11 the canonical NVMe (``C:\\Molt``) filled to
**0 bytes** mid-session and failed several build lanes. The former generic
age/LRU janitor could sweep unrelated artifact classes without consumer
custody, so that authority is retired. This guard owns only its explicit,
consumer-aware build-artifact allow-set; it is not a replacement generic
``tmp`` sweeper.

This guard is **provably agent-safe**: it imports no process API, spawns
nothing, signals nothing, and kills nothing. The paired
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
    keeps, so no one ``rm``-s a finished session's target by hand.

Pure-stdlib (no third-party imports), so it runs under the host ``python`` a
Claude Code hook invokes as well as the molt venv. Windows-first (``msvcrt``
lock probe) with a POSIX (``fcntl``) path; explicit UTF-8 everywhere (M43).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from dataclasses import dataclass, field
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
    _read_git_path,
    custody_root,
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
    bound_checkout_owner,
    checkout_target_of,
    checkout_state_containers,
    checkout_state_decision,
    checkout_state_kind_of,
    checkout_state_last_use,
    is_session_target,
    newest_activity_mtime as _newest_activity_mtime,
    owner_is_absent,
    retention_limits,
    session_target_dirs,
    target_build_control_dirs,
)
from molt.file_deletion import delete_path  # noqa: E402
from molt.memory_guard_paths import active_guard_marker_dirs_of  # noqa: E402
from tools.memory_guard_core.active_custody import (  # noqa: E402
    CUSTODY_COMMAND,
    active_guard_blockers,
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
    "checkout-state", "session-target", "lane", "quarantine", "cargo-target"
]
_LAST_USE_KINDS: frozenset[CandidateKind] = frozenset(
    {"checkout-state", "session-target"}
)


# Hard denylist: even if a classifier ever matched one of these under target/,
# refuse to delete it. These are the SHARED build outputs every lane depends on
# and structural control dirs -- defense-in-depth beside the narrow allow-set.
PROTECTED_TARGET_NAMES = frozenset(
    {
        "debug",
        "release",
        "release-output",
        "doc",
        "package",
        "tmp",
        "sessions",  # the parent; we reclaim its CHILDREN, never itself
        ".molt_state",  # cargo quarantine control parent
        ".rustc_info.json",
        "CACHEDIR.TAG",
        ".fingerprint",
        "wasm32-wasip1",
        "wasm32-unknown-unknown",
        "x86_64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
        "aarch64-apple-darwin",
    }
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
        if running_free >= target_bytes:
            plan.skipped.append((c, "target already met"))
            continue
        plan.selected.append(c)
        running_free += max(0, c.size)
    return plan


def decide_owner_reclaim(
    candidate: Candidate, *, protected: Iterable[Path] = ()
) -> tuple[bool, str]:
    """PURE: an owner's retirement skips the idle window, never lock or protection."""
    if candidate.lock_held:
        return False, "lock-held"
    if _is_protected(candidate.path, tuple(Path(path) for path in protected)):
        return False, "protected"
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
        if c.lock_held:
            continue
        if c.age_s(now) < min_idle_s:
            continue
        out.append(c)
    return out


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
    if rp.name in PROTECTED_TARGET_NAMES:
        return None
    parts = rp.relative_to(root).parts
    target = _target_root(root).relative_to(root).parts
    # A pinned session's target, below a checkout target or the root's own
    # target, and a checkout's state root in its managed container under a
    # component name (HF-313); `checkout_state_decision` decides when each
    # retires.
    if is_session_target(rp, root):
        return "session-target"
    if checkout_state_kind_of(rp, root) is not None:
        return "checkout-state"
    # cargo-incremental quarantine anywhere under a target tree
    if ".molt_state" in parts and "quarantine" in parts:
        return "quarantine"
    # flat legacy cargo-target-* at the artifact-root top level
    if len(parts) == 1 and parts[0].startswith("cargo-target-"):
        return "cargo-target"
    # per-lane isolated target dir directly under <root>/target/
    if len(parts) == len(target) + 1 and parts[: len(target)] == target:
        name = parts[-1]
        if any(_fnmatch(name, g) for g in lane_globs):
            return "lane"
    return None


def _target_root(owner_root: Path) -> Path:
    """The artifact target container, including persistent checkout targets."""
    return owner_root / "target"


def _fnmatch(name: str, pattern: str) -> bool:
    import fnmatch

    return fnmatch.fnmatchcase(name, pattern)


def assert_safe_to_delete(path: Path, root: Path, lane_globs: Sequence[str]) -> None:
    """Raise unless ``path`` is provably a reclaimable artifact dir under root.

    The single choke point every delete passes through. It fails CLOSED: any
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


# --- I/O helpers (stat / lock probe / size) ---------------------------------


# Locks a live user holds under a reclaimable root. Cargo locks each profile
# directory (`<target>/debug/.cargo-lock`, `<target>/<triple>/<profile>/...`),
# not the target root, and a pinned session builds below its checkout target;
# build control holds its build, diff and daemon-suite owner locks.
_LOCK_PATTERNS = (
    ".cargo-lock",
    "*/.cargo-lock",
    "*/*/.cargo-lock",
    "sessions/*/*/.cargo-lock",
    "sessions/*/*/*/.cargo-lock",
    "diff_run.lock",
    "build_locks/*.lock",
    "backend_daemon/suite-leases/*/owner.lock",
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
    try:
        handle = open(lock, "a+b")
    except OSError:
        return False
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


def _has_active_guard(root: Path) -> bool:
    """Project the shared marker authority without observing host processes."""
    return any(
        has_active_guard_marker(markers)
        for markers in active_guard_marker_dirs_of(root)
    )


def _active_guard_custody(roots: Iterable[Path]) -> list[dict]:
    """Name the records that keep each owner root active, and the next step.

    This reads files only. The custody tool takes the process snapshot that
    tells a live guard from a stale or inconclusive record.
    """
    directories = sorted(
        {markers for root in roots for markers in active_guard_marker_dirs_of(root)},
        key=str,
    )
    custody = []
    for markers in directories:
        blockers = active_guard_blockers(markers)
        if not blockers:
            continue
        custody.append(
            {
                "active_dir": str(markers),
                "blocking_records": len(blockers),
                "examples": list(blockers[:5]),
                "next_step": (
                    f"{CUSTODY_COMMAND} --active-dir {markers} names the live, "
                    "stale and inconclusive records; add --apply to resolve "
                    "the stale ones"
                ),
            }
        )
    return custody


def discover_candidates(
    root: Path,
    config: GuardConfig,
    *,
    roots: Sequence[Path] | None = None,
    measure_sizes: bool = True,
    size_budget_s: float = 30.0,
) -> list[Candidate]:
    """Scan every registered worktree's allow-set into one candidate family."""
    root = root.resolve()
    scan_roots = tuple(path.resolve() for path in (roots or reclaim_roots(root)))
    seen: set[str] = set()
    candidates: list[Candidate] = []
    size_deadline = time.monotonic() + size_budget_s

    def add(
        path: Path, kind: CandidateKind, owner_root: Path, *, guard_active: bool
    ) -> None:
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
                # Guard markers elsewhere in the artifact root say nothing
                # about a last-use root; its last use and its locks do.
                guard_active=guard_active and kind not in _LAST_USE_KINDS,
            )
        )

    for owner_root in scan_roots:
        guard_active = _has_active_guard(owner_root)
        # pinned session targets, evicted by last use under pressure
        for child in session_target_dirs(owner_root):
            add(child, "session-target", owner_root, guard_active=guard_active)
        # per-lane target dirs under target/
        for child in _iter_dir(_target_root(owner_root)):
            if (
                child.is_dir()
                and _is_reclaimable(child, owner_root, config.lane_globs) == "lane"
            ):
                add(child, "lane", owner_root, guard_active=guard_active)
        # cargo-incremental quarantine dirs
        for qdir in _iter_quarantine_dirs(owner_root):
            add(qdir, "quarantine", owner_root, guard_active=guard_active)
        # flat legacy cargo-target-*
        for child in _iter_dir(owner_root):
            if child.is_dir() and child.name.startswith("cargo-target-"):
                add(child, "cargo-target", owner_root, guard_active=guard_active)
        # checkout state roots, evicted by last use under pressure
        for child in _checkout_state_dirs(owner_root):
            add(child, "checkout-state", owner_root, guard_active=guard_active)
    return candidates


def _iter_dir(path: Path) -> list[Path]:
    try:
        return sorted(path.iterdir())
    except OSError:
        return []


def _checkout_state_dirs(root: Path) -> list[Path]:
    """Checkout state roots in their managed containers (HF-313)."""
    return [
        child
        for container in checkout_state_containers(root)
        for child in _iter_dir(container)
        if child.is_dir()
        and not child.is_symlink()
        and checkout_state_kind_of(child, root) is not None
    ]


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

    @property
    def reclaimed_bytes(self) -> int:
        return self.free_after - self.free_before

    def to_dict(self) -> dict:
        return {
            "schema_version": 1,
            "tool": "disk_guard",
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
        root=str(resolved_root),
        triggered=free_before < high_water,
        free_before=free_before,
        free_after=free_before,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    if not result.triggered:
        if log:
            _write_log(resolved_root, result)
        return result

    candidates = discover_candidates(resolved_root, cfg, roots=scope_roots)
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
    result.custody = _active_guard_custody(
        cand.owner_root or resolved_root
        for cand, why in plan.skipped
        if why == "active-guard"
    )

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
            try:
                assert_safe_to_delete(cand.path, owner_root, cfg.lane_globs)
            except ValueError as exc:
                result.errors.append(str(exc))
                continue
            ok, err = delete_path(cand.path)
            if not ok:
                result.errors.append(f"{cand.path}: {err}")
                continue
        result.reclaimed.append(
            {"path": str(cand.path), "kind": cand.kind, "est_bytes": max(0, cand.size)}
        )
        free_now = free_fn() if apply else free_now + max(0, cand.size)
    result.free_after = free_fn() if apply else free_now
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

    `checkout_state_decision` selects checkout state past its max age or with
    a removed owner, and pinned session targets idle past their limit; a held
    lock or a protected path keeps a root. A second pass retires the build
    control of the targets this pass reclaimed.
    """
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
    scope_roots = reclaim_roots(resolved_root)
    cfg = config or GuardConfig.from_env(env)
    now = now_fn()
    limits = retention_limits(env)
    free_fn = _default_free_fn(resolved_root)
    free_before = free_fn()
    result = ReclaimResult(
        root=str(resolved_root),
        triggered=True,
        free_before=free_before,
        free_after=free_before,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    protected = _protected_paths(scope_roots, env)
    collectable = gc_plan(
        [
            cand
            for owner_root in scope_roots
            for cand in _retention_candidates(
                owner_root, scope_roots, now=now, limits=limits
            )
        ],
        now=now,
        min_idle_s=cfg.min_idle_s,
        protected=protected,
    )
    collected_norms: set[str] = set()

    def reclaim(planned: list[Candidate]) -> None:
        for cand in planned:
            if apply:
                if not cand.path.exists():
                    continue  # A session target went with its checkout target.
                owner_root = cand.owner_root or resolved_root
                try:
                    assert_safe_to_delete(cand.path, owner_root, cfg.lane_globs)
                except ValueError as exc:
                    result.errors.append(str(exc))
                    continue
                ok, err = delete_path(cand.path)
                if not ok:
                    result.errors.append(f"{cand.path}: {err}")
                    continue
            collected_norms.add(_norm(cand.path))
            result.reclaimed.append({"path": str(cand.path), "kind": cand.kind})

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
                        )
                    ],
                    now=now,
                    min_idle_s=cfg.min_idle_s,
                    protected=protected,
                )
                if _norm(cand.path) not in collected_norms
            ]
        )
    result.free_after = free_fn()
    if log:
        _write_log(resolved_root, result)
    return result


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


def _retention_candidates(
    owner_root: Path,
    scope_roots: Sequence[Path],
    *,
    now: float,
    limits: RetentionLimits,
    build_control: bool = False,
) -> list[Candidate]:
    """The last-use roots of ``owner_root`` that the retention policy selects.

    With ``build_control`` it selects only build-control roots, for the pass
    that follows a GC's reclaimed targets.
    """
    roots: list[tuple[Path, CandidateKind]] = [
        (child, "checkout-state")
        for child in _checkout_state_dirs(owner_root)
        if not build_control or child.parent.name == "build-control"
    ]
    if not build_control:
        roots.extend(
            (child, "session-target") for child in session_target_dirs(owner_root)
        )
    return [
        Candidate(
            path=child,
            kind=kind,
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
    env = os.environ if env is None else env
    checkout = checkout.expanduser().resolve()
    resolved_root = resolve_root(root, env=env)
    scope_roots = reclaim_roots(resolved_root)
    cfg = GuardConfig.from_env(env)
    result = ReclaimResult(
        root=str(resolved_root),
        triggered=True,
        free_before=0,
        free_after=0,
        scope_roots=[str(path) for path in scope_roots],
        mode="apply" if apply else "dry-run",
    )
    protected = _protected_paths(scope_roots, env)
    component = checkout_component(checkout)
    retired_targets: set[Path] = set()
    for owner_root in scope_roots:
        # Build control follows its target, so the targets go first.
        children = sorted(
            _checkout_state_dirs(owner_root),
            key=lambda child: child.parent.name == "build-control",
        )
        for child in children:
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
            )
            allowed, reason = decide_owner_reclaim(candidate, protected=protected)
            if not allowed:
                result.skipped.append({"path": str(candidate.path), "reason": reason})
                continue
            assert_safe_to_delete(candidate.path, owner_root, cfg.lane_globs)
            if apply:
                ok, error = delete_path(candidate.path)
                if not ok:
                    result.errors.append(f"{candidate.path}: {error}")
                    continue
            result.reclaimed.append(
                {"path": str(candidate.path), "kind": "checkout-state"}
            )
            if owner.kind == "cargo-target":
                retired_targets.add(candidate.path)
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
    for name in ("CARGO_TARGET_DIR", "MOLT_DIFF_CARGO_TARGET_DIR"):
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
        path = log_dir / f"disk_guard-{stamp}-{os.getpid()}.json"
        path.write_text(json.dumps(result.to_dict(), indent=2), encoding="utf-8")
        return path
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
    verb = "reclaimed" if result.mode == "apply" else "would reclaim"
    print(
        f"disk_guard [{result.mode}] root={result.root} "
        f"free={_human(result.free_before)} triggered={result.triggered}"
    )
    for item in result.reclaimed:
        print(f"  {verb}: {item['path']} ({item['kind']})")
    if result.mode == "apply" and result.triggered:
        print(
            f"  free {_human(result.free_before)} -> {_human(result.free_after)} "
            f"(+{_human(max(0, result.reclaimed_bytes))})"
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
        "targets by last use (MOLT_SESSION_TARGET_IDLE_HOURS)",
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
    args = ap.parse_args(argv)

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
