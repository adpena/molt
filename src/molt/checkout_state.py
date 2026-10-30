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
A pinned session's Cargo target is a root of the same policy with its own,
shorter idle limit (`is_session_target`).
"""

from __future__ import annotations

from collections.abc import Callable, Generator, Iterable, Mapping, Sequence
from contextlib import contextmanager, ExitStack, suppress
from dataclasses import dataclass
from datetime import UTC, datetime
import errno
import hashlib
import os
from pathlib import Path
import re
import stat
import time
from types import MappingProxyType
from typing import TYPE_CHECKING, Literal, get_args

if TYPE_CHECKING:
    from .file_locks import _FileLockHandle
    from .process_launch import NativeLaunchResource

from .custody_layout import (
    SCRATCH_DIRNAME,
    CheckoutStateKind,
    StorageClass,
    EvidenceClass,
    OwnedStateKind,
    Regenerable,
    DurableEvidence,
    PackageEnvironmentKind,
    git_directories,
    storage_rule,
)
from .exact_json import ExactJsonError, read_exact, write_exact

EVIDENCE_INDEX_NAME = "storage-index.json"


def is_evidence_index(path: Path) -> bool:
    """Reserved recovery coordinates are durable control, never raw payload."""
    return path.name == EVIDENCE_INDEX_NAME or path.name.endswith(
        "." + EVIDENCE_INDEX_NAME
    )


def evidence_index_path(root: Path) -> Path:
    return (
        root / EVIDENCE_INDEX_NAME
        if not root.is_symlink() and not root.is_junction() and root.is_dir()
        else root.with_name(root.name + "." + EVIDENCE_INDEX_NAME)
    )


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
_CHECKOUT_RETENTION = storage_rule("cargo-target").retention
assert isinstance(_CHECKOUT_RETENTION, Regenerable)
DEFAULT_MAX_AGE_DAYS = _CHECKOUT_RETENTION.max_idle_s / 86400
# A pinned session's target holds isolated work (a benchmark, a test shard),
# not a checkout's lasting build, so it retires after a short idle time: the
# 6 hours that the disk guard's lane registry applied.
SESSION_IDLE_ENV = "MOLT_SESSION_TARGET_IDLE_HOURS"
DEFAULT_SESSION_IDLE_HOURS = 6.0
AGENT_EVIDENCE_IDLE_ENV = "MOLT_AGENT_EVIDENCE_IDLE_DAYS"
_EVIDENCE_RETENTION = storage_rule("lane-evidence").retention
assert isinstance(_EVIDENCE_RETENTION, DurableEvidence)
assert _EVIDENCE_RETENTION.compact_after_s is not None
DEFAULT_AGENT_EVIDENCE_IDLE_DAYS = _EVIDENCE_RETENTION.compact_after_s / 86400
SCRATCH_IDLE_ENV = "MOLT_SCRATCH_IDLE_DAYS"
_SCRATCH_RETENTION = storage_rule("scratch").retention
assert isinstance(_SCRATCH_RETENTION, Regenerable)
DEFAULT_SCRATCH_IDLE_DAYS = _SCRATCH_RETENTION.max_idle_s / 86400
RETIRED_DIR = "retired"
RETIREMENT_RECORD_SUFFIX = ".retirement.json"
RETIREMENT_WINDOW_S = 7 * 86400.0
_RETIREMENT_SCHEMA = "molt.soft-retirement.v2"
# The automatic GC runs at most once per interval per artifact root.
GC_INTERVAL_S = 24 * 3600.0
GC_STAMP = ("target", ".molt-checkout-state-gc")
_GC_LOCK = ("target", ".disk_guard", "checkout-state-gc.lock")
# `molt.dx.checkout_component`: a sanitized name prefix and 12 hex digits.
_COMPONENT = re.compile(r"[A-Za-z0-9_.-]{1,24}-[0-9a-f]{12}")
# `molt.build_state_layout.build_state_address`: a SHA-256 digest.
_ADDRESS = re.compile(r"[0-9a-f]{64}")
_SCRATCH_ROOT = re.compile(r"scratch-[0-9a-f]{12}")
# `molt.dx.session_artifact_component`: 1 to 32 letters, digits, `-` or `_`,
# in the `molt.dx.session_targets_dir` of a target.
_SESSION = re.compile(r"[A-Za-z0-9_-]{1,32}")
_SESSIONS = "sessions"

# Hard denylist: even if a classifier ever matched one of these under target/,
# refuse to delete it. These are the SHARED build outputs every lane depends on
# and structural control dirs -- defense-in-depth beside the narrow allow-set.
PROTECTED_TARGET_NAMES = frozenset(
    name.casefold()
    for name in {
        "debug",
        "release",
        "release-output",
        "doc",
        "package",
        "tmp",
        "checkouts",  # managed payload container, never a legacy lane
        ".disk_guard",  # stable external interlocks, never payload
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


_KINDS: frozenset[str] = frozenset(get_args(OwnedStateKind))

# Each kind's container, relative to an artifact root. The selectors in
# molt.dx, molt.build_state_layout and the pytest bootstrap place the state;
# tests bind them to this table. A uv environment's name ends with
# ``__src-<component>``, a build-control root is named by its target's address,
# and every other root is named by the component itself.
CHECKOUT_STATE_CONTAINERS: Mapping[CheckoutStateKind, tuple[str, ...]] = (
    MappingProxyType(
        {kind: storage_rule(kind).relative for kind in get_args(CheckoutStateKind)}
    )
)


@dataclass(frozen=True, slots=True)
class CheckoutOwner:
    """The owner a root's record names: its checkout, or for build control its
    Cargo target."""

    path: Path
    kind: OwnedStateKind


@dataclass(frozen=True, slots=True)
class CheckoutStateRoot:
    path: Path
    kind: CheckoutStateKind
    owner: Path


def _name_key(kind: CheckoutStateKind, name: str) -> str | None:
    """The owner key a root's name carries, or None for a foreign name."""
    name = os.path.normcase(name)
    if kind == "scratch":
        return name if name not in scratch_control_names() else None
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
        from .build_state_layout import build_state_address

        return build_state_address(owner)
    from .dx import checkout_component

    component = os.path.normcase(checkout_component(owner))
    # Reuse the checkout's identity, without its human prefix, to preserve
    # the Unix temporary-socket and Windows compiler path budgets.
    return "scratch-" + component[-12:] if kind == "scratch" else component


def _root_key(kind: CheckoutStateKind, root: Path) -> str | None:
    if kind == "python-environment":
        return (
            os.path.normcase(root.parent.name)
            if _ADDRESS.fullmatch(root.name) and _COMPONENT.fullmatch(root.parent.name)
            else None
        )
    if kind == "cargo-target" and os.path.normcase(root.parent.name) == "sessions":
        from .dx import is_session_artifact_component

        if not is_session_artifact_component(root.name):
            return None
        return _name_key(kind, root.parent.parent.name)
    return _name_key(kind, root.name)


def _toolchain_owner_root(root: Path) -> Path | None:
    if root.parent.name == "toolchains" and root.name not in {
        "sources",
        "downloads",
        "build",
    }:
        return root.parent.parent
    if (
        root.parent.name in {"sources", "downloads", "build"}
        and root.parent.parent.name == "toolchains"
    ):
        return root.parents[2]
    return None


def _binds(kind: OwnedStateKind, root: Path, owner: Path) -> bool:
    if kind == "toolchains":
        return _toolchain_owner_root(root) == owner
    if kind in {"pip-cache", "ruff-cache"}:
        return root == owner.joinpath(*storage_rule(kind).relative)
    if kind in {"browser-environment", "edgebox-environment"}:
        return root == owner.joinpath(*storage_rule(kind).relative)
    key = _root_key(kind, root)
    if key is None:
        return False
    try:
        return key == _owner_key(kind, owner)
    except ValueError:
        return False  # An owner path through a link names no owned root.


@dataclass(frozen=True, slots=True)
class _ManagedStateRoot:
    root: Path
    artifact: Path
    kind: StorageClass | Literal["agent-evidence"]


def package_environment_coordinate(
    root: Path,
) -> tuple[PackageEnvironmentKind, Path, Path] | None:
    """Closed package input coordinates, independent of ambient volume choices."""
    for kind in get_args(PackageEnvironmentKind):
        relative = storage_rule(kind).relative
        if len(root.parts) > len(relative) and root.parts[-len(relative) :] == relative:
            checkout = root.parents[len(relative) - 1]
            try:
                directories = git_directories(checkout)
            except (OSError, ValueError, RuntimeError):
                continue  # Optional resource claims do not replace native diagnostics.
            if directories is not None:
                return kind, checkout, directories[1]
    return None


def _global_storage_coordinate(root: Path) -> _ManagedStateRoot | None:
    """Finite authority placements; no owner or preimage adoption."""
    from .file_publication import resolve_owned_path

    if package := package_environment_coordinate(root):
        kind, _checkout, common = package
        return _ManagedStateRoot(resolve_owned_path(root), common / "molt", kind)
    for kind in ("compile-cache", "uv-cache", "pip-cache", "ruff-cache", "sccache"):
        if root.name == storage_rule(kind).relative[0]:
            physical = resolve_owned_path(root)
            return _ManagedStateRoot(physical, physical.parent, kind)
    for candidate in (root, *root.parents):
        if _toolchain_owner_root(candidate) is None:
            continue
        owner = read_checkout_owner(candidate)
        if (
            owner is not None
            and owner.kind == "toolchains"
            and _binds(owner.kind, candidate, owner.path)
        ):
            physical = resolve_owned_path(candidate)
            return _ManagedStateRoot(physical, owner.path, "toolchains")
    return None


def target_use_artifact_root(root: Path) -> Path | None:
    """The finite structural target placement, independent of deletion globs.

    This only names the external lifetime lock and last-use stamp. It confers
    no checkout ownership or deletion permission, including for shared outputs.
    """
    if root.name.casefold() in PROTECTED_TARGET_NAMES:
        return None
    if os.path.normcase(root.parent.name) == "target":
        return root.parent.parent
    if os.path.normcase(root.name).startswith("cargo-target-"):
        return root.parent
    return None


def legacy_target_kind_of(
    root: Path, artifact: Path, lane_globs: Iterable[str]
) -> Literal["lane", "cargo-target"] | None:
    """The existing legacy deletion layouts, with caller-owned glob policy."""
    import fnmatch

    if target_use_artifact_root(root) != artifact:
        return None
    if os.path.normcase(root.name).startswith("cargo-target-"):
        return "cargo-target"
    if any(fnmatch.fnmatchcase(root.name, pattern) for pattern in lane_globs):
        return "lane"
    return None


def _managed_coordinate(root: Path) -> _ManagedStateRoot | None:
    """Bind existing placement with the canonical no-link custody identity."""
    root = root.expanduser().absolute()
    if global_state := _global_storage_coordinate(root):
        return global_state
    if root.name == "agents" and root.parent.name == "logs":
        from .custody_layout import coordination_state_root
        from .file_publication import resolve_owned_path

        root = resolve_owned_path(root)
        return _ManagedStateRoot(
            root, coordination_state_root(root.parents[1]), "agent-evidence"
        )
    if root.parent.name == "agents" and root.parent.parent.name == "logs":
        from .custody_layout import coordination_state_root
        from .file_publication import resolve_owned_path

        root = resolve_owned_path(root)
        return _ManagedStateRoot(
            root, coordination_state_root(root.parents[2]), "agent-evidence"
        )
    names = tuple(os.path.normcase(part) for part in root.parts)
    containers = (
        *(
            (*container, "*") if kind == "python-environment" else container
            for kind, container in CHECKOUT_STATE_CONTAINERS.items()
            if kind != "scratch" or _SCRATCH_ROOT.fullmatch(os.path.normcase(root.name))
        ),
        ("target", "checkouts", "*", "sessions"),
        ("target", "sessions"),
    )
    possible = target_use_artifact_root(root) is not None or any(
        len(names) > len(container)
        and all(
            expected == "*" or observed == os.path.normcase(expected)
            for observed, expected in zip(names[-len(container) - 1 : -1], container)
        )
        for container in containers
    )
    if not possible:
        return None  # No filesystem probe for an unrelated caller-owned root.
    from .file_publication import resolve_owned_path

    root = resolve_owned_path(root)
    if os.path.normcase(root.parent.name) == "sessions" and len(root.parents) > 2:
        artifact = root.parents[2]
        if is_session_target(root, artifact):
            return _ManagedStateRoot(root, artifact, "cargo-target")
    if os.path.normcase(root.parent.name) == "sessions" and len(root.parents) > 4:
        artifact = root.parents[4]
        if checkout_state_kind_of(root, artifact) == "cargo-target":
            return _ManagedStateRoot(root, artifact, "cargo-target")
    for kind, container in CHECKOUT_STATE_CONTAINERS.items():
        depth = len(container) + (kind == "python-environment")
        if len(root.parents) <= depth:
            continue
        artifact = root.parents[depth]
        if checkout_state_kind_of(root, artifact) == kind:
            if kind == "scratch" and not _SCRATCH_ROOT.fullmatch(
                os.path.normcase(root.name)
            ):
                # Untracked scratch has no cooperating use protocol. In
                # particular, ordinary host /tmp children must never acquire
                # artifact locks under the filesystem root.
                continue
            return _ManagedStateRoot(root, artifact, kind)
    artifact = target_use_artifact_root(root)
    if artifact is not None:
        return _ManagedStateRoot(root, artifact, "cargo-target")
    return None


# One lock per checkout-state root, named by its build-state address.
_STATE_LOCK_DIR = ("target", ".disk_guard", "checkout-state-locks")
_STATE_LOCK_NAME = re.compile(r"[0-9a-f]{64}\.lock")


def _state_metadata_root(root: Path) -> Path:
    """Mutable use metadata stays outside a published Python generation."""
    if (toolchain_root := _toolchain_owner_root(root)) is not None:
        from .build_state_layout import build_state_address

        return (
            toolchain_root
            / "target"
            / ".disk_guard"
            / "storage-use"
            / build_state_address(root)
        )
    global_state = _global_storage_coordinate(root)
    if global_state is not None:
        from .build_state_layout import build_state_address

        return (
            global_state.artifact
            / "target"
            / ".disk_guard"
            / "storage-use"
            / build_state_address(root)
        )
    container = CHECKOUT_STATE_CONTAINERS["python-environment"]
    depth = len(container) + 1
    if (
        len(root.parents) > depth
        and _root_key("python-environment", root) is not None
        and tuple(os.path.normcase(part) for part in root.parts[-depth - 1 : -2])
        == container
    ):
        from .build_state_layout import build_state_address

        return root.parents[depth].joinpath(
            *_STATE_LOCK_DIR, "metadata", build_state_address(root)
        )
    return root


def is_checkout_state_lock_file(path: Path) -> bool:
    """Recognize an empty lock that `_state_lock_plan` names, never data.

    Like a publication lock, it leaves only with its whole checkout and is
    never unlinked to make one look clean: a waiter may hold its inode.
    """
    if (
        _STATE_LOCK_NAME.fullmatch(path.name) is None
        or path.parent.parts[-len(_STATE_LOCK_DIR) :] != _STATE_LOCK_DIR
    ):
        return False
    try:
        found = path.lstat()
    except OSError:
        return False
    return stat.S_ISREG(found.st_mode) and found.st_size == 0


def _state_lock_plan(
    selections: Iterable[tuple[Path | _ManagedStateRoot, CheckoutOwner | None]],
) -> tuple[
    tuple[tuple[_ManagedStateRoot, CheckoutOwner | None], ...], dict[Path, Path]
]:
    """Bind each finite coordinate and external lock once for one admission.

    This plan is never cached across operations. Publication revalidates after
    acquisition and at completion, including a reader delayed by retirement.
    """
    from .build_state_layout import build_state_address

    coordinates: dict[Path, _ManagedStateRoot | None] = {}
    locks: dict[Path, Path] = {}
    bound = []

    def qualify(path: Path) -> _ManagedStateRoot | None:
        if path not in coordinates:
            coordinates[path] = _managed_coordinate(path)
        return coordinates[path]

    for selected, owner in selections:
        coordinate = (
            _ManagedStateRoot(selected, owner.path, "toolchains")
            if isinstance(selected, Path)
            and owner is not None
            and owner.kind == "toolchains"
            and _binds(owner.kind, selected, owner.path)
            else selected
            if isinstance(selected, _ManagedStateRoot)
            else qualify(selected)
        )
        if coordinate is None:
            continue
        root = coordinate.root
        coordinates[root] = coordinate
        bound.append((coordinate, owner))
        dependency = root
        if coordinate.kind == "build-control":
            known_owner = owner or bound_checkout_owner(root, coordinate.artifact)
            if known_owner is not None and known_owner.kind == "build-control":
                dependency = known_owner.path
        paths = [root]
        if dependency != root:
            paths.insert(0, dependency)
        if os.path.normcase(dependency.parent.name) == "sessions":
            paths.insert(0, dependency.parent.parent)
        for path in paths:
            known = qualify(path)
            if known is not None and known.root not in locks:
                locks[known.root] = known.artifact.joinpath(
                    *_STATE_LOCK_DIR, build_state_address(known.root) + ".lock"
                )
    return tuple(bound), locks


def _publish_checkout_use(
    coordinate: _ManagedStateRoot, owner: CheckoutOwner | None, *, create: bool
) -> None:
    root = coordinate.root
    if (
        coordinate.kind in {"uv-environment", "python-environment"}
        and not (root / "pyvenv.cfg").is_file()
    ):
        return  # uv owns creation of its environment before our control files.
    if owner is not None:
        if create and coordinate.kind == "build-control":
            # Created here, under this use's lock, so the owner is bound
            # before the writer writes into the root: an owner record is
            # written only into an existing root, and selections ran before
            # consumers created theirs (HF-469: 6,609 of 7,475 roots had no
            # owner on 2026-10-22).
            with suppress(OSError):
                root.mkdir(parents=True, exist_ok=True)
        _record_checkout_owner(root, owner=owner.path, kind=owner.kind)
    _record_checkout_state_use(root)


def checkout_state_lock_paths(root: Path) -> tuple[Path, ...]:
    """The existing external lifetime locks; observing them creates nothing."""
    _, locks = _state_lock_plan(((root, None),))
    return tuple(locks.values())


@contextmanager
def checkout_state_use(
    roots: Iterable[tuple[Path, CheckoutOwner | None]],
    *,
    publish: bool = True,
    create: bool = False,
) -> Generator[tuple[_FileLockHandle, ...], None, None]:
    """Hold shared use for this operation, including root materialization.

    The caller owns the lifetime. Resolving an environment alone does not hold
    a lease; command/build/exec owners retain these ordinary OS lock handles.

    A selection that writes passes ``create``: each of its build-control roots
    is created under the use's lock, then its owner is bound. Without it a use
    binds and stamps only a root that exists, so a command's roots
    (`checkout_state_command_roots`) never create one (HF-469).
    """
    selections, locks = _state_lock_plan(roots)
    if not locks:
        yield ()
        return
    from .file_locks import _acquire_file_lock, _release_file_lock

    with ExitStack() as stack:
        handles = []
        for root, lock in sorted(
            locks.items(), key=lambda item: (len(item[0].parts), str(item[0]))
        ):
            handle = _acquire_file_lock(
                lock,
                shared=True,
                timeout_s=None,
                timeout_message=f"checkout state stayed in retirement: {root}",
            )
            stack.callback(_release_file_lock, handle)
            handles.append(handle)

        def publish_use() -> None:
            for coordinate, owner in selections:
                known = _managed_coordinate(coordinate.root)
                if known != coordinate and not (
                    known is None
                    and owner is not None
                    and owner.kind == "toolchains"
                    and _binds(owner.kind, coordinate.root, owner.path)
                ):
                    raise ValueError(
                        f"checkout state coordinate changed: {coordinate.root}"
                    )
                if publish:
                    _publish_checkout_use(coordinate, owner, create=create)

        publish_use()
        try:
            yield tuple(handles)
        except BaseException as error:
            try:
                publish_use()
            except BaseException as publication_error:
                error.add_note(
                    f"checkout state completion publication failed: {publication_error!r}"
                )
            raise
        else:
            # Completion is a fresh boundary, not reuse of the initial path
            # observation, even when a user outlives the protection window.
            publish_use()


@contextmanager
def checkout_state_retirement(
    root: Path,
    *,
    purged_root: Path | None = None,
) -> Generator[bool, None, None]:
    """Try exclusive mutation; every live managed reader keeps its lineage."""
    from .file_locks import _try_acquire_file_lock, _release_file_lock

    coordinate = _managed_coordinate(root)
    owner = (
        None
        if coordinate is None
        else bound_checkout_owner(coordinate.root, coordinate.artifact)
    )
    if coordinate is not None:
        root = coordinate.root
    with ExitStack() as stack:
        _, locks = _state_lock_plan(((coordinate or root, owner),))
        for lock in locks.values():
            handle = _try_acquire_file_lock(lock)
            if handle is None:
                yield False
                return
            stack.callback(_release_file_lock, handle)
        if (
            coordinate is not None
            and bound_checkout_owner(coordinate.root, coordinate.artifact) != owner
        ):
            # A control's changed owner must never redirect its dependency
            # while retirement holds the previous owner's lock chain.
            yield False
            return
        yield True
        # Python generation metadata lives outside its immutable payload.
        # Keep it through soft retirement so rename-back restores ownership;
        # only final GC may forget it, and never while a replacement exists.
        if (
            purged_root is not None
            and not os.path.lexists(purged_root)
            and coordinate is not None
            and coordinate.kind == "python-environment"
        ):
            metadata = _state_metadata_root(root)
            if not os.path.lexists(root):
                try:
                    entries = tuple(metadata.iterdir())
                except FileNotFoundError:
                    entries = ()
                if entries and all(
                    entry.name in {OWNER_RECORD, LAST_USE_STAMP}
                    and stat.S_ISREG(entry.lstat().st_mode)
                    for entry in entries
                ):
                    for entry in entries:
                        entry.unlink()
                    metadata.rmdir()


def checkout_state_environment_roots(
    source_root: Path,
    environment: Mapping[str, str],
    *,
    cwd: str | os.PathLike[str] | None = None,
    include_current_process: bool = True,
) -> tuple[tuple[Path, CheckoutOwner | None], ...]:
    """Declared state and this process's environment, without a child command.

    Suite owners and a detached worker acknowledging its own use hold this
    explicit projection. Launch owners must use ``checkout_state_command_roots``.
    Activation is a declared resource selection, never interpreter identity.
    A receiving exec handoff excludes the replaced process's ambient prefix;
    declared environment resources still belong to the receiving invocation.
    """
    from .build_state_layout import build_state_root

    roots: dict[Path, CheckoutOwner | None] = {}
    working = Path.cwd() if cwd is None else Path(cwd).absolute()
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_BUILD_STATE_DIR",
        "MOLT_CACHE",
        "UV_CACHE_DIR",
        "PIP_CACHE_DIR",
        "RUFF_CACHE_DIR",
        "SCCACHE_DIR",
    ):
        raw = environment.get(key, "").strip()
        if raw:
            path = Path(raw).expanduser()
            if not path.is_absolute():
                path = working / path
            path = path.absolute()
            roots[path] = (
                CheckoutOwner(source_root.resolve(), "python-environment")
                if _state_metadata_root(path) != path
                and _binds("python-environment", path, source_root.resolve())
                else None
            )
    import sys

    if include_current_process:
        roots[Path(sys.prefix).absolute()] = None
    raw_prefix = environment.get("PYTHONPYCACHEPREFIX", "").strip()
    if raw_prefix:
        from .python_environment_location import environment_bytecode_prefix

        prefix = Path(raw_prefix).expanduser()
        if not prefix.is_absolute():
            prefix = working / prefix
        image = prefix.absolute().parent
        if (
            prefix.absolute() == environment_bytecode_prefix(image)
            and _state_metadata_root(image) != image
        ):
            roots.setdefault(image, None)
    raw_target = environment.get("CARGO_TARGET_DIR", "").strip()
    if raw_target:
        target = Path(raw_target).expanduser()
        if not target.is_absolute():
            target = working / target
        target = target.absolute()
        explicit_control = environment.get("MOLT_BUILD_STATE_DIR", "").strip()
        if explicit_control:
            control = Path(explicit_control).expanduser()
            if not control.is_absolute():
                control = working / control
            control = control.absolute()
        else:
            control = build_state_root(
                project_root=source_root, cargo_target=target, environment=environment
            )
        roots[control] = CheckoutOwner(target, "build-control")
    if raw_virtual := environment.get("VIRTUAL_ENV", "").strip():
        from .process_launch import NativeLaunchResource

        virtual = Path(raw_virtual).expanduser()
        if not virtual.is_absolute():
            virtual = working / virtual
        _retain_python_resources(roots, (NativeLaunchResource(virtual, "directory"),))
    for key in (
        "TMPDIR",
        "TMP",
        "TEMP",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "MOLT_PROOF_SCRATCH_ROOT",
        "MOLT_GUARD_SCRATCH_ROOT",
    ):
        if raw := environment.get(key, "").strip():
            selected = Path(raw).expanduser()
            selected = (working / selected).absolute()
            for candidate in (selected, *selected.parents):
                if not _SCRATCH_ROOT.fullmatch(os.path.normcase(candidate.name)):
                    continue
                coordinate = _managed_coordinate(candidate)
                if coordinate is not None and coordinate.kind == "scratch":
                    roots.setdefault(coordinate.root, None)
                    break
    return tuple(roots.items())


def checkout_state_command_roots(
    source_root: Path,
    environment: Mapping[str, str],
    command: Sequence[str],
    *,
    cwd: str | os.PathLike[str] | None = None,
    include_current_process: bool = True,
) -> tuple[tuple[Path, CheckoutOwner | None], ...]:
    """Retain finite declared and possible native command resources.

    Candidate directories conservatively retain a published environment even
    when another PATH entry wins. This does not select or admit an executable.
    Ordinary unmanaged coordinates receive no filesystem probe; only a bounded
    enclosing published-image coordinate enters the existing custody authority.
    """
    from .process_launch import command_resources

    roots = dict(
        checkout_state_environment_roots(
            source_root,
            environment,
            cwd=cwd,
            include_current_process=include_current_process,
        )
    )
    working = Path.cwd() if cwd is None else Path(cwd).absolute()
    _retain_python_resources(
        roots, command_resources(command, environment, cwd=working)
    )
    # Executing a provisioned tool retains the exact version, rather than
    # pinning every old toolchain merely because MOLT_TARGET_ROOT is set.
    for resource in command_resources(command, environment, cwd=working):
        for candidate in (resource.path, *resource.path.parents):
            if _toolchain_owner_root(candidate) is not None:
                coordinate = _managed_coordinate(candidate)
                if coordinate is not None and coordinate.kind == "toolchains":
                    roots.setdefault(candidate, None)
                break
    from .custody_layout import enclosing_checkouts

    try:
        checkouts = enclosing_checkouts(working)
    except (OSError, ValueError, RuntimeError):
        checkouts = ()  # The unchanged native launch decides invalid cwd/Git inputs.
    for checkout in dict.fromkeys((source_root, *checkouts)):
        for kind in get_args(PackageEnvironmentKind):
            path = checkout.joinpath(*storage_rule(kind).relative)
            owner = read_checkout_owner(path)
            if owner == CheckoutOwner(checkout, kind):
                roots.setdefault(path, owner)
    return tuple(roots.items())


def _retain_python_resources(
    roots: dict[Path, CheckoutOwner | None],
    resources: Iterable[NativeLaunchResource],
) -> None:
    """Optional launch resources confer use only on existing bound images.

    Missing, foreign, unowned or inaccessible search coordinates impose no new
    launch refusal or materialization intent. Strict admission and retirement
    retain their existing no-link checks. Resolve only enclosing directories,
    preserving the environment's lexical interpreter entrypoint symlink.
    """
    container = CHECKOUT_STATE_CONTAINERS["python-environment"]
    qualified: dict[Path, tuple[Path, CheckoutOwner | None] | None] = {}

    def prefixes(member: Path) -> Generator[Path, None, None]:
        parts = member.parts
        names = tuple(os.path.normcase(part) for part in parts)
        for start in range(len(parts) - len(container) - 1):
            end = start + len(container) + 2
            if (
                names[start : start + len(container)] == container
                and _COMPONENT.fullmatch(names[end - 2]) is not None
                and _ADDRESS.fullmatch(names[end - 1]) is not None
            ):
                yield Path(*parts[:end])

    def retain(root: Path) -> bool:
        if root in roots:
            return True  # Already retained by this operation's explicit intent.
        if root not in qualified:
            qualified[root] = None
            try:
                physical = root.resolve(strict=True)
                if physical in roots:
                    qualified[root] = (physical, roots[physical])
                    return True
                coordinate = _managed_coordinate(physical)
                if (
                    coordinate is not None
                    and coordinate.kind == "python-environment"
                    and physical.is_dir()
                ):
                    owner = bound_checkout_owner(physical, coordinate.artifact)
                    if owner is not None and owner.kind == "python-environment":
                        qualified[root] = (physical, owner)
            except (OSError, ValueError, RuntimeError):
                # Optional search membership is not a stricter executable
                # resolver. The unchanged native launch decides these errors.
                pass
        selected = qualified[root]
        if selected is None:
            return False
        physical, owner = selected
        roots.setdefault(physical, owner)
        return True

    for resource in resources:
        member = resource.path
        names = tuple(os.path.normcase(part) for part in member.parts)
        if any(part not in names for part in container):
            continue  # No filesystem probe for an ordinary unmanaged coordinate.
        normalized = Path(os.path.abspath(member))
        present = False
        for lexical in dict.fromkeys((member, normalized)):
            for root in prefixes(lexical):
                present = retain(root) or present
        if not present:
            continue
        directory = member if resource.kind == "directory" else member.parent
        try:
            physical_directory = directory.resolve(strict=True)
        except (OSError, ValueError, RuntimeError):
            continue
        for root in prefixes(physical_directory):
            retain(root)


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
    from .dx import checkout_component, control_state_dir, scratch_dir

    purpose = "/".join(
        (*CHECKOUT_STATE_CONTAINERS[kind][1:], checkout_component(source_root))
    )
    if kind == "pytest-cache":
        return control_state_dir(source_root, purpose, environment)
    return scratch_dir(source_root, purpose, environment)


def scratch_control_names() -> frozenset[str]:
    """Existing control namespaces, whose own protocols alone retire them.

    Guard scratch (gs and pt-* allocations) requires terminal custody, not
    age. IR leases, daemon sockets and build admission also outlive a resolver.
    Keep those protocols outside the agent-scratch collector.
    """
    from .memory_guard_paths import STATE_DIRNAME

    return frozenset(
        {
            *(
                container[1]
                for container in CHECKOUT_STATE_CONTAINERS.values()
                if len(container) == 2 and container[0] == SCRATCH_DIRNAME
            ),
            STATE_DIRNAME,
            "pytest-memory-guard",
            "harness_memory_guard",
            "gs",
            "backend-ir-leases",
            "molt-build-slots",
        }
    )


def scratch_state_root(container: Path, *, owner: Path, create: bool = True) -> Path:
    """Select one lane's scratch; materialize, bind and stamp it before use.

    The owner is the lane's checkout, just like other checkout state. Purpose
    directories share this root, never the root of another lane. Selection
    alone is read-only; DX's materializing path stamps existing roots too.
    """
    from .file_publication import resolve_owned_path

    owner = resolve_owned_path(owner.expanduser().absolute())
    root = resolve_owned_path(container / _owner_key("scratch", owner))
    if create:
        with checkout_state_use(((root, None),), publish=False):
            root.mkdir(parents=True, exist_ok=True)
            resolve_owned_path(root / OWNER_RECORD)
            _record_checkout_owner(root, owner=owner, kind="scratch")
            if read_checkout_owner(root) != CheckoutOwner(owner, "scratch"):
                raise ValueError(f"scratch owner does not bind: {root}")
            resolve_owned_path(root / LAST_USE_STAMP)
            _record_checkout_state_use(root, required=True)
    return root


def checkout_state_kind_at(parts: tuple[str, ...]) -> CheckoutStateKind | None:
    """The kind of checkout state root at ``parts`` below an artifact root."""
    parts = tuple(os.path.normcase(part) for part in parts)
    container = CHECKOUT_STATE_CONTAINERS["python-environment"]
    if len(parts) == len(container) + 2 and parts[:-2] == container:
        return "python-environment"
    if (
        len(parts) == 5
        and parts[:2] == ("target", "checkouts")
        and parts[3] == "sessions"
    ):
        return "cargo-target"
    for kind, container in CHECKOUT_STATE_CONTAINERS.items():
        if len(parts) == len(container) + 1 and parts[:-1] == container:
            if kind == "scratch" and (
                parts[-1] in scratch_control_names()
                or parts[-1].startswith(("pt-", "molt-backend-"))
            ):
                return None
            return kind
    return None


def checkout_state_kind_of(root: Path, artifact_root: Path) -> CheckoutStateKind | None:
    """The kind of checkout state at ``root``: its place and an owner-key name."""
    try:
        parts = root.relative_to(artifact_root).parts
    except ValueError:
        return None
    kind = checkout_state_kind_at(parts)
    if kind is None or _root_key(kind, root) is None:
        return None
    return kind


def checkout_state_containers(artifact_root: Path) -> tuple[Path, ...]:
    """Every container of checkout state roots under ``artifact_root``."""
    containers = [
        artifact_root.joinpath(*container)
        for container in dict.fromkeys(CHECKOUT_STATE_CONTAINERS.values())
    ]
    targets = artifact_root.joinpath(*CHECKOUT_STATE_CONTAINERS["cargo-target"])
    containers.extend(
        target / "sessions"
        for target in _directories(targets)
        if _name_key("cargo-target", target.name) is not None
    )
    generations = artifact_root.joinpath(
        *CHECKOUT_STATE_CONTAINERS["python-environment"]
    )
    containers.extend(
        owner for owner in _directories(generations) if _COMPONENT.fullmatch(owner.name)
    )
    return tuple(containers)


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


def is_session_target(root: Path, artifact_root: Path) -> bool:
    """Whether ``root`` is a pinned session's Cargo target under ``artifact_root``.

    A session target is ``<target>/sessions/<session>``, where ``<target>``
    is a checkout target (``target/checkouts/<component>``) or the root's own
    ``target``, as a plain clone keeps it. The place and the session name
    decide; nothing records a session's owner, because its target retires
    with it.
    """
    try:
        parts = root.relative_to(artifact_root).parts
    except ValueError:
        return False
    if len(parts) < 3 or parts[-2] != _SESSIONS or not _SESSION.fullmatch(parts[-1]):
        return False
    holder = parts[:-2]
    return holder == ("target",) or (
        checkout_state_kind_at(holder) == "cargo-target"
        and _name_key("cargo-target", holder[-1]) is not None
    )


def session_target_dirs(artifact_root: Path) -> list[Path]:
    """Every pinned session's target under ``artifact_root``; links excluded."""
    target = artifact_root / "target"
    holders = [
        target,
        *(
            child
            for child in _directories(
                artifact_root.joinpath(*CHECKOUT_STATE_CONTAINERS["cargo-target"])
            )
            if _name_key("cargo-target", child.name) is not None
        ),
    ]
    return [
        session
        for holder in holders
        for session in _directories(holder / _SESSIONS)
        if _SESSION.fullmatch(session.name)
    ]


def target_build_control_dirs(
    target: Path, artifact_roots: Iterable[Path]
) -> list[Path]:
    """The build-control roots that a Cargo target and its pinned sessions own.

    `molt.build_state_layout.build_state_root` places a target's build
    control in the shared container of the build's artifact root, under the
    target's address, or in the target's own ``.molt_state`` without an
    artifact root. A reader cannot tell which root a build used, so this
    names the address under each of ``artifact_roots``. Only existing
    directories are returned; a target reached through a link owns none.
    """
    from molt.build_state_layout import build_state_address

    controls: list[Path] = []
    for owner in (target, *_directories(target / _SESSIONS)):
        try:
            address = build_state_address(owner)
        except ValueError:
            continue
        controls.append(owner / ".molt_state")
        controls.extend(
            root.joinpath(*CHECKOUT_STATE_CONTAINERS["build-control"], address)
            for root in artifact_roots
        )
    return [path for path in dict.fromkeys(controls) if path.is_dir()]


def checkout_state_roots(
    source_root: Path, environment: Mapping[str, str]
) -> tuple[CheckoutStateRoot, ...]:
    """The existing state roots of ``source_root`` under its shared artifact root.

    They include the build-control roots of its checkout target and of the
    pinned sessions below it. A plain clone keeps its target inside the
    checkout and its scratch in the host temporary folder, so it has none.
    """
    from .build_state_layout import build_state_address
    from .dx import artifact_root, cargo_target_dir_for_artifact_root

    source = source_root.expanduser().resolve()
    artifact = artifact_root(source, environment)
    if artifact == source or source in artifact.parents:
        return ()
    roots: list[CheckoutStateRoot] = []
    target = cargo_target_dir_for_artifact_root(artifact, None, source_root=source)
    sessions = _directories(target / "sessions")
    for kind, container in CHECKOUT_STATE_CONTAINERS.items():
        parent = artifact.joinpath(*container)
        if kind == "python-environment":
            parent /= _owner_key(kind, source)
        if kind == "build-control":
            for owner in (target, *sessions):
                path = parent / build_state_address(owner)
                if path.is_dir() and not path.is_symlink():
                    roots.append(CheckoutStateRoot(path, kind, owner))
            continue
        candidates = (
            _directories(parent)
            if kind in {"uv-environment", "python-environment"}
            else [target, *sessions]
            if kind == "cargo-target"
            else [parent / _owner_key(kind, source)]
        )
        roots.extend(
            CheckoutStateRoot(path, kind, source)
            for path in candidates
            if path.is_dir() and not path.is_symlink() and _binds(kind, path, source)
        )
    return tuple(roots)


def stamp_checkout_state(
    source_root: Path, environment: Mapping[str, str]
) -> tuple[CheckoutStateRoot, ...]:
    """Record the owner of ``source_root``'s existing roots, and a use of each.

    A uv environment is stamped only once uv created it, so no file of ours
    precedes ``pyvenv.cfg``. Returns the roots it stamped.
    """
    from .dx import (
        artifact_root,
        cargo_target_dir_for_environment,
        project_cargo_target_dir,
    )

    # Stamping belongs to the development context. Without an explicit target,
    # use that context's selector, not the CLI's in-project default.
    selected = (
        project_cargo_target_dir(source_root, environment)
        if environment.get("CARGO_TARGET_DIR", "").strip()
        else cargo_target_dir_for_environment(
            artifact_root(source_root, environment),
            environment,
            source_root=source_root,
        )
    )
    targets = {selected}
    if os.path.normcase(selected.parent.name) == "sessions":
        targets.add(selected.parent.parent)
    if target_use_artifact_root(selected) is not None:
        record_checkout_target_use(selected, checkout=source_root)
    stamped: list[CheckoutStateRoot] = []
    selected_python = {
        Path(raw).expanduser().absolute()
        for key in ("UV_PROJECT_ENVIRONMENT", "MOLT_VENV")
        if (raw := environment.get(key, "").strip())
    }
    for root in checkout_state_roots(source_root, environment):
        if root.kind == "cargo-target" and root.path not in targets:
            continue
        if root.kind == "build-control" and root.owner != selected:
            continue
        if root.kind == "python-environment" and root.path not in selected_python:
            continue
        if (
            root.kind in {"uv-environment", "python-environment"}
            and not (root.path / "pyvenv.cfg").is_file()
        ):
            continue
        with checkout_state_use(((root.path, CheckoutOwner(root.owner, root.kind)),)):
            stamped.append(root)
    return tuple(stamped)


def record_checkout_state_use(root: Path, *, now: float | None = None) -> None:
    with checkout_state_use(((root, None),), publish=False):
        _record_checkout_state_use(root, now=now)


def _record_checkout_state_use(
    root: Path, *, now: float | None = None, required: bool = False
) -> None:
    """Mark ``root`` used now; a stamp newer than the resolution stays as is.

    The common case is one ``stat``. A missing root stays missing.
    """
    metadata = _state_metadata_root(root)
    stamp = metadata / LAST_USE_STAMP
    moment = time.time() if now is None else now
    try:
        if moment - stamp.stat().st_mtime < USE_RESOLUTION_S:
            return
    except FileNotFoundError:
        if not (
            root.exists() if _toolchain_owner_root(root) is not None else root.is_dir()
        ):
            if required:
                raise FileNotFoundError(root)
            return
        try:
            metadata.mkdir(parents=True, exist_ok=True)
            stamp.touch()
        except OSError:
            if required:
                raise
            return
    except OSError:
        if required:
            raise
        return
    try:
        os.utime(stamp, (moment, moment))
    except OSError:
        if required:
            raise
        return


# Layouts a later design replaced inside roots that stay live, frozen as the
# last code that wrote them left them (design 56, sections 2.8 and 2.9). Until
# the host's product store, every Cargo target kept, per profile directory
# (``<target>/<profile>`` or ``<target>/<triple>/<profile>``), its native
# runtime generations with their selection receipts and its backend
# generations; every build control kept backend probe evidence and a receipt
# beside Cargo's backend output. Nothing writes these names now. They hold
# about 10 to 20 GB in live targets on the integrator Mac, so the daily GC
# retires them inside a live root by the same custody it retires roots with.
SupersededLayoutRoot = Literal["cargo-target", "build-control"]
_SUPERSEDED_GENERATION_DIRS = (".molt-native-generations", ".molt-backend-generations")
_SUPERSEDED_RUNTIME_RECEIPT = re.compile(
    r"(?:lib)?molt_runtime\.stdlib_[a-z]+\.(?:a|lib)\.generation\.json"
)
_SUPERSEDED_PROBE_EVIDENCE = "backend_probe_validations"
_SUPERSEDED_RECEIPTS_DIR = "backend_fingerprints"
_SUPERSEDED_CARGO_OUTPUT_RECEIPT = re.compile(
    r"(?:molt-backend|molt-wasm-facts)(?:\.exe)?\.[A-Za-z0-9_-]+\.[0-9a-f]{16}"
    r"\.fingerprint"
)


def _real_child(parent: Path, name: str, *, directory: bool) -> Path | None:
    path = parent / name
    try:
        found = path.lstat()
    except OSError:
        return None
    wanted = stat.S_ISDIR(found.st_mode) if directory else stat.S_ISREG(found.st_mode)
    return path if wanted else None


def _superseded_profile_entries(profile: Path) -> list[Path]:
    """One profile directory's superseded receipts, then its generations.

    Receipts come first, so an interrupted retirement never leaves a receipt
    naming a removed generation; the old reader misses on either.
    """
    generations = [
        path
        for name in _SUPERSEDED_GENERATION_DIRS
        if (path := _real_child(profile, name, directory=True)) is not None
    ]
    if not generations:
        return []  # Receipts appear only beside a generation directory.
    try:
        with os.scandir(profile) as entries:
            receipts = sorted(
                Path(entry.path)
                for entry in entries
                if _SUPERSEDED_RUNTIME_RECEIPT.fullmatch(entry.name)
                and entry.is_file(follow_symlinks=False)
            )
    except OSError:
        receipts = []
    return [*receipts, *generations]


def superseded_layout_paths(root: Path, kind: SupersededLayoutRoot) -> list[Path]:
    """The superseded layout entries inside one live root, in removal order.

    It reads a Cargo target's direct children and their children (finite
    listings, never a profile's ``deps``), and two names in a build control.
    """
    found: list[Path] = []
    if kind == "build-control":
        evidence = _real_child(root, _SUPERSEDED_PROBE_EVIDENCE, directory=True)
        receipts = _real_child(root, _SUPERSEDED_RECEIPTS_DIR, directory=True)
        if receipts is not None:
            try:
                with os.scandir(receipts) as entries:
                    found.extend(
                        sorted(
                            Path(entry.path)
                            for entry in entries
                            if _SUPERSEDED_CARGO_OUTPUT_RECEIPT.fullmatch(entry.name)
                            and entry.is_file(follow_symlinks=False)
                        )
                    )
            except OSError:
                pass
        if evidence is not None:
            found.append(evidence)
        return found
    # Two levels reach every profile directory and nothing deeper: a pinned
    # session's profiles sit three levels down (a root of its own), and an
    # operator-owned target's store four.
    for child in _directories(root):
        found.extend(_superseded_profile_entries(child))
        for grandchild in _directories(child):
            found.extend(_superseded_profile_entries(grandchild))
    return found


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
    newest = newest_activity_mtime(root)
    metadata = _state_metadata_root(root)
    if metadata != root:
        try:
            newest = max(newest, (metadata / LAST_USE_STAMP).stat().st_mtime)
        except OSError:
            pass
    if os.path.normcase(root.parent.name) == "checkouts" and _name_key(
        "cargo-target", root.name
    ):
        # A nested session may write without touching its parent directory.
        # Its recent activity must also keep that enclosing target alive.
        newest = max(
            (
                newest,
                *(
                    newest_activity_mtime(path)
                    for path in _directories(root / "sessions")
                    if _root_key("cargo-target", path) is not None
                ),
            )
        )
    return newest


def record_checkout_target_use(target: Path, *, checkout: Path) -> None:
    """Mark Cargo target ``target`` of ``checkout`` used, and bind its owner.

    A pinned session's target (``<target>/sessions/<session>``) is a root of
    its own: its use stamps it, then the target that holds it. Only a checkout
    target (``target/checkouts/<component>``) takes an owner record; any
    other target is not checkout state. The common case costs one ``stat``
    per stamp and one for the owner record.
    """
    targets = [target]
    # Explicit structural targets retain their use without adopting an owner.
    if target_use_artifact_root(target) is not None:
        with checkout_state_use(((target, None),)):
            pass
    if os.path.normcase(target.parent.name) == "sessions":
        # Main's local session layout has no checkout owner, but its use and
        # retirement still share the same managed lifetime lock.
        if os.path.normcase(target.parent.parent.name) == "target":
            with checkout_state_use(((target, None),)):
                pass
        targets.append(target.parent.parent)
    for root in targets:
        if (
            os.path.normcase(root.parent.name) == "checkouts"
            and os.path.normcase(root.parent.parent.name) == "target"
        ) or (
            os.path.normcase(root.parent.name) == "sessions"
            and os.path.normcase(root.parent.parent.parent.name) == "checkouts"
            and os.path.normcase(root.parent.parent.parent.parent.name) == "target"
        ):
            with checkout_state_use(((root, CheckoutOwner(checkout, "cargo-target")),)):
                pass


def bound_project_build_state_root(
    project_root: Path, environment: Mapping[str, str]
) -> Path:
    """The project's build-control root, created with its owner bound, for a
    writer that does not select it through a build (HF-469)."""
    from molt.build_state_layout import project_build_state_root
    from molt.dx import project_cargo_target_dir

    root = project_build_state_root(project_root, environment)
    record_build_control_use(
        root, cargo_target=project_cargo_target_dir(project_root, environment)
    )
    return root


def record_build_control_use(control: Path, *, cargo_target: Path) -> None:
    """Create build-control root ``control`` of ``cargo_target``, bind it and
    mark it used, for a writer that selects it.

    Only a root in the shared build-control container is checkout state; an
    explicit ``MOLT_BUILD_STATE_DIR`` or a target-local ``.molt_state`` is not.
    """
    if control.parent.name == "build-control" and _ADDRESS.fullmatch(control.name):
        with checkout_state_use(
            ((control, CheckoutOwner(cargo_target, "build-control")),), create=True
        ):
            pass


def record_toolchain_use(prefix: Path) -> None:
    """Record use only after the owning provider has admitted this generation.

    The owner and stamp stay outside the sealed payload. A directory census
    never adopts an unproven toolchain preimage as disposable state.
    """
    from .file_publication import resolve_owned_path

    toolchain_root = _toolchain_owner_root(prefix)
    if toolchain_root is None:
        return  # System SDKs are not Molt-managed installations.
    prefix = resolve_owned_path(prefix)
    owner = CheckoutOwner(toolchain_root, "toolchains")
    with checkout_state_use(((prefix, owner),)):
        pass


def record_package_environment_use(root: Path) -> None:
    """The lock-file provisioner calls this only after admitting its stamp."""
    package = package_environment_coordinate(root)
    if package is not None:
        kind, checkout, _common = package
        with checkout_state_use(((root, CheckoutOwner(checkout, kind)),)):
            pass


def materialize_provider_cache(
    root: Path, *, kind: Literal["pip-cache", "ruff-cache"]
) -> None:
    """Bind only a freshly created cache, never adopt existing preimage data.

    Use metadata stays outside the provider's format. Custom cache selectors
    still work, but an unbound placement cannot be retired wholesale by Molt.
    """
    from .file_publication import resolve_owned_path

    root = resolve_owned_path(root)
    with checkout_state_use(((root, None),), publish=False):
        fresh = not root.exists()
        root.mkdir(parents=True, exist_ok=True)
        owner = CheckoutOwner(root.parent, kind)
        if fresh and _binds(kind, root, owner.path):
            _record_checkout_owner(root, owner=owner.path, kind=kind)
        if read_checkout_owner(root) == owner:
            _record_checkout_state_use(root)


@contextmanager
def toolchain_state_use(
    paths: Iterable[Path], *, create: bool = False
) -> Generator[None, None, None]:
    """Retain a provider's declared paths before publication through closure.

    Holding a lock never adopts a preimage. Only the successful provider's
    `record_toolchain_use` publishes provenance; an absent/unproven payload
    stays ineligible for GC. Existing admitted payload records completion use
    even when an operation fails.
    """
    from .file_publication import resolve_owned_path

    selections = []
    for path in paths:
        path = path.expanduser().absolute()
        if not create:
            known = _global_storage_coordinate(path)
            if known is None or known.kind != "toolchains":
                continue
            selections.append((known.root, CheckoutOwner(known.artifact, "toolchains")))
            continue
        toolchain_root = _toolchain_owner_root(path)
        if toolchain_root is None:
            known = _global_storage_coordinate(path)
            if known is None or known.kind != "toolchains":
                continue
            path, toolchain_root = known.root, known.artifact
        path = resolve_owned_path(path)
        selections.append((path, CheckoutOwner(toolchain_root, "toolchains")))
    with checkout_state_use(selections, publish=False):

        def used() -> None:
            for path, owner in selections:
                if read_checkout_owner(path) == owner:
                    _record_checkout_state_use(path)

        used()
        try:
            yield
        finally:
            used()


@dataclass(frozen=True, slots=True)
class RetentionLimits:
    """How long a root may stay unused before the GC retires it."""

    max_age_s: float
    session_idle_s: float
    scratch_idle_s: float = DEFAULT_SCRATCH_IDLE_DAYS * 86400.0


def retention_limits(environment: Mapping[str, str]) -> RetentionLimits:
    """The idle limits ``environment`` declares; an invalid one raises ValueError."""
    return RetentionLimits(
        max_age_s=_idle_limit_s(
            environment, MAX_AGE_ENV, DEFAULT_MAX_AGE_DAYS, 86400.0, "days"
        ),
        session_idle_s=_idle_limit_s(
            environment, SESSION_IDLE_ENV, DEFAULT_SESSION_IDLE_HOURS, 3600.0, "hours"
        ),
        scratch_idle_s=_idle_limit_s(
            environment, SCRATCH_IDLE_ENV, DEFAULT_SCRATCH_IDLE_DAYS, 86400.0, "days"
        ),
    )


def agent_evidence_idle_s(environment: Mapping[str, str]) -> float:
    return _idle_limit_s(
        environment,
        AGENT_EVIDENCE_IDLE_ENV,
        DEFAULT_AGENT_EVIDENCE_IDLE_DAYS,
        86400.0,
        "days",
    )


def _idle_limit_s(
    environment: Mapping[str, str],
    name: str,
    default: float,
    unit_s: float,
    unit: str,
) -> float:
    """An idle limit in seconds. It may not undercut the protection window."""
    raw = environment.get(name, "").strip()
    if not raw:
        return default * unit_s
    try:
        limit_s = float(raw) * unit_s
    except ValueError:
        limit_s = float("nan")
    # NaN fails both comparisons.
    if not PROTECTION_WINDOW_S <= limit_s < float("inf"):
        raise ValueError(
            f"{name} must be a number of {unit} that covers the "
            f"{PROTECTION_WINDOW_S / 3600:g}-hour protection window: {raw!r}"
        )
    return limit_s


@dataclass(frozen=True, slots=True)
class SoftRetirement:
    path: Path
    original: Path
    reason: str
    retired_at: float
    owner: str
    kind: StorageClass

    @property
    def record_path(self) -> Path:
        return self.path.with_name(self.path.name + RETIREMENT_RECORD_SUFFIX)


def _retirement_name(root: Path, now: float) -> str:
    # Bounded, portable encoding of the full coordinate and this generation.
    # A reused original path must not replace an earlier recoverable root.
    digest = hashlib.sha256(os.fsencode(root)).hexdigest()
    return f"{digest}-{int(now * 1_000_000_000)}"


def compact_evidence(
    root: Path,
    archive_root: Path,
    *,
    kind: EvidenceClass,
    keep: Iterable[Path] = (),
    companions: Sequence[Path] = (),
    remove_source: bool = True,
    index_retained_source: bool = False,
    preserve_leaf_link: bool = False,
) -> Path:
    """Losslessly compact evidence, publishing its recovery index before removal.

    The caller holds exclusive producer custody. Every archived regular file
    is independently hashed from the archive before any source is removed.
    A crash can leave duplicate bytes, never unindexed lost evidence. This is
    deliberately separate from regenerable-state retirement. Retained snapshots
    opt into a source recovery index with index_retained_source; checkout
    retirement snapshots otherwise publish only the external recovery index.
    preserve_leaf_link is namespace custody: archive/unlink the leaf itself,
    without reading or granting custody over its target.
    """
    import tarfile
    import uuid

    from .file_publication import resolve_owned_path

    # A lane may own an ignored namespace entry while its target belongs to a
    # provider. Preserve that link itself, never traverse or adopt its target.
    root = (
        resolve_owned_path(root.parent) / root.name
        if preserve_leaf_link and root.is_symlink()
        else resolve_owned_path(root)
    )
    archive_root = resolve_owned_path(archive_root)
    if archive_root == root or root in archive_root.parents:
        raise ValueError("evidence archive must be outside its source")
    kept = {str(path) for path in keep}
    root_metadata = root.lstat()
    is_directory = stat.S_ISDIR(root_metadata.st_mode)
    if not is_directory and is_evidence_index(root):
        raise ValueError("durable recovery index cannot be compacted as raw payload")
    index = evidence_index_path(root)
    previous_index = None
    if index.exists():
        previous = read_exact(
            index, max_bytes=64 * 1024, label="evidence recovery index"
        )
        if (
            not isinstance(previous, dict)
            or previous.get("schema") != "molt.evidence-index.v1"
            or previous.get("source") != str(root)
            or previous.get("class") != kind
            or not isinstance(previous.get("archive"), str)
        ):
            raise ValueError(
                f"unrecognized evidence index; preserve and reconcile {index}"
            )
        old_archive = Path(previous["archive"])
        if old_archive.parent != archive_root or not old_archive.name.endswith(
            ".tar.gz"
        ):
            raise ValueError(
                f"previous archive is outside its evidence authority: {old_archive}"
            )
        previous_index = str(old_archive.with_suffix(".index.json"))
        if not Path(previous_index).is_file():
            raise ValueError(f"missing previous evidence manifest: {previous_index}")
    paths = (
        [path for path in sorted(root.rglob("*")) if path != index]
        if is_directory
        else [root]
    )
    base = root if is_directory else root.parent
    for companion in companions:
        companion = resolve_owned_path(companion)
        if is_directory or companion.parent != root.parent:
            raise ValueError("evidence companions must share the file's parent")
        if companion.exists():
            paths.extend(
                [companion, *sorted(companion.rglob("*"))]
                if companion.is_dir()
                else [companion]
            )
    # rglob does not descend directory symlinks. Reject non-file payloads;
    # neither devices nor sockets are durable portable evidence.
    manifest = {}
    signatures = {}
    if is_directory:
        signatures[root] = (
            root_metadata.st_dev,
            root_metadata.st_ino,
            root_metadata.st_size,
            root_metadata.st_mtime_ns,
        )
    for path in paths:
        metadata = path.lstat()
        relative = path.relative_to(base).as_posix()
        if stat.S_ISLNK(metadata.st_mode):
            manifest[relative] = {"link": os.readlink(path)}
        elif stat.S_ISREG(metadata.st_mode):
            manifest[relative] = {"sha256": "", "bytes": metadata.st_size}
        elif not stat.S_ISDIR(metadata.st_mode):
            raise ValueError(f"nonportable evidence: {path}")
        signatures[path] = (
            metadata.st_dev,
            metadata.st_ino,
            metadata.st_size,
            metadata.st_mtime_ns,
        )
    from .disk_capacity import require_storage_capacity

    require_storage_capacity(
        (archive_root,),
        kind=kind,
        additional_bytes=sum(entry[2] for entry in signatures.values())
        + 2048 * len(signatures),
    )
    archive_root.mkdir(parents=True, exist_ok=True)
    archive = (
        archive_root
        / f"{hashlib.sha256(os.fsencode(root)).hexdigest()[:16]}-{uuid.uuid4().hex}.tar.gz"
    )
    staging = archive.with_suffix(".pending")
    try:
        with (
            os.fdopen(
                os.open(staging, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb"
            ) as archive_output,
            tarfile.open(
                fileobj=archive_output, mode="w:gz", dereference=False
            ) as stream,
        ):
            for path in paths:
                relative = path.relative_to(base).as_posix()
                if "sha256" in manifest.get(relative, {}):

                    class HashingReader:
                        def read(self, size: int = -1) -> bytes:
                            data = source.read(size)
                            digest.update(data)
                            return data

                    with path.open("rb") as source:
                        digest = hashlib.sha256()
                        info = stream.gettarinfo(str(path), arcname=relative)
                        info.type, info.linkname = tarfile.REGTYPE, ""
                        info.size = signatures[path][2]
                        stream.addfile(info, HashingReader())
                        manifest[relative]["sha256"] = digest.hexdigest()
                else:
                    stream.add(path, arcname=relative, recursive=False)
        with tarfile.open(staging, "r:gz") as stream:
            observed = {}
            for member in stream:
                if member.isfile():
                    payload = stream.extractfile(member)
                    assert payload is not None
                    with payload:
                        archived_digest = hashlib.sha256()
                        while block := payload.read(1024 * 1024):
                            archived_digest.update(block)
                    observed[member.name] = {
                        "sha256": archived_digest.hexdigest(),
                        "bytes": member.size,
                    }
                elif member.issym():
                    observed[member.name] = {"link": member.linkname}
            if observed != manifest:
                raise ValueError("evidence changed during compaction")
        for path, expected in signatures.items():
            metadata = path.lstat()
            if (
                metadata.st_dev,
                metadata.st_ino,
                metadata.st_size,
                metadata.st_mtime_ns,
            ) != expected:
                raise ValueError(f"evidence changed during compaction: {path}")
        with staging.open("rb") as source:
            os.fsync(source.fileno())
        from .file_publication import durable_namespace_replace

        durable_namespace_replace(staging, archive)
        with archive.open("rb") as stream:
            archive_digest = hashlib.file_digest(stream, "sha256").hexdigest()
        record = {
            "schema": "molt.evidence-index.v1",
            "class": kind,
            "source": str(root),
            "archive": str(archive),
            "archive_sha256": archive_digest,
            "files": manifest,
        }
        if previous_index is not None:
            record["previous_index"] = previous_index
        # External sidecar survives even a failed checkout-index publication.
        external_index = archive.with_suffix(".index.json")
        write_exact(external_index, record)
        if not remove_source and not index_retained_source:
            return archive
        write_exact(
            index, {key: value for key, value in record.items() if key != "files"}
        )
        if not remove_source:
            return archive
        from .file_deletion import delete_path

        # Remove maximal represented subtrees through the canonical sealed-tree
        # boundary. Never chmod files or follow links to external images.
        preserved = {base / relative for relative in kept}
        preserved.update(
            parent
            for path in tuple(preserved)
            for parent in path.parents
            if parent.is_relative_to(base)
        )
        removals: set[Path] = set()
        for path in paths:
            if path in preserved or any(parent in removals for parent in path.parents):
                continue
            removals.add(path)
            removed, error = delete_path(path, owning_root=base)
            if not removed:
                raise OSError(f"archived source removal deferred: {path}: {error}")
        return archive
    finally:
        staging.unlink(missing_ok=True)


def soft_retire(
    root: Path,
    owning_root: Path,
    *,
    reason: str,
    owner: str,
    kind: StorageClass,
    now: float | None = None,
) -> SoftRetirement:
    """Retire a root with one same-filesystem rename and one small record.

    The caller owns eligibility and exclusive custody. Artifact state stays
    under its artifact root; checkout evidence stays under its family custody
    root. No copying or cross-device fallback is permitted. The sidecar is
    published before the rename, so interruption never leaves moved bytes
    without a recovery address. A sidecar without payload grants no deletion.
    Recovery is ``retirement.path.rename(retirement.original)`` after clearing
    any replacement at the original coordinate.
    """
    from .file_publication import resolve_owned_path

    from .custody_layout import Regenerable

    if not isinstance(storage_rule(kind).retention, Regenerable):
        raise ValueError(f"durable evidence requires indexed compaction: {kind}")

    root, owning_root = resolve_owned_path(root), resolve_owned_path(owning_root)
    if root == owning_root or owning_root not in root.parents:
        raise ValueError(f"retirement outside owning root: {root}")
    if root.is_relative_to(owning_root / RETIRED_DIR):
        raise ValueError(f"root is already retired: {root}")
    if (
        not isinstance(reason, str)
        or not reason
        or not isinstance(owner, str)
        or not owner
    ):
        raise ValueError("retirement requires a reason and owner")
    moment = time.time() if now is None else now
    if type(moment) not in (float, int) or not 0 <= moment < float("inf"):
        raise ValueError("retirement requires a finite, nonnegative timestamp")
    date = datetime.fromtimestamp(moment, UTC).strftime("%Y-%m-%d")
    parent = resolve_owned_path(owning_root / RETIRED_DIR / date)
    parent.mkdir(parents=True, exist_ok=True)
    if root.stat().st_dev != parent.stat().st_dev:
        raise OSError(
            errno.EXDEV, "soft retirement requires the same filesystem", str(root)
        )
    retirement = SoftRetirement(
        parent / _retirement_name(root, moment), root, reason, moment, owner, kind
    )
    if os.path.lexists(retirement.path):
        raise FileExistsError(str(retirement.path))
    write_exact(
        retirement.record_path,
        {
            "schema": _RETIREMENT_SCHEMA,
            "original_path": str(root),
            "reason": reason,
            "retired_at": moment,
            "owner": owner,
            "kind": kind,
        },
        exclusive=True,
    )
    try:
        root.rename(retirement.path)
    except BaseException:
        retirement.record_path.unlink()
        raise
    return retirement


def read_soft_retirement(record: Path, owning_root: Path) -> SoftRetirement | None:
    """Validate a sidecar's exact coordinate; malformed records keep payloads."""
    from .file_publication import resolve_owned_path

    try:
        owning_root, record = (
            resolve_owned_path(owning_root),
            resolve_owned_path(record),
        )
        value = read_exact(record, max_bytes=_MAX_RECORD_BYTES, label="soft retirement")
        if not isinstance(value, dict) or set(value) != {
            "schema",
            "original_path",
            "reason",
            "retired_at",
            "owner",
            "kind",
        }:
            return None
        original, reason, moment, owner = (
            value["original_path"],
            value["reason"],
            value["retired_at"],
            value["owner"],
        )
        if (
            value["schema"] != _RETIREMENT_SCHEMA
            or value["kind"] not in get_args(StorageClass)
            or not isinstance(original, str)
            or not Path(original).is_absolute()
            or not isinstance(reason, str)
            or not reason
            or not isinstance(owner, str)
            or not owner
            or type(moment) not in (float, int)
            or not 0 <= moment < float("inf")
        ):
            return None
        original_path = Path(original)
        if owning_root not in original_path.parents or original_path.is_relative_to(
            owning_root / RETIRED_DIR
        ):
            return None
        expected = (
            owning_root
            / RETIRED_DIR
            / datetime.fromtimestamp(moment, UTC).strftime("%Y-%m-%d")
            / _retirement_name(original_path, moment)
        )
        retirement = SoftRetirement(
            expected, original_path, reason, moment, owner, value["kind"]
        )
        if retirement.record_path != record or resolve_owned_path(expected) != expected:
            return None
        return retirement if os.path.lexists(expected) else None
    except (OSError, ExactJsonError, ValueError, OverflowError):
        return None


# Walk time that one family of complete walks may spend in one daily pass. The
# daily collection runs detached, so it may walk longer than pressure reclaim,
# which never uses this rotation. The budget must hold a root's plan walk and
# its recheck walk: the slowest external scratch root walked in 75 s cold
# (2026-10-28) and 48 s warm (2026-10-29), so a pair needs about 150 s; 30 s
# left 13 roots (47.8 GiB) unreclaimable every cycle (HF-537).
WALK_BUDGET_S = 180.0
# Where the next applied pass resumes each family: one small record in the
# disk guard's stable interlock directory. It grants no deletion authority.
WALK_ROTATION_RECORD = ("target", ".disk_guard", "walk-rotation.json")
_WALK_ROTATION_SCHEMA = "molt.walk-rotation.v1"
_WALK_ROTATION_MAX_BYTES = 64 * 1024
# The daily pass's complete-walk families: legacy scratch under every scope
# root's `tmp`, expired scratch retirements, terminal agent evidence under
# every checkout's `logs/agents`, and integration receipts.
WalkFamily = Literal[
    "scratch", "retired-scratch", "agent-evidence", "integration-receipts"
]
_WALK_FAMILIES: frozenset[str] = frozenset(get_args(WalkFamily))
WalkIncompleteReason = Literal["inventory-deferred", "inventory-exceeds-pass-budget"]


class WalkIncomplete(Exception):
    """A complete walk did not finish in this pass; it authorizes nothing."""

    def __init__(self, reason: WalkIncompleteReason) -> None:
        super().__init__(reason)
        self.reason: WalkIncompleteReason = reason


@dataclass(slots=True)
class _WalkFamilyState:
    cursor: str | None
    remaining_s: float
    first: str | None = None
    next_cursor: str | None = None
    walks: int = 0
    deferred: int = 0
    exceeded: int = 0
    walk_s: float = 0.0


class WalkRotation:
    """Complete walks that resume where the previous applied pass stopped.

    A family visits its entries in key (path) order from a persisted cursor
    and wraps around, as ccache cleans one cache bucket and git maintenance
    packs one bounded batch per run. The family's walks share ``budget_s``
    of walk time per pass; constant-work decisions between walks do not
    spend it. A walk the budget interrupts is deferred, and the next pass
    starts there with the whole budget. If the first walk of a pass cannot
    finish in the whole budget, the entry is reported and kept, and the
    cursor passes it. So each pass completes at least one entry. Each pass
    but the last of a cycle spends the budget on completed walks plus at
    most one interrupted walk, which the next pass completes first. Every
    entry is therefore evaluated within ``min(L, 1 + 2 * C / budget_s)``
    passes: ``L`` entries need a walk, and ``C`` is the sum of their walk
    times (every walk an applied pass makes before it retires the entry),
    each capped at the budget. Only an applied pass moves the cursor; a dry
    run previews the next applied pass.
    """

    def __init__(
        self, record: Path, budget_s: float, cursors: Mapping[str, str]
    ) -> None:
        self.record = record
        self.budget_s = budget_s
        self.cursors = dict(cursors)
        self._families: dict[WalkFamily, _WalkFamilyState] = {}

    @classmethod
    def load(cls, artifact_root: Path) -> WalkRotation:
        record = artifact_root.joinpath(*WALK_ROTATION_RECORD)
        return cls(record, WALK_BUDGET_S, _read_walk_cursors(record))

    def _family(self, name: WalkFamily) -> _WalkFamilyState:
        if name not in self._families:
            self._families[name] = _WalkFamilyState(
                self.cursors.get(name), self.budget_s
            )
        return self._families[name]

    def order(self, name: WalkFamily, keys: Iterable[str]) -> list[str]:
        """``keys`` in order, from the first one not before the cursor."""
        import bisect

        ordered = sorted(keys)
        cursor = self._family(name).cursor
        if cursor is None:
            return ordered
        start = bisect.bisect_left(ordered, cursor)
        return ordered[start:] + ordered[:start]

    def spend[T](self, name: WalkFamily, key: str, work: Callable[[float], T]) -> T:
        """Run ``work(deadline)`` for ``key``, or raise `WalkIncomplete`.

        ``work`` raises TimeoutError once the deadline passes; it must change
        nothing before it completes. Every walk of one entry in one pass,
        such as a plan and its recheck, spends the same family budget.
        """
        family = self._family(name)
        if family.first is None:
            family.first = key
        started = time.monotonic()
        deadline = started + family.remaining_s
        try:
            if family.remaining_s <= 0:
                raise TimeoutError("walk budget spent")
            result = work(deadline)
        except TimeoutError:
            if time.monotonic() < deadline:
                raise  # A filesystem timeout is an ordinary walk error.
            if key == family.first:
                family.exceeded += 1
                raise WalkIncomplete("inventory-exceeds-pass-budget") from None
            if family.next_cursor is None:
                family.next_cursor = key
            family.deferred += 1
            raise WalkIncomplete("inventory-deferred") from None
        finally:
            elapsed = max(0.0, time.monotonic() - started)
            family.remaining_s -= elapsed
            family.walk_s += elapsed
        family.walks += 1
        return result

    def next_cursors(self) -> dict[str, str]:
        return {
            name: family.next_cursor
            for name, family in sorted(self._families.items())
            if family.next_cursor is not None
        }

    def report(self) -> dict[str, dict[str, object]]:
        return {
            name: {
                "cursor": family.cursor,
                "next_cursor": family.next_cursor,
                "walks": family.walks,
                "deferred": family.deferred,
                "exceeded": family.exceeded,
                "walk_s": family.walk_s,
            }
            for name, family in sorted(self._families.items())
        }

    def publish(self) -> None:
        """Persist where each family resumes; an unchanged record stays as is."""
        from .file_publication import PublicationDurability, resolve_owned_path

        cursors = self.next_cursors()
        if cursors == self.cursors:
            return
        record = resolve_owned_path(self.record)
        record.parent.mkdir(parents=True, exist_ok=True)
        write_exact(
            record,
            {"schema": _WALK_ROTATION_SCHEMA, "cursors": cursors},
            durability=PublicationDurability.RECONSTRUCTIBLE,
        )
        self.cursors = cursors


def _read_walk_cursors(record: Path) -> dict[str, str]:
    """The persisted cursors; an absent or malformed record restarts each family."""
    from .file_publication import resolve_owned_path

    try:
        value = read_exact(
            resolve_owned_path(record),
            max_bytes=_WALK_ROTATION_MAX_BYTES,
            label="walk rotation",
        )
    except (OSError, ExactJsonError, ValueError, RecursionError):
        return {}
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "cursors"}
        or value["schema"] != _WALK_ROTATION_SCHEMA
        or not isinstance(value["cursors"], dict)
        or not all(
            name in _WALK_FAMILIES and isinstance(cursor, str)
            for name, cursor in value["cursors"].items()
        )
    ):
        return {}
    return dict(value["cursors"])


RetentionReason = Literal["owner-gone", "max-age", "session-idle"]


def checkout_state_decision(
    root: Path,
    artifact_root: Path,
    *,
    now: float,
    limits: RetentionLimits,
    last_use: float | None = None,
) -> RetentionReason | None:
    """Why the GC may retire ``root`` now, or None to keep it.

    ``root`` is a pinned session's target (`is_session_target`), or sits in a
    checkout state container of ``artifact_root`` under an owner-key name. A
    session target retires once idle past ``limits.session_idle_s``. Any
    other root retires past ``limits.max_age_s`` of idleness whoever owns it,
    and past the protection window once its recorded owner is gone. Liveness
    checks (locks, protected paths) are the caller's.
    """
    if is_session_target(root, artifact_root):
        idle = now - checkout_state_last_use(root)
        return "session-idle" if idle >= limits.session_idle_s else None
    kind = checkout_state_kind_of(root, artifact_root)
    if kind is None and target_use_artifact_root(root) != artifact_root:
        return None
    idle = now - (checkout_state_last_use(root) if last_use is None else last_use)
    if idle < PROTECTION_WINDOW_S:
        return None
    if idle >= (limits.scratch_idle_s if kind == "scratch" else limits.max_age_s):
        return "max-age"
    if idle >= PROTECTION_WINDOW_S and orphaned_checkout_state(root, artifact_root):
        return "owner-gone"
    return None


@dataclass(frozen=True, slots=True)
class CheckoutStateGcEpoch:
    """Cadence and attempt identity from one observation of the daily stamp."""

    generation_ns: int | None
    due: bool

    @property
    def identity(self) -> str:
        return "initial" if self.generation_ns is None else str(self.generation_ns)


def checkout_state_gc_epoch(
    artifact_root: Path, *, now: float | None = None
) -> CheckoutStateGcEpoch | None:
    """Unreadable cadence defers; a missing stamp names the initial epoch."""
    moment = time.time() if now is None else now
    try:
        stamp = artifact_root.joinpath(*GC_STAMP).stat()
    except FileNotFoundError:
        return CheckoutStateGcEpoch(None, True)
    except OSError:
        return None
    return CheckoutStateGcEpoch(
        stamp.st_mtime_ns, not 0 <= moment - stamp.st_mtime < GC_INTERVAL_S
    )


def checkout_state_gc_due(artifact_root: Path, *, now: float | None = None) -> bool:
    """Whether the daily GC of ``artifact_root``'s checkout state is due."""
    epoch = checkout_state_gc_epoch(artifact_root, now=now)
    return epoch is not None and epoch.due


@contextmanager
def checkout_state_gc(artifact_root: Path) -> Generator[bool, None, None]:
    """Admit one daily attempt without waiting, retaining ownership until done.

    The stamp remains the cadence authority. The stable external interlock
    coalesces callers that observed the same due stamp, including collections
    lasting longer than the interval. An admitted attempt stamps its start and
    completion under that lock, so a terminal contended no-op cannot consume
    the next due epoch. Hard interruption keeps the start-time throttle.
    Explicit collection retains its own root locks.
    """
    if not checkout_state_gc_due(artifact_root):
        yield False
        return
    from .file_locks import (
        _file_lock_owned_operation,
        _release_file_lock,
        _try_acquire_file_lock,
    )

    lock_path = artifact_root.joinpath(*_GC_LOCK)
    handle = _try_acquire_file_lock(lock_path)
    if handle is None:
        yield False
        return
    try:
        with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
            if not checkout_state_gc_due(artifact_root):
                yield False
                return
            stamp = artifact_root.joinpath(*GC_STAMP)
            stamp.touch()
            os.utime(stamp)
            failure: BaseException | None = None
            try:
                yield True
            except BaseException as exc:
                failure = exc
                raise
            finally:
                try:
                    os.utime(stamp)
                except OSError as exc:
                    if failure is None:
                        raise
                    failure.add_note(f"daily GC completion stamp failed: {exc}")
    finally:
        _release_file_lock(handle)


def record_checkout_owner(root: Path, *, owner: Path, kind: OwnedStateKind) -> None:
    with checkout_state_use(((root, CheckoutOwner(owner, kind)),), publish=False):
        _record_checkout_owner(root, owner=owner, kind=kind)


def _record_checkout_owner(root: Path, *, owner: Path, kind: OwnedStateKind) -> None:
    """Publish ``root``'s owner once; an existing record stays unchanged.

    A record that would not bind to ``root``'s name is never written.
    """
    record = _state_metadata_root(root) / OWNER_RECORD
    if record.is_file() or not root.exists():
        return
    owner = owner.expanduser().resolve()
    if not _binds(kind, root, owner):
        return
    try:
        record.parent.mkdir(parents=True, exist_ok=True)
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
            _state_metadata_root(root) / OWNER_RECORD,
            max_bytes=_MAX_RECORD_BYTES,
            label="checkout state owner",
        )
    except (OSError, ExactJsonError, ValueError, RecursionError):
        return None
    if not isinstance(value, dict) or set(value) != {"schema", "owner", "kind"}:
        return None
    owner, kind = value["owner"], value["kind"]
    if (
        value["schema"] != SCHEMA
        or not isinstance(owner, str)
        or not isinstance(kind, str)
        or kind not in _KINDS
        or not Path(owner).is_absolute()
    ):
        return None
    return CheckoutOwner(Path(owner), kind)


def owner_is_absent(owner: CheckoutOwner) -> bool:
    """True only when the owner path is gone and its parent is present.

    A missing parent may be an unmounted volume or an offline share, which is
    not proof that the owner was deleted. A pinned session's target goes with
    the checkout target that holds it, so that target's absence proves the
    session's.
    """
    if _path_is_absent(owner.path):
        return True
    holder = checkout_target_of(owner)
    return holder is not None and holder != owner.path and _path_is_absent(holder)


def _path_is_absent(path: Path) -> bool:
    try:
        path.lstat()
    except FileNotFoundError:
        return path.parent.is_dir()
    except OSError:
        return False
    return False


def bound_checkout_owner(root: Path, artifact_root: Path) -> CheckoutOwner | None:
    """``root``'s recorded owner when the record binds to ``root``.

    The record's kind must be the kind of ``root``'s place under
    ``artifact_root``, and the record's owner must give ``root``'s name.
    """
    owner = read_checkout_owner(root)
    if owner is not None and owner.kind == "toolchains":
        return (
            owner
            if owner.path == artifact_root and _binds(owner.kind, root, owner.path)
            else None
        )
    try:
        parts = root.relative_to(artifact_root).parts
    except ValueError:
        return None
    owner = read_checkout_owner(root)
    if (
        owner is None
        or owner.kind != checkout_state_kind_at(parts)
        or not _binds(owner.kind, root, owner.path)
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
    if os.path.normcase(target.parent.name) == "sessions":
        target = target.parent.parent
    if (
        os.path.normcase(target.parent.name) == "checkouts"
        and os.path.normcase(target.parent.parent.name) == "target"
    ):
        return target
    return None
