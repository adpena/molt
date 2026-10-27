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

from collections.abc import Generator, Iterable, Mapping, Sequence
from contextlib import contextmanager, ExitStack, suppress
from dataclasses import dataclass
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

from .custody_layout import SCRATCH_DIRNAME
from .exact_json import ExactJsonError, read_exact, write_exact

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
# A pinned session's target holds isolated work (a benchmark, a test shard),
# not a checkout's lasting build, so it retires after a short idle time: the
# 6 hours that the disk guard's lane registry applied.
SESSION_IDLE_ENV = "MOLT_SESSION_TARGET_IDLE_HOURS"
DEFAULT_SESSION_IDLE_HOURS = 6.0
# The automatic GC runs at most once per interval per artifact root.
GC_INTERVAL_S = 24 * 3600.0
GC_STAMP = ("target", ".molt-checkout-state-gc")
_GC_LOCK = ("target", ".disk_guard", "checkout-state-gc.lock")
# `molt.dx.checkout_component`: a sanitized name prefix and 12 hex digits.
_COMPONENT = re.compile(r"[A-Za-z0-9_.-]{1,24}-[0-9a-f]{12}")
# `molt.build_state_layout.build_state_address`: a SHA-256 digest.
_ADDRESS = re.compile(r"[0-9a-f]{64}")
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


CheckoutStateKind = Literal[
    "cargo-target",
    "uv-environment",
    "python-environment",
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
            "python-environment": ("build-environments", "development"),
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
    name = os.path.normcase(name)
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

    return os.path.normcase(checkout_component(owner))


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


def _binds(kind: CheckoutStateKind, root: Path, owner: Path) -> bool:
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
    kind: CheckoutStateKind


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
    names = tuple(os.path.normcase(part) for part in root.parts)
    containers = (
        *(
            (*container, "*") if kind == "python-environment" else container
            for kind, container in CHECKOUT_STATE_CONTAINERS.items()
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
            selected if isinstance(selected, _ManagedStateRoot) else qualify(selected)
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
                if _managed_coordinate(coordinate.root) != coordinate:
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
        if coordinate is not None and coordinate.kind == "python-environment":
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
) -> tuple[tuple[Path, CheckoutOwner | None], ...]:
    """Declared state and this process's environment, without a child command.

    Suite owners and a detached worker acknowledging its own use hold this
    explicit projection. Launch owners must use ``checkout_state_command_roots``.
    Activation is a declared resource selection, never interpreter identity.
    """
    from .build_state_layout import build_state_root

    roots: dict[Path, CheckoutOwner | None] = {}
    working = Path.cwd() if cwd is None else Path(cwd).absolute()
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_BUILD_STATE_DIR",
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
    return tuple(roots.items())


def checkout_state_command_roots(
    source_root: Path,
    environment: Mapping[str, str],
    command: Sequence[str],
    *,
    cwd: str | os.PathLike[str] | None = None,
) -> tuple[tuple[Path, CheckoutOwner | None], ...]:
    """Retain finite declared and possible native command resources.

    Candidate directories conservatively retain a published environment even
    when another PATH entry wins. This does not select or admit an executable.
    Ordinary unmanaged coordinates receive no filesystem probe; only a bounded
    enclosing published-image coordinate enters the existing custody authority.
    """
    from .process_launch import command_resources

    roots = dict(checkout_state_environment_roots(source_root, environment, cwd=cwd))
    working = Path.cwd() if cwd is None else Path(cwd).absolute()
    _retain_python_resources(
        roots, command_resources(command, environment, cwd=working)
    )
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


def _record_checkout_state_use(root: Path, *, now: float | None = None) -> None:
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
        if not root.is_dir():
            return
        try:
            metadata.mkdir(parents=True, exist_ok=True)
            stamp.touch()
        except OSError:
            return
    except OSError:
        return
    try:
        os.utime(stamp, (moment, moment))
    except OSError:
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


@dataclass(frozen=True, slots=True)
class RetentionLimits:
    """How long a root may stay unused before the GC retires it."""

    max_age_s: float
    session_idle_s: float


def retention_limits(environment: Mapping[str, str]) -> RetentionLimits:
    """The idle limits ``environment`` declares; an invalid one raises ValueError."""
    return RetentionLimits(
        max_age_s=_idle_limit_s(
            environment, MAX_AGE_ENV, DEFAULT_MAX_AGE_DAYS, 86400.0, "days"
        ),
        session_idle_s=_idle_limit_s(
            environment, SESSION_IDLE_ENV, DEFAULT_SESSION_IDLE_HOURS, 3600.0, "hours"
        ),
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


RetentionReason = Literal["owner-gone", "max-age", "session-idle"]


def checkout_state_decision(
    root: Path, artifact_root: Path, *, now: float, limits: RetentionLimits
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
    if (
        checkout_state_kind_of(root, artifact_root) is None
        and target_use_artifact_root(root) != artifact_root
    ):
        return None
    idle = now - checkout_state_last_use(root)
    if idle < PROTECTION_WINDOW_S:
        return None
    if idle >= limits.max_age_s:
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
    lasting longer than the interval. Failed/interrupted attempts keep the
    existing daily throttle. Explicit collection retains its own root locks.
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
            yield True
    finally:
        _release_file_lock(handle)


def record_checkout_owner(root: Path, *, owner: Path, kind: CheckoutStateKind) -> None:
    with checkout_state_use(((root, CheckoutOwner(owner, kind)),), publish=False):
        _record_checkout_owner(root, owner=owner, kind=kind)


def _record_checkout_owner(root: Path, *, owner: Path, kind: CheckoutStateKind) -> None:
    """Publish ``root``'s owner once; an existing record stays unchanged.

    A record that would not bind to ``root``'s name is never written.
    """
    record = _state_metadata_root(root) / OWNER_RECORD
    if record.is_file() or not root.is_dir():
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
