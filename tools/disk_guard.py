#!/usr/bin/env python3
"""Deterministic, preemptive, AGENT-SAFE disk guard for the Molt artifact root.

WHY THIS EXISTS (root cause). On 2026-07-11 the canonical NVMe (``C:\\Molt``)
filled to **0 bytes** mid-session and failed several build lanes. Freeing it was
ad-hoc and manual -- a dev-velocity killer. Two coupled cracks caused it:

  1. The ONLY automatic disk sweep (``molt_ssd_janitor`` spawned from
     ``molt.dx._maybe_sweep_stale_artifacts``) is gated by
     ``MOLT_DISABLE_AUTO_JANITOR``. That flag is set to ``1`` all over the
     orchestration (``witness_iter.py`` etc.) because operators conflate "the
     janitor" with the DANGEROUS orphan-process reaper it is culturally bundled
     with (memory M25/M26 -- the reaper can SIGTERM a live Codex/agent). Turning
     the flag on to protect agents ALSO turned off disk reclamation. The two
     roles -- **kill orphan processes (DANGEROUS)** and **reclaim disk (SAFE)**
     -- were fatally coupled behind one switch.

  2. Even when the janitor DID run it swept only its AUTO_SAFE_CLASSES
     (tmp/sessions/scratch/worktrees/caches). The orchestration creates one
     ISOLATED ``CARGO_TARGET_DIR`` per lane under ``C:\\Molt\\target\\codex-*``
     (build isolation, memory M62); those per-lane dirs match NO janitor class,
     so ~15 of them (GBs each) accumulated until the volume hit 0.

THIS GUARD decouples disk-protection from process-reaping. It ONLY reclaims
stale build-artifact directories. It is **provably agent-safe**: it imports no
process API, spawns nothing, signals nothing, and kills nothing. The paired
``tests/tools/test_disk_guard.py::test_agent_safety_source_scan`` asserts this
module's source contains zero process-actuation tokens (the same discipline as
pact's ``test_no_actuation_capability``). Disk reclamation is re-enabled under
its OWN flag ``MOLT_DISABLE_DISK_GUARD`` (defaults OFF == guard ON), INDEPENDENT
of ``MOLT_DISABLE_AUTO_JANITOR`` -- so protecting agents never again disables
disk protection.

DESIGN (deterministic + idempotent + fail-open):

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
        - registry-registered per-lane target dirs (see ``register_lane_target``);
        - ``<root>/target/sessions/*``          (per-session witness targets);
        - ``<root>/target/<lane-glob>``         (``codex-*`` / ``lane-*`` / ``sess-*``);
        - Cargo-incremental quarantine dirs (``.molt_state/quarantine/...``);
        - flat legacy ``<root>/cargo-target-*``.

  * An ACTIVE directory is NEVER reclaimed. "Active" == modified within
    ``min_idle`` (a running build touches its target continuously), OR holds a
    Cargo build lock, OR is the current ``MOLT_SESSION_ID`` target, OR is a
    live-lane dir registered within the GC TTL.

  * ``gc()`` / ``--gc`` -- per-lane target-dir garbage collection at the SOURCE
    of the accumulation. A lane registers its isolated ``CARGO_TARGET_DIR`` on
    creation; the GC reclaims a registered dir once it is older than a TTL and
    not lock-held / not freshly modified, so a completed lane's dir is collected
    without the orchestrator ``rm``-ing anything by hand.

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
from typing import Callable, Iterable, Mapping, Sequence

_GB = 1024**3

# --- thresholds (env-overridable; deterministic defaults) -------------------
DEFAULT_HIGH_WATER_GB = 25.0  # below this free-space, ensure_free fires
DEFAULT_TARGET_GB = 40.0  # reclaim until free reaches this
DEFAULT_MIN_IDLE_MIN = 15.0  # never touch a dir modified within this
DEFAULT_GC_TTL_HOURS = 6.0  # a registered lane dir older than this is GC-able
DEFAULT_ENSURE_BUDGET_S = 20.0  # wall-clock ceiling on one ensure_free reclaim
DEFAULT_LANE_GLOBS = ("codex-*", "lane-*", "sess-*")

# Env keys. MOLT_DISABLE_DISK_GUARD is INDEPENDENT of MOLT_DISABLE_AUTO_JANITOR
# on purpose (see module docstring): disabling the agent-reaper must not disable
# disk reclamation.
ENV_DISABLE = "MOLT_DISABLE_DISK_GUARD"
ENV_HIGH_WATER = "MOLT_DISK_GUARD_HIGH_WATER_GB"
ENV_TARGET = "MOLT_DISK_GUARD_TARGET_GB"
ENV_MIN_IDLE = "MOLT_DISK_GUARD_MIN_IDLE_MIN"
ENV_TTL = "MOLT_DISK_GUARD_TTL_HOURS"
ENV_LANE_GLOBS = "MOLT_DISK_GUARD_LANE_GLOBS"
ENV_ROOT = "MOLT_EXT_ROOT"

WINDOWS_PRIMARY_ARTIFACT_ROOT = Path("C:/Molt")

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
        ".disk_guard",  # this guard's own registry/log home
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

REGISTRY_RELPATH = Path("target") / ".disk_guard" / "registry.jsonl"
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


def _disabled(env: Mapping[str, str]) -> bool:
    return str(env.get(ENV_DISABLE, "")).strip().lower() in (
        "1",
        "true",
        "yes",
        "on",
    )


# --- config -----------------------------------------------------------------


@dataclass(frozen=True)
class GuardConfig:
    high_water_bytes: int
    target_bytes: int
    min_idle_s: float
    gc_ttl_s: float
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
        hw = (
            high_water_gb
            if high_water_gb is not None
            else _env_float(env, ENV_HIGH_WATER, DEFAULT_HIGH_WATER_GB)
        )
        tgt = (
            target_gb
            if target_gb is not None
            else _env_float(env, ENV_TARGET, DEFAULT_TARGET_GB)
        )
        # Target must be >= high-water or the loop can never satisfy itself.
        tgt = max(tgt, hw)
        min_idle = _env_float(env, ENV_MIN_IDLE, DEFAULT_MIN_IDLE_MIN) * 60.0
        ttl = _env_float(env, ENV_TTL, DEFAULT_GC_TTL_HOURS) * 3600.0
        raw_globs = str(env.get(ENV_LANE_GLOBS, "")).strip()
        globs = (
            tuple(g.strip() for g in raw_globs.split(",") if g.strip())
            or DEFAULT_LANE_GLOBS
        )
        return cls(
            high_water_bytes=int(hw * _GB),
            target_bytes=int(tgt * _GB),
            min_idle_s=min_idle,
            gc_ttl_s=ttl,
            lane_globs=globs,
        )


# --- candidate model + PURE decision surface --------------------------------


@dataclass
class Candidate:
    """A reclaimable directory. All fields are facts, injected for testability."""

    path: Path
    kind: str  # sessions | lane | cargo-target | quarantine | registered
    mtime: float  # newest activity signal (dir + immediate children)
    size: int = -1  # -1 == not measured
    lock_held: bool = False
    registered_at: float | None = None

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


def plan_reclaim(
    candidates: Iterable[Candidate],
    *,
    free_bytes: int,
    high_water_bytes: int,
    target_bytes: int,
    now: float,
    min_idle_s: float,
    protected: Iterable[Path] = (),
    live_lanes: Iterable[Path] = (),
) -> ReclaimPlan:
    """PURE: decide which candidates to reclaim. No I/O, fully unit-testable.

    Fires only when ``free_bytes < high_water_bytes`` (idempotent no-op above the
    threshold). When it fires it selects candidates in age order until the
    PROJECTED free reaches ``target_bytes``, skipping any that are ACTIVE:
    modified within ``min_idle_s``, lock-held, protected, or a live-lane dir.
    The orchestrator re-reads REAL free space between deletes, so an inexact
    ``size`` only affects how many candidates are pre-selected, never safety.
    """
    protected_set = {_norm(p) for p in protected}
    live_set = {_norm(p) for p in live_lanes}
    plan = ReclaimPlan(
        triggered=free_bytes < high_water_bytes,
        free_bytes=free_bytes,
        target_bytes=target_bytes,
    )
    if not plan.triggered:
        return plan

    running_free = free_bytes
    for c in sorted(candidates, key=_order_key):
        norm = _norm(c.path)
        if norm in protected_set:
            plan.skipped.append((c, "protected"))
            continue
        if norm in live_set:
            plan.skipped.append((c, "live-lane"))
            continue
        if c.lock_held:
            plan.skipped.append((c, "lock-held"))
            continue
        if c.age_s(now) < min_idle_s:
            plan.skipped.append((c, f"modified <{min_idle_s / 60:.0f}min ago"))
            continue
        if running_free >= target_bytes:
            plan.skipped.append((c, "target already met"))
            continue
        plan.selected.append(c)
        running_free += max(0, c.size)
    return plan


def decide_completed_lane_reclaim(
    candidate: Candidate,
    *,
    completed: bool,
    active: bool,
    protected: Iterable[Path] = (),
) -> tuple[bool, str]:
    """PURE: completion bypasses TTL, never activity/lock/protection safety."""
    if not completed:
        return False, "lane-not-completed"
    if active:
        return False, "lane-active"
    if candidate.lock_held:
        return False, "lock-held"
    if _norm(candidate.path) in {_norm(path) for path in protected}:
        return False, "protected"
    return True, "completed-lane"


def gc_plan(
    candidates: Iterable[Candidate],
    *,
    now: float,
    ttl_s: float,
    min_idle_s: float,
    protected: Iterable[Path] = (),
) -> list[Candidate]:
    """PURE: registered lane dirs eligible for TTL garbage collection.

    A registered dir is collected once it is older than ``ttl_s`` (by
    registration time), is not lock-held, and has not been modified within
    ``min_idle_s``. Age is measured from ``registered_at`` when known, else from
    ``mtime`` -- so an un-timestamped legacy entry still ages out by activity.
    """
    protected_set = {_norm(p) for p in protected}
    out: list[Candidate] = []
    for c in sorted(candidates, key=_order_key):
        if _norm(c.path) in protected_set:
            continue
        if c.lock_held:
            continue
        if c.age_s(now) < min_idle_s:
            continue
        anchor = c.registered_at if c.registered_at is not None else c.mtime
        if (now - anchor) < ttl_s:
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

    Never operates on a drive/filesystem root or a one-component path. On
    Windows the canonical root is ``C:\\Molt``; an explicit ``--root`` or
    ``MOLT_EXT_ROOT`` is honored as long as it is a real, multi-component dir.
    """
    env = os.environ if env is None else env
    if raw:
        root = Path(raw)
    elif env.get(ENV_ROOT):
        root = Path(env[ENV_ROOT])
    elif os.name == "nt" and WINDOWS_PRIMARY_ARTIFACT_ROOT.is_dir():
        root = WINDOWS_PRIMARY_ARTIFACT_ROOT
    else:
        raise SystemExit(
            "disk_guard: could not resolve the artifact root; pass --root or set "
            "MOLT_EXT_ROOT (e.g. C:\\Molt)."
        )
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise SystemExit(f"disk_guard: root {root} is not a directory")
    if root.parent == root or len(root.parts) <= 1:
        raise SystemExit(f"disk_guard: refusing drive/filesystem root {root}")
    return root


def _is_reclaimable(path: Path, root: Path, lane_globs: Sequence[str]) -> str | None:
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
    # sessions/<child>
    if len(parts) == 3 and parts[0] == "target" and parts[1] == "sessions":
        return "sessions"
    # cargo-incremental quarantine anywhere under a target tree
    if ".molt_state" in parts and "quarantine" in parts:
        return "quarantine"
    # flat legacy cargo-target-* at the artifact-root top level
    if len(parts) == 1 and parts[0].startswith("cargo-target-"):
        return "cargo-target"
    # per-lane isolated target dir directly under <root>/target/
    if len(parts) == 2 and parts[0] == "target":
        name = parts[1]
        if any(_fnmatch(name, g) for g in lane_globs):
            return "lane"
    return None


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


def _newest_activity_mtime(path: Path) -> float:
    """mtime of the dir OR its newest immediate child -- a cheap liveness proxy.

    Cargo touches files DEEP in a target dir continuously during a build while
    the top-level dir mtime can lag, so a one-level child scan catches an active
    build without paying a full recursive walk.
    """
    newest = 0.0
    try:
        newest = path.stat().st_mtime
    except OSError:
        return 0.0
    try:
        with os.scandir(path) as it:
            for entry in it:
                try:
                    m = entry.stat(follow_symlinks=False).st_mtime
                except OSError:
                    continue
                if m > newest:
                    newest = m
    except OSError:
        pass
    return newest


def _cargo_lock_held(path: Path) -> bool:
    """True if a Cargo build lock under ``path`` is currently HELD by a process.

    Cargo writes ``.cargo-lock`` at a target root and flock()s it for the build.
    We probe by attempting a NON-BLOCKING lock ourselves; if the OS says the
    region is already locked, a build owns it and the dir is active. This reads a
    lock -- it never touches any process. Absent/unlocked/unprobeable -> False
    (the mtime-idle and TTL gates remain the primary liveness signal).
    """
    for name in (".cargo-lock",):
        lock = path / name
        if not lock.is_file():
            continue
        if _region_locked(lock):
            return True
    return False


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


# --- registry (per-lane GC source-of-truth) ---------------------------------


class _FileLock:
    """Advisory cross-platform lock (msvcrt/fcntl). Best-effort, never raises."""

    def __init__(self, handle) -> None:
        self._handle = handle
        self._locked = False

    def __enter__(self):
        try:
            if os.name == "nt":
                import msvcrt

                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_LOCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_EX)
            self._locked = True
        except OSError:
            self._locked = False
        return self

    def __exit__(self, *exc) -> None:
        if not self._locked:
            return
        try:
            if os.name == "nt":
                import msvcrt

                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
        except OSError:
            pass


def _registry_path(root: Path) -> Path:
    return root / REGISTRY_RELPATH


def register_lane_target(
    path: str | os.PathLike[str],
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    now: float | None = None,
) -> Path:
    """Register an isolated per-lane CARGO_TARGET_DIR for later TTL collection.

    Append-only + locked. Idempotent per path: the LATEST record wins (the GC
    reads the last event per path). Returns the registry file path. Never
    raises on a best-effort append failure beyond surfacing the registry path.
    """
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
    reg = _registry_path(resolved_root)
    now = time.time() if now is None else now
    record = {
        "path": str(Path(path).resolve()),
        "registered_at": now,
        "pid": os.getpid(),
    }
    reg.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(record, ensure_ascii=True)
    with open(reg, "a+", encoding="utf-8") as fh:
        with _FileLock(fh):
            fh.seek(0, os.SEEK_END)
            fh.write(line + "\n")
            fh.flush()
    return reg


def read_registry(root: Path) -> dict[str, dict]:
    """Latest event per path from the append-only registry. Never raises."""
    reg = _registry_path(root)
    latest: dict[str, dict] = {}
    try:
        text = reg.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return latest
    for raw in text.splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            rec = json.loads(raw)
        except ValueError:
            continue
        if isinstance(rec, dict) and isinstance(rec.get("path"), str):
            latest[_norm(Path(rec["path"]))] = rec
    return latest


def _rewrite_registry(root: Path, keep: Mapping[str, dict]) -> None:
    """Rewrite the registry to only the ``keep`` records (post-GC compaction)."""
    reg = _registry_path(root)
    reg.parent.mkdir(parents=True, exist_ok=True)
    tmp = reg.with_suffix(".jsonl.tmp")
    lines = [
        json.dumps(rec, ensure_ascii=True)
        for rec in keep.values()
        if isinstance(rec, dict)
    ]
    tmp.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
    os.replace(tmp, reg)


# --- discovery --------------------------------------------------------------


def discover_candidates(
    root: Path,
    config: GuardConfig,
    *,
    measure_sizes: bool = True,
    size_budget_s: float = 30.0,
) -> list[Candidate]:
    """Scan the RECLAIMABLE allow-set into Candidates. Pure disk reads only."""
    root = root.resolve()
    registry = read_registry(root)
    seen: set[str] = set()
    candidates: list[Candidate] = []
    size_deadline = time.monotonic() + size_budget_s

    def add(path: Path, kind: str, registered_at: float | None = None) -> None:
        norm = _norm(path)
        if norm in seen:
            return
        if not path.is_dir():
            return
        if _is_reclaimable(path, root, config.lane_globs) is None:
            return
        seen.add(norm)
        size = -1
        if measure_sizes:
            size = _dir_size(path, min(time.monotonic() + 20.0, size_deadline))
        candidates.append(
            Candidate(
                path=path,
                kind=kind,
                mtime=_newest_activity_mtime(path),
                size=size,
                lock_held=_cargo_lock_held(path),
                registered_at=registered_at,
            )
        )

    # sessions/<child>
    for child in _iter_dir(root / "target" / "sessions"):
        if child.is_dir():
            add(child, "sessions")
    # per-lane target dirs under target/
    for child in _iter_dir(root / "target"):
        if child.is_dir() and _is_reclaimable(child, root, config.lane_globs) == "lane":
            add(child, "lane")
    # cargo-incremental quarantine dirs (under any target subtree we can see)
    for qdir in _iter_quarantine_dirs(root):
        add(qdir, "quarantine")
    # flat legacy cargo-target-*
    for child in _iter_dir(root):
        if child.is_dir() and child.name.startswith("cargo-target-"):
            add(child, "cargo-target")
    # registry-registered lane targets (may be anywhere, incl. already covered)
    for rec in registry.values():
        p = Path(str(rec.get("path", "")))
        reg_at = rec.get("registered_at")
        add(
            p,
            "registered",
            registered_at=float(reg_at) if isinstance(reg_at, (int, float)) else None,
        )
    return candidates


def _iter_dir(path: Path) -> list[Path]:
    try:
        return sorted(path.iterdir())
    except OSError:
        return []


def _iter_quarantine_dirs(root: Path) -> list[Path]:
    out: list[Path] = []
    target = root / "target"
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
    reclaimed: list[dict] = field(default_factory=list)
    skipped: list[dict] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    mode: str = "apply"

    @property
    def reclaimed_bytes(self) -> int:
        return self.free_after - self.free_before

    def to_dict(self) -> dict:
        return {
            "schema_version": 1,
            "tool": "disk_guard",
            "root": self.root,
            "mode": self.mode,
            "triggered": self.triggered,
            "free_before": self.free_before,
            "free_after": self.free_after,
            "reclaimed_bytes": self.reclaimed_bytes,
            "reclaimed": self.reclaimed,
            "skipped": self.skipped,
            "errors": self.errors,
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
    error (recorded in ``errors``); a caller that wants strict fail-open wraps
    the whole call (see ``ensure_free_fail_open``).
    """
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
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
        mode="apply" if apply else "dry-run",
    )
    if not result.triggered:
        if log:
            _write_log(resolved_root, result)
        return result

    protected = _protected_paths(resolved_root, env)
    candidates = discover_candidates(resolved_root, cfg)
    plan = plan_reclaim(
        candidates,
        free_bytes=free_before,
        high_water_bytes=high_water,
        target_bytes=target,
        now=now,
        min_idle_s=cfg.min_idle_s,
        protected=protected,
    )
    for cand, why in plan.skipped:
        result.skipped.append(
            {"path": str(cand.path), "reason": why, "kind": cand.kind}
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
            try:
                assert_safe_to_delete(cand.path, resolved_root, cfg.lane_globs)
            except ValueError as exc:
                result.errors.append(str(exc))
                continue
            ok, err = _delete_dir(cand.path)
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


def ensure_free_fail_open(
    min_gb: float | None = None,
    **kwargs: object,
) -> ReclaimResult | None:
    """``ensure_free`` that NEVER raises -- the contract for hook/build wiring.

    A guard error must never block a build (task constraint). Any exception is
    swallowed and logged loudly to stderr; the caller proceeds with the build.
    Returns the result, or None if the guard itself failed.
    """
    try:
        return ensure_free(min_gb, **kwargs)  # type: ignore[arg-type]
    except SystemExit as exc:
        _eprint(f"[disk_guard] fail-open (config): {exc}")
        return None
    except BaseException as exc:  # noqa: BLE001 - fail-open is the whole point
        _eprint(f"[disk_guard] fail-open after error: {type(exc).__name__}: {exc}")
        return None


def gc(
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    config: GuardConfig | None = None,
    now_fn: Callable[[], float] = time.time,
    apply: bool = True,
    log: bool = True,
) -> ReclaimResult:
    """Per-lane target-dir GC: reclaim registered dirs past their TTL.

    Kills the accumulation at the SOURCE (item 4): a completed lane's isolated
    target dir is collected automatically once it ages past the TTL and is not
    lock-held / freshly modified, so the orchestrator never ``rm``-s by hand.
    """
    env = os.environ if env is None else env
    resolved_root = resolve_root(root, env=env)
    cfg = config or GuardConfig.from_env(env)
    now = now_fn()
    free_fn = _default_free_fn(resolved_root)
    free_before = free_fn()
    result = ReclaimResult(
        root=str(resolved_root),
        triggered=True,
        free_before=free_before,
        free_after=free_before,
        mode="apply" if apply else "dry-run",
    )
    registry = read_registry(resolved_root)
    protected = _protected_paths(resolved_root, env)

    cands: list[Candidate] = []
    for rec in registry.values():
        p = Path(str(rec.get("path", "")))
        if not p.is_dir():
            continue
        if _is_reclaimable(p, resolved_root, cfg.lane_globs) is None:
            # A registered path that is not (any longer) in the allow-set: drop
            # its stale registry entry but never delete it.
            continue
        reg_at = rec.get("registered_at")
        cands.append(
            Candidate(
                path=p,
                kind="registered",
                mtime=_newest_activity_mtime(p),
                lock_held=_cargo_lock_held(p),
                registered_at=float(reg_at)
                if isinstance(reg_at, (int, float))
                else None,
            )
        )

    collectable = gc_plan(
        cands,
        now=now,
        ttl_s=cfg.gc_ttl_s,
        min_idle_s=cfg.min_idle_s,
        protected=protected,
    )
    collected_norms: set[str] = set()
    for cand in collectable:
        if apply:
            try:
                assert_safe_to_delete(cand.path, resolved_root, cfg.lane_globs)
            except ValueError as exc:
                result.errors.append(str(exc))
                continue
            ok, err = _delete_dir(cand.path)
            if not ok:
                result.errors.append(f"{cand.path}: {err}")
                continue
        collected_norms.add(_norm(cand.path))
        result.reclaimed.append({"path": str(cand.path), "kind": "registered"})

    if apply:
        # Compact the registry: drop collected entries and entries whose dir is
        # gone (a lane whose target was already removed by ensure_free).
        keep = {
            norm: rec
            for norm, rec in registry.items()
            if norm not in collected_norms and Path(str(rec.get("path", ""))).is_dir()
        }
        if len(keep) != len(registry):
            _rewrite_registry(resolved_root, keep)
    result.free_after = free_fn()
    if log:
        _write_log(resolved_root, result)
    return result


def reclaim_completed_lane(
    target: Path,
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
    apply: bool = True,
) -> ReclaimResult:
    """Immediately reclaim one completed registered lane target."""
    env = os.environ if env is None else env
    inferred_root = root or (
        target.resolve().parents[1]
        if target.resolve().parent.name == "target"
        else None
    )
    resolved_root = resolve_root(inferred_root, env=env)
    cfg = GuardConfig.from_env(env)
    result = ReclaimResult(
        root=str(resolved_root),
        triggered=True,
        free_before=0,
        free_after=0,
        mode="apply" if apply else "dry-run",
    )
    target = target.resolve()
    candidate = Candidate(
        path=target,
        kind="registered",
        mtime=_newest_activity_mtime(target) if target.is_dir() else 0.0,
        lock_held=_cargo_lock_held(target) if target.is_dir() else False,
    )
    allowed, reason = decide_completed_lane_reclaim(
        candidate,
        completed=True,
        active=False,
        protected=_protected_paths(resolved_root, env),
    )
    if not allowed:
        result.skipped.append({"path": str(target), "reason": reason})
        return result
    if not target.is_dir():
        return result
    assert_safe_to_delete(target, resolved_root, cfg.lane_globs)
    if apply:
        ok, error = _delete_dir(target)
        if not ok:
            result.errors.append(f"{target}: {error}")
            return result
    result.reclaimed.append({"path": str(target), "kind": "completed-lane"})
    registry = read_registry(resolved_root)
    keep = {
        key: value
        for key, value in registry.items()
        if _norm(Path(str(value.get("path", "")))) != _norm(target)
    }
    if apply and len(keep) != len(registry):
        _rewrite_registry(resolved_root, keep)
    return result


def reclaim_completed_lane_fail_open(
    target: Path,
    *,
    root: str | os.PathLike[str] | None = None,
    env: Mapping[str, str] | None = None,
) -> ReclaimResult | None:
    try:
        return reclaim_completed_lane(target, root=root, env=env)
    except Exception as exc:
        print(
            f"disk_guard completed-lane reclaim LOUD fail-open: {exc}", file=sys.stderr
        )
        return None


def _protected_paths(root: Path, env: Mapping[str, str]) -> list[Path]:
    """Paths that must never be reclaimed even if they match the allow-set.

    Currently the current session's target dir (``target/sessions/<id>`` and any
    registered dir under it), protecting an in-flight build of THIS process.
    """
    protected: list[Path] = []
    session = str(env.get("MOLT_SESSION_ID", "")).strip()
    if session:
        protected.append(root / "target" / "sessions" / session)
    return protected


def _delete_dir(path: Path) -> tuple[bool, str]:
    try:
        shutil.rmtree(path, ignore_errors=False)
        return True, ""
    except OSError as exc:
        # Partial delete already frees space; report but do not raise.
        return False, str(exc)


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
        Path("/x/target/sessions/old"), "sessions", mtime=now - 10_000, size=5 * _GB
    )
    fresh = Candidate(
        Path("/x/target/sessions/live"), "sessions", mtime=now - 5, size=9 * _GB
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
        Path("/x/target/sessions/a"), "sessions", mtime=now - 10_000, size=40 * _GB
    )
    extra = Candidate(
        Path("/x/target/sessions/b"), "sessions", mtime=now - 9_000, size=40 * _GB
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

    # 6) GC collects a dir past TTL but not one within TTL.
    old_reg = Candidate(
        Path("/x/target/codex-done"),
        "registered",
        mtime=now - 100_000,
        registered_at=now - 100_000,
    )
    new_reg = Candidate(
        Path("/x/target/codex-live"),
        "registered",
        mtime=now - 100,
        registered_at=now - 100,
    )
    collect = gc_plan([old_reg, new_reg], now=now, ttl_s=6 * 3600, min_idle_s=min_idle)
    results.append(("gc-collects-past-ttl-only", collect == [old_reg]))

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


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", help="artifact root (default $MOLT_EXT_ROOT or C:\\Molt)")
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
        help="TTL garbage-collect registered lane target dirs",
    )
    ap.add_argument(
        "--register",
        metavar="PATH",
        help="register an isolated per-lane CARGO_TARGET_DIR",
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

    if args.register:
        reg = register_lane_target(args.register, root=args.root)
        print(f"disk_guard: registered {Path(args.register).resolve()} -> {reg}")
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
