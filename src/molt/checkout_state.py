"""Per-checkout state roots, their owners, their last use and their retirement.

Under a shared artifact root, a checkout's Cargo target, uv environments,
pytest cache and Hypothesis home carry ``molt.dx.checkout_component``, a hash
of the checkout path, in their names, and each Cargo target owns a build-control
root named by ``molt.build_state_layout.build_state_address``. The names
isolate the state, but a hash cannot be turned back into a path and says
nothing about use. This module owns where that state lives
(`CHECKOUT_STATE_CONTAINERS`), the projection of one checkout's roots
(`checkout_state_roots`), the owner record and last-use stamp inside each root,
and the retirement policy (`checkout_state_decision`) that `tools/disk_guard.py`
applies. The policy follows Go's build-cache trim and Cargo's global-cache GC:
retire by last use, and soon after the owner is gone (design 56, section 2.6).
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path
import re
import time
from types import MappingProxyType
from typing import Literal, get_args

from molt.custody_layout import SCRATCH_DIRNAME
from molt.exact_json import ExactJsonError, read_exact, write_exact

OWNER_RECORD = ".molt-state-owner.json"
SCHEMA = "molt.state-root-owner.v1"
_MAX_RECORD_BYTES = 8192

# A root's last use is its newest activity, which includes this stamp. A user
# refreshes it at most once per resolution, as Go updates a cache entry's
# mtime at most once an hour.
LAST_USE_STAMP = ".molt-last-use"
USE_RESOLUTION_S = 3600.0
# No root used within this window is retired by the GC or by disk pressure;
# it exceeds the stamp's resolution, so a live user's stamp is always newer.
PROTECTION_WINDOW_S = 2 * USE_RESOLUTION_S
MAX_AGE_ENV = "MOLT_CHECKOUT_STATE_MAX_AGE_DAYS"
# Cargo's default for data it can recreate locally.
DEFAULT_MAX_AGE_DAYS = 30.0
# The automatic GC runs at most once per interval per artifact root.
GC_INTERVAL_S = 24 * 3600.0
GC_STAMP = ("target", ".molt-checkout-state-gc")
# `molt.dx.checkout_component`: a sanitized name prefix and 12 hex digits.
_COMPONENT = re.compile(r"[A-Za-z0-9_.-]{1,24}-[0-9a-f]{12}")
# `molt.build_state_layout.build_state_address`: a SHA-256 digest.
_ADDRESS = re.compile(r"[0-9a-f]{64}")

CheckoutStateKind = Literal[
    "cargo-target",
    "uv-environment",
    "pytest-cache",
    "hypothesis-home",
    "build-control",
]
_KINDS: frozenset[str] = frozenset(get_args(CheckoutStateKind))

# Each kind's container, relative to an artifact root. The selectors in
# molt.dx, molt.build_state_layout and the pytest bootstrap place the state;
# tests bind them to this table. A uv environment's name ends with
# ``__src-<component>``, a build-control root is named by its target's address,
# and every other root is named by the component itself.
CHECKOUT_STATE_CONTAINERS: Mapping[CheckoutStateKind, tuple[str, ...]] = (
    MappingProxyType(
        {
            "cargo-target": ("target", "checkouts"),
            "uv-environment": ("uv-project-envs",),
            "pytest-cache": (SCRATCH_DIRNAME, "pytest-cache"),
            "hypothesis-home": (SCRATCH_DIRNAME, "hypothesis"),
            "build-control": (SCRATCH_DIRNAME, "build-control"),
        }
    )
)


@dataclass(frozen=True, slots=True)
class CheckoutOwner:
    """The owner a root's record names: its checkout, or for build control its
    Cargo target."""

    path: Path
    kind: CheckoutStateKind


@dataclass(frozen=True, slots=True)
class CheckoutStateRoot:
    path: Path
    kind: CheckoutStateKind
    owner: Path


def _name_key(kind: CheckoutStateKind, name: str) -> str | None:
    """The owner key a root's name carries, or None for a foreign name."""
    if kind == "build-control":
        return name if _ADDRESS.fullmatch(name) else None
    if kind == "uv-environment":
        head, separator, name = name.rpartition("__src-")
        if not separator or not head:
            return None
    return name if _COMPONENT.fullmatch(name) else None


def _owner_key(kind: CheckoutStateKind, owner: Path) -> str:
    """The key that owner ``owner`` gives the name of a root of ``kind``."""
    if kind == "build-control":
        from molt.build_state_layout import build_state_address

        return build_state_address(owner)
    from molt.dx import checkout_component

    return checkout_component(owner)


def _binds(kind: CheckoutStateKind, name: str, owner: Path) -> bool:
    key = _name_key(kind, name)
    if key is None:
        return False
    try:
        return key == _owner_key(kind, owner)
    except ValueError:
        return False  # An owner path through a link names no owned root.


def checkout_scratch_state_dir(
    kind: Literal["pytest-cache", "hypothesis-home"],
    source_root: Path,
    environment: Mapping[str, str],
) -> Path:
    """The pytest cache or Hypothesis home that ``source_root`` uses.

    The pytest cache is control state and stays on disk; the Hypothesis home
    follows the selected scratch storage, so with memory scratch it lives
    outside the artifact root and dies with that storage.
    """
    from molt.dx import checkout_component, control_state_dir, scratch_dir

    purpose = "/".join(
        (*CHECKOUT_STATE_CONTAINERS[kind][1:], checkout_component(source_root))
    )
    if kind == "pytest-cache":
        return control_state_dir(source_root, purpose, environment)
    return scratch_dir(source_root, purpose, environment)


def checkout_state_kind_at(parts: tuple[str, ...]) -> CheckoutStateKind | None:
    """The kind of checkout state root at ``parts`` below an artifact root."""
    for kind, container in CHECKOUT_STATE_CONTAINERS.items():
        if len(parts) == len(container) + 1 and parts[:-1] == container:
            return kind
    return None


def checkout_state_kind_of(root: Path, artifact_root: Path) -> CheckoutStateKind | None:
    """The kind of checkout state at ``root``: its place and an owner-key name."""
    try:
        parts = root.relative_to(artifact_root).parts
    except ValueError:
        return None
    kind = checkout_state_kind_at(parts)
    if kind is None or _name_key(kind, root.name) is None:
        return None
    return kind


def checkout_state_containers(artifact_root: Path) -> tuple[Path, ...]:
    """Every container of checkout state roots under ``artifact_root``."""
    return tuple(
        artifact_root.joinpath(*container)
        for container in dict.fromkeys(CHECKOUT_STATE_CONTAINERS.values())
    )


def _directories(parent: Path) -> list[Path]:
    try:
        with os.scandir(parent) as entries:
            return [
                Path(entry.path)
                for entry in entries
                if entry.is_dir(follow_symlinks=False)
            ]
    except OSError:
        return []


def checkout_state_roots(
    source_root: Path, environment: Mapping[str, str]
) -> tuple[CheckoutStateRoot, ...]:
    """The existing state roots of ``source_root`` under its shared artifact root.

    They include the build-control roots of its checkout target and of the
    pinned sessions below it. A plain clone keeps its target inside the
    checkout and its scratch in the host temporary folder, so it has none.
    """
    from molt.build_state_layout import build_state_address
    from molt.dx import artifact_root, cargo_target_dir_for_artifact_root

    source = source_root.expanduser().resolve()
    artifact = artifact_root(source, environment)
    if artifact == source or source in artifact.parents:
        return ()
    roots: list[CheckoutStateRoot] = []
    for kind, container in CHECKOUT_STATE_CONTAINERS.items():
        parent = artifact.joinpath(*container)
        if kind == "build-control":
            target = cargo_target_dir_for_artifact_root(
                artifact, None, source_root=source
            )
            for owner in (target, *_directories(target / "sessions")):
                path = parent / build_state_address(owner)
                if path.is_dir() and not path.is_symlink():
                    roots.append(CheckoutStateRoot(path, kind, owner))
            continue
        candidates = (
            _directories(parent)
            if kind == "uv-environment"
            else [parent / _owner_key(kind, source)]
        )
        roots.extend(
            CheckoutStateRoot(path, kind, source)
            for path in candidates
            if path.is_dir()
            and not path.is_symlink()
            and _binds(kind, path.name, source)
        )
    return tuple(roots)


def stamp_checkout_state(
    source_root: Path, environment: Mapping[str, str]
) -> tuple[CheckoutStateRoot, ...]:
    """Record the owner of ``source_root``'s existing roots, and a use of each.

    A uv environment is stamped only once uv created it, so no file of ours
    precedes ``pyvenv.cfg``. Returns the roots it stamped.
    """
    stamped: list[CheckoutStateRoot] = []
    for root in checkout_state_roots(source_root, environment):
        if root.kind == "uv-environment" and not (root.path / "pyvenv.cfg").is_file():
            continue
        record_checkout_owner(root.path, owner=root.owner, kind=root.kind)
        record_checkout_state_use(root.path)
        stamped.append(root)
    return tuple(stamped)


def record_checkout_state_use(root: Path, *, now: float | None = None) -> None:
    """Mark ``root`` used now; a stamp newer than the resolution stays as is.

    The common case is one ``stat``. A missing root stays missing.
    """
    stamp = root / LAST_USE_STAMP
    moment = time.time() if now is None else now
    try:
        if moment - stamp.stat().st_mtime < USE_RESOLUTION_S:
            return
    except FileNotFoundError:
        if not root.is_dir():
            return
        try:
            stamp.touch()
        except OSError:
            return
    except OSError:
        return
    try:
        os.utime(stamp, (moment, moment))
    except OSError:
        return


def newest_activity_mtime(path: Path) -> float:
    """The mtime of ``path`` or of its newest immediate child; 0.0 if unreadable.

    Cargo touches files deep in a target while the top directory lags, so one
    level of children catches recent work without a recursive walk.
    """
    try:
        newest = path.stat().st_mtime
    except OSError:
        return 0.0
    try:
        with os.scandir(path) as entries:
            for entry in entries:
                try:
                    newest = max(newest, entry.stat(follow_symlinks=False).st_mtime)
                except OSError:
                    continue
    except OSError:
        pass
    return newest


def checkout_state_last_use(root: Path) -> float:
    """The last use of ``root``: its newest activity, which includes the stamp.

    The stamp is an immediate child that every use refreshes, so a use that
    writes nothing still counts; a write without a stamp counts too. A root
    made before stamps existed ages out by its own activity, as Cargo adopts
    untracked entries instead of guessing their owner.
    """
    return newest_activity_mtime(root)


def record_checkout_target_use(target: Path, *, checkout: Path) -> None:
    """Mark checkout target ``target`` of ``checkout`` used, and bind its owner.

    ``target`` is a checkout target (``target/checkouts/<component>``) or a
    pinned session below one; any other target is not checkout state.
    """
    parent = target.parent
    if parent.name == "sessions":
        target, parent = parent.parent, parent.parent.parent
    if parent.name == "checkouts" and parent.parent.name == "target":
        record_checkout_owner(target, owner=checkout, kind="cargo-target")
        record_checkout_state_use(target)


def record_build_control_use(control: Path, *, cargo_target: Path) -> None:
    """Mark build-control root ``control`` of ``cargo_target`` used, and bind it.

    Only a root in the shared build-control container is checkout state; an
    explicit ``MOLT_BUILD_STATE_DIR`` or a target-local ``.molt_state`` is not.
    """
    if control.parent.name == "build-control" and _ADDRESS.fullmatch(control.name):
        record_checkout_owner(control, owner=cargo_target, kind="build-control")
        record_checkout_state_use(control)


def checkout_state_max_age_s(environment: Mapping[str, str]) -> float:
    """The idle time after which any checkout state root is retired."""
    raw = environment.get(MAX_AGE_ENV, "").strip()
    if not raw:
        return DEFAULT_MAX_AGE_DAYS * 86400.0
    try:
        days = float(raw)
    except ValueError:
        days = -1.0
    if not days > 0 or days != days or days == float("inf"):
        raise ValueError(f"{MAX_AGE_ENV} must be a positive number of days: {raw!r}")
    return days * 86400.0


CheckoutStateReason = Literal["owner-gone", "max-age"]


def checkout_state_decision(
    root: Path, artifact_root: Path, *, now: float, max_age_s: float
) -> CheckoutStateReason | None:
    """Why the GC may retire ``root`` now, or None to keep it.

    ``root`` must sit in a checkout state container of ``artifact_root`` under
    an owner-key name. Past ``max_age_s`` of idleness it retires whoever owns
    it; past the protection window it retires once its recorded owner is gone.
    Liveness checks (locks, protected paths) are the caller's.
    """
    if checkout_state_kind_of(root, artifact_root) is None:
        return None
    idle = now - checkout_state_last_use(root)
    if idle >= max_age_s:
        return "max-age"
    if idle >= PROTECTION_WINDOW_S and orphaned_checkout_state(root, artifact_root):
        return "owner-gone"
    return None


def checkout_state_gc_due(artifact_root: Path, *, now: float | None = None) -> bool:
    """Whether the daily GC of ``artifact_root``'s checkout state is due."""
    moment = time.time() if now is None else now
    try:
        stamp = artifact_root.joinpath(*GC_STAMP).stat().st_mtime
    except FileNotFoundError:
        return True
    except OSError:
        return False
    return not 0 <= moment - stamp < GC_INTERVAL_S


def claim_checkout_state_gc(artifact_root: Path) -> None:
    """Record that a GC of ``artifact_root`` started, so others skip it today."""
    stamp = artifact_root.joinpath(*GC_STAMP)
    stamp.parent.mkdir(parents=True, exist_ok=True)
    stamp.touch()
    os.utime(stamp)


def record_checkout_owner(root: Path, *, owner: Path, kind: CheckoutStateKind) -> None:
    """Publish ``root``'s owner once; an existing record stays unchanged.

    A record that would not bind to ``root``'s name is never written.
    """
    record = root / OWNER_RECORD
    if record.is_file() or not root.is_dir():
        return
    owner = owner.expanduser().resolve()
    if not _binds(kind, root.name, owner):
        return
    try:
        write_exact(
            record,
            {"schema": SCHEMA, "owner": str(owner), "kind": kind},
            exclusive=True,
        )
    except FileExistsError:
        return  # Another use published the same owner first.


def read_checkout_owner(root: Path) -> CheckoutOwner | None:
    """The strict owner record of ``root``, or None when absent or malformed."""
    try:
        value = read_exact(
            root / OWNER_RECORD,
            max_bytes=_MAX_RECORD_BYTES,
            label="checkout state owner",
        )
    except (OSError, ExactJsonError, ValueError):
        return None
    if not isinstance(value, dict) or set(value) != {"schema", "owner", "kind"}:
        return None
    owner, kind = value["owner"], value["kind"]
    if (
        value["schema"] != SCHEMA
        or not isinstance(owner, str)
        or kind not in _KINDS
        or not Path(owner).is_absolute()
    ):
        return None
    return CheckoutOwner(Path(owner), kind)


def owner_is_absent(owner: CheckoutOwner) -> bool:
    """True only when the owner path is gone and its parent is present.

    A missing parent may be an unmounted volume or an offline share, which is
    not proof that the owner was deleted.
    """
    try:
        owner.path.lstat()
    except FileNotFoundError:
        return owner.path.parent.is_dir()
    except OSError:
        return False
    return False


def bound_checkout_owner(root: Path, artifact_root: Path) -> CheckoutOwner | None:
    """``root``'s recorded owner when the record binds to ``root``.

    The record's kind must be the kind of ``root``'s place under
    ``artifact_root``, and the record's owner must give ``root``'s name.
    """
    try:
        parts = root.relative_to(artifact_root).parts
    except ValueError:
        return None
    owner = read_checkout_owner(root)
    if (
        owner is None
        or owner.kind != checkout_state_kind_at(parts)
        or not _binds(owner.kind, root.name, owner.path)
    ):
        return None
    return owner


def orphaned_checkout_state(root: Path, artifact_root: Path) -> CheckoutOwner | None:
    """``root``'s bound owner when that checkout or target is gone."""
    owner = bound_checkout_owner(root, artifact_root)
    return owner if owner is not None and owner_is_absent(owner) else None


def checkout_target_of(owner: CheckoutOwner) -> Path | None:
    """The checkout target a build-control owner names, or None.

    A pinned session's build control belongs to the checkout target above it.
    """
    if owner.kind != "build-control":
        return None
    target = owner.path
    if target.parent.name == "sessions":
        target = target.parent.parent
    if target.parent.name == "checkouts" and target.parent.parent.name == "target":
        return target
    return None
