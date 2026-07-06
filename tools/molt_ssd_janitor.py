#!/usr/bin/env python3
"""Reclaim space on the Molt artifact SSD (APDataStore / MOLT_EXT_ROOT) safely.

The external artifact root (``D:\\Molt`` on the Windows workstation,
``/Volumes/APDataStore/Molt`` on macOS) accumulates per-session cargo targets,
pytest temproots, uv locks, dated one-off scratch, orphaned worktrees, and
unbounded caches. Nothing swept these, so the volume grows without bound. This
janitor sweeps them by AGE, dry-run by default, scoped strictly to the artifact
root, and NEVER touches anything that is plausibly live:

  - registered git worktrees (via ``git worktree list``) are always protected;
  - the current session's dirs (``MOLT_SESSION_ID``) are protected;
  - anything modified within ``--min-idle-hours`` is protected (a running build
    touches its target dir continuously, so idle-age is a reliable liveness
    proxy);
  - sessions with a live memory-guard marker under ``tmp/memory_guard/active``
    are protected.

Classes (``--classes``, default = the auto-safe set; ``cargo`` and ``all`` are
opt-in because deleting a cargo target forces a rebuild):

  tmp        <root>/tmp/*                      idle > --tmp-age-days      (default 3)
  sessions   <root>/target/sessions/*          idle > --session-age-days (default 3)
  scratch    dated/one-off dirs (``*-YYYYMMDD*``, clippy-*, extrepro-*, ...)
                                               idle > --scratch-age-days (default 21)
  worktrees  ORPHANED (unregistered) dirs under <root>/worktrees/
                                               idle > --worktree-age-days(default 7)
  caches     .molt_cache/.sccache/.uv-cache/.pip-cache capped to --cache-cap-gb (LRU)
  cargo      <root>/cargo-target-* (flat legacy targets, NOT target/sessions)
                                               idle > --cargo-age-days   (default 30)

Nothing is deleted without ``--apply``. Every run appends a JSON record to
``<root>/logs/janitor/``. Use ``--watch --interval SECONDS`` for a periodic sweep.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

AUTO_SAFE_CLASSES = ("tmp", "sessions", "scratch", "worktrees", "caches")
ALL_CLASSES = (*AUTO_SAFE_CLASSES, "cargo")

CACHE_DIRNAMES = (".molt_cache", ".sccache", ".uv-cache", ".pip-cache")

# Dated / one-off scratch dirs created ad hoc by past sessions and never swept.
_SCRATCH_PATTERNS = (
    re.compile(r".*-20\d{6}([T-].*)?$"),      # anything stamped -YYYYMMDD[...]
    re.compile(r"clippy-.*"),
    re.compile(r"extrepro.*"),
    re.compile(r"ext-ci-repro.*"),
    re.compile(r"mix-build.*"),
    re.compile(r"diff(_.*)?$"),
    re.compile(r"fuzz_results$"),
    re.compile(r"generated_tests$"),
    re.compile(r"ci-logs$"),
)
# Never treat these as scratch even if a pattern matches — they are structural.
_SCRATCH_KEEP = {"target", "worktrees", "cargo-home", "bin", "logs", "tmp"}


def _eprint(*a: object) -> None:
    print(*a, file=sys.stderr, flush=True)


def _human(n: int) -> str:
    f = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if f < 1024 or unit == "TB":
            return f"{f:.1f}{unit}"
        f /= 1024
    return f"{f:.1f}TB"


@dataclass
class Candidate:
    path: Path
    kind: str
    reason: str
    mtime: float
    size: int = -1  # -1 = not yet measured

    @property
    def age_days(self) -> float:
        return max(0.0, (time.time() - self.mtime) / 86400.0)


@dataclass
class SweepPlan:
    root: Path
    candidates: list[Candidate] = field(default_factory=list)
    protected: list[tuple[Path, str]] = field(default_factory=list)


def _resolve_root(raw: str | None, *, force: bool) -> Path:
    """Resolve and SAFETY-CHECK the artifact root. Refuse dangerous targets."""
    if raw:
        root = Path(raw).expanduser()
    elif os.environ.get("MOLT_EXT_ROOT"):
        root = Path(os.environ["MOLT_EXT_ROOT"]).expanduser()
    else:
        root = _autodetect_root()
    if root is None:
        raise SystemExit(
            "molt_ssd_janitor: could not resolve the artifact root; pass --root "
            "or set MOLT_EXT_ROOT (e.g. D:\\Molt or /Volumes/APDataStore/Molt)."
        )
    root = root.resolve()
    if not root.is_dir():
        raise SystemExit(f"molt_ssd_janitor: root {root} is not a directory")
    # Refuse drive/filesystem roots and 1-component paths (C:\, /, D:\).
    if root.parent == root or len(root.parts) <= 1:
        raise SystemExit(f"molt_ssd_janitor: refusing to operate on root {root}")
    repo_root = REPO_ROOT.resolve()
    # Refuse the repo checkout itself or any path inside it. The artifact root
    # may contain this checkout as <root>/worktrees/<name>; that is the normal
    # APDataStore layout and must remain usable from linked worktrees.
    if root == repo_root or repo_root in root.parents:
        raise SystemExit(f"molt_ssd_janitor: refusing to operate inside the repo {root}")
    if root in repo_root.parents and not _repo_is_artifact_worktree_under_root(
        repo_root, root
    ):
        raise SystemExit(
            "molt_ssd_janitor: refusing parent of repo checkout that is not "
            f"an artifact worktree root: {root}"
        )
    # On Windows, refuse a C: root unless forced (artifacts must not live on C:).
    drive = root.drive.rstrip(":").upper()
    if os.name == "nt" and drive == "C" and not force:
        raise SystemExit(
            f"molt_ssd_janitor: refusing C:-drive root {root} (use --force to override)"
        )
    return root


def _repo_is_artifact_worktree_under_root(repo_root: Path, root: Path) -> bool:
    return (
        repo_root.parent.name == "worktrees"
        and repo_root.parent.parent == root
    )


def _autodetect_root() -> Path | None:
    if sys.platform == "darwin":
        cand = Path("/Volumes/APDataStore/Molt")
        return cand if cand.is_dir() else None
    if os.name == "nt":
        for letter in "DEFGHIJKLMNOPQRSTUVWXYZ":
            drive = Path(f"{letter}:\\")
            if _windows_volume_label(drive) == "APDataStore":
                cand = drive / "Molt"
                return cand if cand.is_dir() else None
    return None


def _windows_volume_label(drive_root: Path) -> str | None:
    if os.name != "nt":
        return None
    try:
        import ctypes

        label = ctypes.create_unicode_buffer(261)
        fs = ctypes.create_unicode_buffer(261)
        serial = ctypes.c_ulong()
        maxlen = ctypes.c_ulong()
        flags = ctypes.c_ulong()
        ok = ctypes.windll.kernel32.GetVolumeInformationW(
            str(drive_root), label, len(label), ctypes.byref(serial),
            ctypes.byref(maxlen), ctypes.byref(flags), fs, len(fs),
        )
    except (AttributeError, OSError, ValueError):
        return None
    return label.value if ok else None


def _registered_worktrees(root: Path) -> set[Path]:
    """Absolute paths of every git-registered worktree (always protected)."""
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO_ROOT), "worktree", "list", "--porcelain"],
            capture_output=True, text=True, timeout=60, check=False,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return set()
    paths: set[Path] = set()
    for line in out.splitlines():
        if line.startswith("worktree "):
            try:
                paths.add(Path(line[len("worktree "):].strip()).resolve())
            except OSError:
                pass
    return paths


def _active_guard_sessions(root: Path) -> set[str]:
    """Session ids with a live memory-guard active marker (protected)."""
    sessions: set[str] = set()
    active_dir = root / "tmp" / "memory_guard" / "active"
    try:
        entries = list(active_dir.iterdir())
    except OSError:
        return sessions
    for entry in entries:
        try:
            data = json.loads(entry.read_text("utf-8"))
        except (OSError, ValueError):
            continue
        for key in ("session_id", "MOLT_SESSION_ID", "session"):
            val = data.get(key) if isinstance(data, dict) else None
            if isinstance(val, str) and val:
                sessions.add(val)
    return sessions


def _dir_size(path: Path, deadline: float) -> tuple[int, bool]:
    """Recursive size via scandir; bail at ``deadline`` (returns complete=False)."""
    total = 0
    stack = [path]
    while stack:
        if time.monotonic() > deadline:
            return total, False
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
    return total, True


def _entry_mtime(path: Path) -> float:
    try:
        return path.stat().st_mtime
    except OSError:
        return 0.0


def _iter_dir(path: Path) -> list[Path]:
    try:
        return sorted(path.iterdir())
    except OSError:
        return []


def _gather(
    root: Path,
    *,
    classes: tuple[str, ...],
    ages: dict[str, float],
    min_idle_hours: float,
    cache_cap_gb: float,
    current_session: str | None,
    registered: set[Path],
    guard_sessions: set[str],
) -> SweepPlan:
    plan = SweepPlan(root=root)
    min_idle_s = min_idle_hours * 3600.0
    now = time.time()

    def too_fresh(mtime: float) -> bool:
        return (now - mtime) < min_idle_s

    def protect_or_add(path: Path, kind: str, reason: str, age_days: float) -> None:
        rp = path.resolve()
        if rp in registered:
            plan.protected.append((path, "registered git worktree"))
            return
        mtime = _entry_mtime(path)
        if too_fresh(mtime):
            plan.protected.append((path, f"modified <{min_idle_hours}h ago"))
            return
        if mtime and (now - mtime) / 86400.0 < age_days:
            return  # younger than the class threshold: silently keep
        plan.candidates.append(Candidate(path=path, kind=kind, reason=reason, mtime=mtime))

    if "tmp" in classes:
        for entry in _iter_dir(root / "tmp"):
            if entry.name in {"memory_guard"}:  # guard control-plane dir stays
                continue
            protect_or_add(entry, "tmp", "stale tmp entry", ages["tmp"])

    if "sessions" in classes:
        for entry in _iter_dir(root / "target" / "sessions"):
            if current_session and entry.name == current_session:
                plan.protected.append((entry, "current MOLT_SESSION_ID"))
                continue
            if entry.name in guard_sessions:
                plan.protected.append((entry, "live memory-guard session"))
                continue
            protect_or_add(entry, "sessions", "stale session target", ages["sessions"])

    if "scratch" in classes:
        for entry in _iter_dir(root):
            if not entry.is_dir() or entry.name in _SCRATCH_KEEP:
                continue
            if any(p.fullmatch(entry.name) for p in _SCRATCH_PATTERNS):
                protect_or_add(entry, "scratch", "dated/one-off scratch", ages["scratch"])

    if "worktrees" in classes:
        for entry in _iter_dir(root / "worktrees"):
            if not entry.is_dir():
                continue
            if entry.resolve() in registered:
                plan.protected.append((entry, "registered git worktree"))
                continue
            protect_or_add(entry, "worktrees", "orphaned (unregistered) worktree", ages["worktrees"])

    if "cargo" in classes:
        for entry in _iter_dir(root):
            if entry.is_dir() and entry.name.startswith("cargo-target-"):
                protect_or_add(entry, "cargo", "stale flat cargo target", ages["cargo"])

    if "caches" in classes:
        _gather_cache_overflow(root, cache_cap_gb, plan, too_fresh)

    return plan


def _gather_cache_overflow(root: Path, cap_gb: float, plan: SweepPlan, too_fresh) -> None:
    cap = int(cap_gb * 1024 ** 3)
    deadline = time.monotonic() + 30.0
    for name in CACHE_DIRNAMES:
        cache = root / name
        if not cache.is_dir():
            continue
        entries: list[tuple[Path, int, float]] = []
        for child in _iter_dir(cache):
            if child.is_dir() and not child.is_symlink():
                size, _ = _dir_size(child, deadline)
            else:
                try:
                    size = child.stat().st_size
                except OSError:
                    size = 0
            entries.append((child, size, _entry_mtime(child)))
        total = sum(s for _, s, _ in entries)
        if total <= cap:
            continue
        # Evict oldest-first until under cap.
        for child, size, mtime in sorted(entries, key=lambda t: t[2]):
            if total <= cap:
                break
            if too_fresh(mtime):
                continue
            c = Candidate(path=child, kind="caches", reason=f"{name} over {cap_gb}GB cap", mtime=mtime, size=size)
            plan.candidates.append(c)
            total -= size


def _measure(plan: SweepPlan, *, budget_s: float) -> None:
    deadline = time.monotonic() + budget_s
    for c in plan.candidates:
        if c.size >= 0:
            continue
        if time.monotonic() > deadline:
            c.size = 0  # leave unmeasured; report will note approximate
            continue
        if c.path.is_dir():
            c.size, _ = _dir_size(c.path, time.monotonic() + 20.0)
        else:
            try:
                c.size = c.path.stat().st_size
            except OSError:
                c.size = 0


def _delete(path: Path) -> tuple[bool, str]:
    try:
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path, ignore_errors=False)
        else:
            path.unlink(missing_ok=True)
        return True, ""
    except OSError as exc:
        return False, str(exc)


def _write_log(root: Path, record: dict) -> Path | None:
    log_dir = root / "logs" / "janitor"
    try:
        log_dir.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%dT%H%M%S", time.localtime())
        path = log_dir / f"janitor-{stamp}.json"
        path.write_text(json.dumps(record, indent=2), encoding="utf-8")
        return path
    except OSError:
        return None


def _run_once(args: argparse.Namespace) -> int:
    root = _resolve_root(args.root, force=args.force)
    classes = _select_classes(args)
    ages = {
        "tmp": args.tmp_age_days,
        "sessions": args.session_age_days,
        "scratch": args.scratch_age_days,
        "worktrees": args.worktree_age_days,
        "cargo": args.cargo_age_days,
    }
    free_before = shutil.disk_usage(root).free
    if args.free_below_gb and free_before > args.free_below_gb * 1024 ** 3:
        print(
            f"molt_ssd_janitor: {_human(free_before)} free on {root} "
            f"(>{args.free_below_gb}GB) — skipping sweep."
        )
        return 0

    current_session = os.environ.get("MOLT_SESSION_ID") or None
    plan = _gather(
        root,
        classes=classes,
        ages=ages,
        min_idle_hours=args.min_idle_hours,
        cache_cap_gb=args.cache_cap_gb,
        current_session=current_session,
        registered=_registered_worktrees(root),
        guard_sessions=_active_guard_sessions(root),
    )
    if not args.no_sizes:
        _measure(plan, budget_s=args.size_budget_s)

    mode = "APPLY" if args.apply else "DRY-RUN"
    print(f"molt_ssd_janitor [{mode}] root={root}  free={_human(free_before)}  "
          f"classes={','.join(classes)}")
    by_kind: dict[str, list[Candidate]] = {}
    for c in plan.candidates:
        by_kind.setdefault(c.kind, []).append(c)
    total_size = 0
    total_count = 0
    for kind in ALL_CLASSES:
        items = by_kind.get(kind, [])
        if not items:
            continue
        ksize = sum(max(0, c.size) for c in items)
        total_size += ksize
        total_count += len(items)
        print(f"  {kind:9s} {len(items):5d} entries  ~{_human(ksize):>9s}")
        for c in sorted(items, key=lambda c: -max(0, c.size))[: args.show]:
            print(f"      ~{_human(max(0,c.size)):>9s}  {c.age_days:5.0f}d  {c.path.name}  ({c.reason})")
        if len(items) > args.show:
            print(f"      ... and {len(items) - args.show} more")
    print(f"  TOTAL     {total_count:5d} entries  ~{_human(total_size):>9s} reclaimable")

    deleted = 0
    reclaimed = 0
    errors: list[str] = []
    if args.apply and plan.candidates:
        for c in plan.candidates:
            ok, err = _delete(c.path)
            if ok:
                deleted += 1
                reclaimed += max(0, c.size)
            else:
                errors.append(f"{c.path}: {err}")
        free_after = shutil.disk_usage(root).free
        print(f"  APPLIED: deleted {deleted}/{total_count}, "
              f"free {_human(free_before)} -> {_human(free_after)} "
              f"(+{_human(free_after - free_before)})")
        for e in errors[:10]:
            _eprint(f"  ERROR {e}")

    record = {
        "schema_version": 1,
        "root": str(root),
        "mode": mode,
        "classes": list(classes),
        "free_before": free_before,
        "candidate_count": total_count,
        "candidate_bytes": total_size,
        "deleted": deleted,
        "reclaimed_bytes": reclaimed,
        "errors": errors,
        "protected_sample": [[str(p), why] for p, why in plan.protected[:20]],
    }
    log_path = _write_log(root, record)
    if log_path:
        print(f"  log: {log_path}")
    return 1 if errors else 0


def _select_classes(args: argparse.Namespace) -> tuple[str, ...]:
    if args.all:
        return ALL_CLASSES
    if args.classes:
        chosen = tuple(c.strip() for c in args.classes.split(",") if c.strip())
        bad = [c for c in chosen if c not in ALL_CLASSES]
        if bad:
            raise SystemExit(f"molt_ssd_janitor: unknown class(es) {bad}; valid: {ALL_CLASSES}")
        return chosen
    return AUTO_SAFE_CLASSES


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", help="artifact root (default: $MOLT_EXT_ROOT or autodetect APDataStore)")
    ap.add_argument("--apply", action="store_true", help="delete (default: dry-run report only)")
    ap.add_argument("--classes", help=f"comma list of {ALL_CLASSES}; default = auto-safe {AUTO_SAFE_CLASSES}")
    ap.add_argument("--all", action="store_true", help="all classes including cargo")
    ap.add_argument("--force", action="store_true", help="allow a C:-drive root (discouraged)")
    ap.add_argument("--tmp-age-days", type=float, default=3.0)
    ap.add_argument("--session-age-days", type=float, default=3.0)
    ap.add_argument("--scratch-age-days", type=float, default=21.0)
    ap.add_argument("--worktree-age-days", type=float, default=7.0)
    ap.add_argument("--cargo-age-days", type=float, default=30.0)
    ap.add_argument("--min-idle-hours", type=float, default=6.0,
                    help="never delete anything modified within this many hours")
    ap.add_argument("--cache-cap-gb", type=float, default=30.0)
    ap.add_argument("--free-below-gb", type=float, default=0.0,
                    help="only sweep when free space is below this many GB (0 = always)")
    ap.add_argument("--no-sizes", action="store_true", help="skip size measurement (faster)")
    ap.add_argument("--size-budget-s", type=float, default=120.0)
    ap.add_argument("--show", type=int, default=8, help="entries to list per class")
    ap.add_argument("--watch", action="store_true", help="loop forever, sweeping every --interval")
    ap.add_argument("--interval", type=float, default=86400.0, help="--watch sweep interval seconds")
    args = ap.parse_args(argv)

    if not args.watch:
        return _run_once(args)
    while True:
        try:
            _run_once(args)
        except SystemExit as exc:
            if exc.code:
                _eprint(f"molt_ssd_janitor: {exc}")
        time.sleep(max(60.0, args.interval))


if __name__ == "__main__":
    raise SystemExit(main())
