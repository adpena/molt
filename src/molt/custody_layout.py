"""Custody layout: which durable root owns a Molt checkout's state.

A checkout family is `<root>/molt-src` plus `<root>/worktrees/<name>`. Every
member resolves to the custody root `<root>`, which owns build artifacts,
toolchains, guard state, and scratch. A clone outside that layout is its own
custody root; its scratch then goes to a per-checkout folder under the host
temp root, never into the source tree, where runtime fixtures and source
custody reject it.

Git metadata fences automatic roots out of enclosing checkouts. This module
reads no environment and launches no Git process: `molt.dx` resolves artifact
and scratch overrides separately.
"""

from __future__ import annotations

import hashlib
import os
import re
import stat
import tempfile
import sys
from dataclasses import dataclass
from typing import Literal, assert_never, get_args
from functools import cache
from pathlib import Path
from collections.abc import Mapping

# Relative: the release bootstrap admits this module in memory before any
# `molt` package is importable (packaging/bootstrap.py).
from .path_custody import host_path_is_within, resolved_host_path_key

MAIN_CHECKOUT_DIRNAME = "molt-src"
WORKTREES_DIRNAME = "worktrees"
# Disk scratch is this child of the artifact root.
SCRATCH_DIRNAME = "tmp"
# Git bundles of retired, unlanded branches: a child of the custody root.
BUNDLES_DIRNAME = "drift-bundles"
# The SPRAWL threshold (M67): at most this many live worktrees, and `lane
# start` refuses a lane that would pass it (owner, 2026-10-22: 12).
LIVE_WORKTREE_LIMIT = 12

# Storage identity is closed here, including the existing checkout-state subset.
# Layout consumers project this authority; they must not register private roots.
CheckoutStateKind = Literal[
    "cargo-target",
    "proof-supervisor",
    "uv-environment",
    "python-environment",
    "pytest-cache",
    "hypothesis-home",
    "build-control",
    "scratch",
]
EvidenceClass = Literal[
    "lane-evidence",
    "integration-receipts",
    "diagnostics",
    "benchmark-evidence",
    "scratch-evidence",
    "sccache-server-evidence",
]
ToolchainStateKind = Literal["toolchains"]
ProviderCacheKind = Literal["pip-cache", "ruff-cache"]
PackageEnvironmentKind = Literal["browser-environment", "edgebox-environment"]
OwnedStateKind = Literal[
    CheckoutStateKind, ToolchainStateKind, ProviderCacheKind, PackageEnvironmentKind
]
ReplacedStorageClass = Literal[
    "replaced-controllers",
    "replaced-git-landing-sandbox",
    "replaced-family-landing-sandbox",
    "replaced-integration-receipts",
    "replaced-lane-evidence",
    "replaced-compile-cache",
    "replaced-temporary",
    "replaced-identity-state",
]
StorageClass = Literal[
    CheckoutStateKind,
    EvidenceClass,
    "compile-cache",
    "uv-cache",
    ProviderCacheKind,
    PackageEnvironmentKind,
    "sccache",
    ToolchainStateKind,
    "controllers",
    "landing-sandbox",
    "coordination",
    "temporary",
    "products",
    "installation",
    "run-control",
    "backend-sockets",
    "external-provider",
    ReplacedStorageClass,
]
StorageVolume = Literal["artifact", "checkout", "user-home", "host-temp"]


def legacy_host_temp_roots(environment: Mapping[str, str]) -> tuple[Path, ...]:
    """Host defaults replaced by DX scratch, independent of rebound TMPDIR.

    Arbitrary historical operator overrides require --legacy-temp-root. These
    are host storage coordinates; they never describe a compilation target.
    """
    if sys.platform == "darwin":
        # Darwin's public <unistd.h> _CS_DARWIN_USER_TEMP_DIR. CPython does
        # not expose its name in confstr_names, but accepts the ABI constant.
        selected = os.confstr(65537)
        if selected is None:
            raise ValueError("Darwin did not supply its user temp authority")
        return (Path(selected).resolve(),)
    if os.name == "nt":
        local = environment.get("LOCALAPPDATA")
        return () if not local else (Path(local) / "Temp",)
    return (Path("/tmp"), Path("/var/tmp"))


@dataclass(frozen=True)
class Regenerable:
    max_idle_s: float
    # None retains the existing provider's age/pressure policy, without
    # inventing a byte limit that its executor does not enforce.
    max_bytes: int | None
    keep_recent: int = 0


@dataclass(frozen=True)
class DurableEvidence:
    # Above this size, payload belongs on the artifact volume. An index and
    # coordination records remain at the original coordinate indefinitely.
    checkout_bytes: int = 1024 * 1024
    compact_after_s: float | None = None


@dataclass(frozen=True)
class ExternalProviderRetention:
    """Provider policy is descriptive; it never grants Molt collection custody."""

    cargo: str = "Cargo global-cache GC: 3 months downloaded, 1 month regenerable; disabled offline"
    npm: str = "provider/user invokes npm cache verify; no automatic age collector"
    rustup: str = (
        "provider/user manages installed toolchains; rustup toolchain uninstall"
    )


@dataclass(frozen=True)
class StorageRule:
    volume: StorageVolume
    owner: str
    relative: tuple[str, ...]
    retention: Regenerable | DurableEvidence | ExternalProviderRetention
    # Finite former producer coordinates under each named class's selected
    # root. Current versions stay outside this inventory; the executor refuses
    # any recovery journal encountered in a former namespace.
    replaced_namespaces: tuple[tuple[StorageClass, str], ...] = ()


@dataclass(frozen=True)
class StorageLocation:
    """A resolved placement, including a provider's checkout-custody default."""

    kind: StorageClass
    path: Path
    rule: StorageRule


def storage_rule(kind: StorageClass) -> StorageRule:
    """The volume, producer and retention of every Molt state class.

    An added Literal without a policy fails ty's exhaustive match. Byte/idle
    limits are defaults; existing cache/checkouts configuration remains the
    policy override boundary. No evidence policy grants deletion authority.
    """
    gib = 1024**3
    month = 30 * 86400.0
    match kind:
        case "replaced-identity-state":
            return StorageRule(
                "artifact",
                "durable-identity producers",
                (),
                DurableEvidence(compact_after_s=month),
                (
                    ("compile-cache", "file-digests/v[12].sqlite"),
                    ("compile-cache", "file-digests/v[12].sqlite-wal"),
                    ("compile-cache", "file-digests/v[12].sqlite-shm"),
                    ("compile-cache", "toolchain-facts/v[12].json"),
                    ("compile-cache", "source_hash_cache/[0-9a-f][0-9a-f]"),
                    ("compile-cache", "source_hash_cache/v2"),
                    ("compile-cache", "*.sync.json"),
                    ("compile-cache", "*.sync.v4.json"),
                    ("run-control", "gs/pending/" + "[0-9a-f]" * 32),
                    ("run-control", "gs/removing/[0-9a-f]*"),
                    ("build-control", "*/build_records/[0-9a-f][0-9a-f]"),
                    ("build-control", "*/build_records/.transient"),
                    ("build-control", "*/build_records/v11"),
                    ("build-control", "*/artifact_sync/*.json"),
                    ("build-control", "*/artifact_sync/v4"),
                    (
                        "build-control",
                        "*/backend_daemon/suite-leases/" + "[0-9a-f]" * 32,
                    ),
                    ("build-control", "*/backend_daemon/suite-leases/v2"),
                    (
                        "replaced-identity-state",
                        "logs/runtime_wasm_final_preflight/*.json",
                    ),
                    ("replaced-identity-state", "logs/runtime_wasm_final_preflight/v4"),
                ),
            )
        case "external-provider":
            return StorageRule("user-home", "provider", (), ExternalProviderRetention())
        case "replaced-controllers":
            return StorageRule(
                "checkout",
                "controller-source",
                (".git", "molt", "controllers"),
                DurableEvidence(compact_after_s=86400.0),
            )
        case "replaced-git-landing-sandbox":
            return StorageRule(
                "checkout",
                "landing-queue",
                (".git", "molt", "landing-sandbox"),
                DurableEvidence(compact_after_s=86400.0),
            )
        case "replaced-family-landing-sandbox":
            return StorageRule(
                "checkout",
                "landing-queue",
                ("worktrees",),
                DurableEvidence(compact_after_s=86400.0),
            )
        case "replaced-integration-receipts":
            return StorageRule(
                "checkout",
                "landing-gates",
                (".git", "molt-integrate"),
                DurableEvidence(compact_after_s=3 * 86400.0),
            )
        case "replaced-lane-evidence":
            return StorageRule(
                "checkout",
                "coordination-records",
                ("logs", "agents"),
                DurableEvidence(compact_after_s=3 * 86400.0),
            )
        case "replaced-compile-cache":
            return StorageRule(
                "user-home",
                "cache-eviction",
                (".cache", "molt"),
                DurableEvidence(compact_after_s=month),
            )
        case "replaced-temporary":
            return StorageRule(
                "host-temp",
                "legacy Molt writers",
                (),
                DurableEvidence(compact_after_s=6 * 3600.0),
            )
        case "cargo-target":
            return StorageRule(
                "artifact",
                "cargo",
                ("target", "checkouts"),
                Regenerable(month, None),
            )
        case "proof-supervisor":
            return StorageRule(
                "artifact",
                "native proof-supervisor provisioning",
                ("proof-supervisor",),
                Regenerable(month, None),
            )
        case "uv-environment":
            return StorageRule(
                "artifact",
                "checkout-provisioning",
                ("uv-project-envs",),
                Regenerable(month, None),
            )
        case "python-environment":
            return StorageRule(
                "artifact",
                "checkout-provisioning",
                ("build-environments", "development"),
                Regenerable(month, None),
            )
        case "browser-environment":
            return StorageRule(
                "checkout",
                "checkout-provisioning",
                ("tools", "browser_asset_graph", "node_modules"),
                Regenerable(month, gib),
            )
        case "edgebox-environment":
            return StorageRule(
                "checkout",
                "checkout-provisioning",
                (
                    "examples",
                    "edgebox",
                    "packages",
                    "edgebox-cloudflare",
                    "node_modules",
                ),
                Regenerable(month, gib),
            )
        case "pytest-cache":
            return StorageRule(
                "artifact",
                "pytest",
                (SCRATCH_DIRNAME, "pytest-cache"),
                Regenerable(month, None),
            )
        case "hypothesis-home":
            return StorageRule(
                "artifact",
                "hypothesis",
                (SCRATCH_DIRNAME, "hypothesis"),
                Regenerable(month, None),
            )
        case "build-control":
            return StorageRule(
                "artifact",
                "build-state-layout",
                (SCRATCH_DIRNAME, "build-control"),
                Regenerable(month, None),
            )
        case "scratch" | "temporary":
            return StorageRule(
                "artifact",
                "run-context",
                (SCRATCH_DIRNAME,),
                Regenerable(3 * 86400.0, None),
            )
        case "compile-cache":
            return StorageRule(
                "artifact",
                "cache-eviction",
                (".molt_cache",),
                Regenerable(month, 30 * gib),
            )
        case "uv-cache":
            return StorageRule(
                "artifact",
                "uv cache prune",
                (".uv-cache",),
                Regenerable(month, 10 * gib),
            )
        case "pip-cache":
            return StorageRule(
                "artifact", "pip", (".pip-cache",), Regenerable(month, 2 * gib)
            )
        case "ruff-cache":
            return StorageRule(
                "artifact", "ruff", (".ruff-cache",), Regenerable(month, gib)
            )
        case "sccache":
            return StorageRule(
                "artifact", "sccache", (".sccache",), Regenerable(month, 10 * gib)
            )
        case "toolchains":
            # Executable placement is a measured decision, independent of build
            # outputs. DX's canonical_toolchain_root owns explicit overrides.
            return StorageRule(
                "checkout",
                "tool-release provisioning",
                ("target-root",),
                Regenerable(month, 15 * gib),
            )
        case "controllers":
            return StorageRule(
                "artifact",
                "controller-source",
                ("molt-state", "controllers"),
                Regenerable(86400.0, None, 4),
            )
        case "landing-sandbox":
            return StorageRule(
                "artifact",
                "landing-queue",
                ("worktrees",),
                Regenerable(86400.0, None, 1),
            )
        case "scratch-evidence":
            return StorageRule(
                "artifact", "process guards", ("evidence", "scratch"), DurableEvidence()
            )
        case "sccache-server-evidence":
            return StorageRule(
                "artifact", "sccache server", ("sct",), DurableEvidence()
            )
        case "lane-evidence":
            return StorageRule(
                "artifact",
                "coordination-records",
                ("evidence", "lanes"),
                DurableEvidence(compact_after_s=3 * 86400.0),
            )
        case "integration-receipts":
            return StorageRule(
                "artifact",
                "landing-gates",
                ("evidence", "integration"),
                DurableEvidence(),
            )
        case "benchmark-evidence":
            return StorageRule(
                "artifact", "benchmark tools", ("bench", "results"), DurableEvidence()
            )
        case "diagnostics":
            return StorageRule(
                "artifact", "process/maintenance guards", ("logs",), DurableEvidence()
            )
        case "run-control":
            return StorageRule(
                "artifact",
                "process/pytest guards",
                (SCRATCH_DIRNAME,),
                DurableEvidence(),
            )
        case "backend-sockets":
            return StorageRule(
                "checkout", "backend-daemon", (), Regenerable(7200, None)
            )
        case "coordination":
            return StorageRule(
                "checkout",
                "coordination-records",
                ("logs", "agents"),
                DurableEvidence(),
            )
        case "products":
            return StorageRule(
                "artifact",
                "product-store",
                ("products",),
                Regenerable(5 * 86400.0, None),
            )
        case "installation":
            return StorageRule("checkout", "installer/user", (), DurableEvidence(0))
        case _:
            assert_never(kind)


def storage_classes() -> tuple[StorageClass, ...]:
    return get_args(StorageClass)


# Historical checkout evidence is never a generic cleanup target. New payload
# uses its external storage class; explicit checkout retirement archives these
# old/source-local coordinates before removing their source checkout.
CHECKOUT_EVIDENCE_PATHS: tuple[tuple[EvidenceClass, tuple[str, ...]], ...] = (
    ("diagnostics", ("logs",)),
    ("diagnostics", ("bench", "results")),
    ("diagnostics", ("bench", "scoreboard", "host_calibration")),
    ("diagnostics", ("tests", "harness", "reports")),
    ("diagnostics", ("tmp",)),
)


def repository_storage_key(common_dir: Path) -> str:
    return hashlib.sha256(os.fsencode(resolved_host_path_key(common_dir))).hexdigest()[
        :16
    ]


# Lanes reuse the slot worktrees `<root>/worktrees/slot-01` to `slot-12`. A
# released slot stays registered and detached, so the next lane inherits its
# path-keyed Cargo target, sccache keys and checkout inputs warm (HF-491). A
# free slot is no live worktree.
LANE_SLOT_LIMIT = LIVE_WORKTREE_LIMIT
_LANE_SLOT_NAME = re.compile(r"slot-([0-9]{2})")
_GIT_PATH_BYTES = 64 * 1024


def _read_git_path(path: Path) -> str:
    with path.open("rb") as stream:
        raw = stream.read(_GIT_PATH_BYTES + 1)
    if len(raw) > _GIT_PATH_BYTES:
        raise ValueError(
            f"Git path metadata exceeds {_GIT_PATH_BYTES} byte limit: {path}"
        )
    # Git path records end with LF (or CRLF), not arbitrary whitespace.
    # Filesystem decoding preserves non-ASCII and Unix path bytes.
    return os.fsdecode(raw.removesuffix(b"\n").removesuffix(b"\r"))


def git_directories(repo_root: Path) -> tuple[Path, Path] | None:
    """Read the checkout's Git/common directories without launching Git.

    Absence is a plain source tree. Present but unreadable, malformed or stale
    metadata cannot authorize an automatic output placement.
    """
    dot_git = repo_root / ".git"
    try:
        try:
            metadata = dot_git.lstat()
        except FileNotFoundError:
            return None
        if stat.S_ISDIR(metadata.st_mode):
            git_dir = dot_git
        elif dot_git.is_dir():
            git_dir = dot_git.resolve()
        else:
            if not dot_git.is_file():
                raise ValueError(
                    f"Git checkout metadata is not a regular file: {dot_git}"
                )
            pointer = _read_git_path(dot_git)
            if not pointer.startswith("gitdir: ") or not (
                target := pointer.removeprefix("gitdir: ")
            ):
                raise ValueError(f"malformed Git checkout metadata: {dot_git}")
            git_dir = (repo_root / target).resolve()
        if not git_dir.is_dir() or not (git_dir / "HEAD").is_file():
            raise ValueError(f"stale Git checkout metadata: {dot_git}")
        commondir = git_dir / "commondir"
        try:
            commondir.lstat()
        except FileNotFoundError:
            common = git_dir
        else:
            if not commondir.is_file():
                raise ValueError(
                    f"Git common directory metadata is not a regular file: {commondir}"
                )
            common_target = _read_git_path(commondir)
            if not common_target:
                raise ValueError(f"malformed Git common directory: {commondir}")
            common = (git_dir / common_target).resolve()
            if not common.is_dir():
                raise ValueError(f"stale Git common directory: {commondir}")
    except (OSError, UnicodeError, RuntimeError) as exc:
        raise ValueError(f"cannot read Git checkout metadata: {dot_git}") from exc
    return git_dir, common


@cache
def checkout_family_state_root(repo_root: Path) -> Path | None:
    """Existing repository control records, shared by all linked worktrees.

    Git path metadata is immutable for the lifetime of a checkout process.
    Source distributions without Git metadata have no durable family record.
    """
    directories = git_directories(repo_root)
    return None if directories is None else directories[1] / "molt"


def enclosing_checkouts(path: str | Path) -> tuple[Path, ...]:
    """Return Git checkout boundaries from the given path outwards.

    A linked worktree's .git pointer is admitted by the same reader as DX HEAD
    inspection. Bare repositories have no working-tree marker and are not
    source checkout boundaries.
    """
    root = Path(path).expanduser().resolve()
    return tuple(
        candidate
        for candidate in (root, *root.parents)
        if git_directories(candidate) is not None
    )


def coordination_state_root(source: Path) -> Path:
    """The existing coordination lock namespace must outlive linked source.

    Named families already have an external custody root. An unconfigured
    linked checkout uses its common Git custody instead of materializing
    control files inside the checkout that retirement is removing.
    """
    selected = custody_root(source)
    if selected == source:
        try:
            directories = git_directories(source)
        except (OSError, ValueError, RuntimeError):
            directories = None
        if directories is not None and directories[0] != directories[1]:
            selected = directories[1] / "molt"
    return selected


def custody_root(repo_root: str | Path) -> Path:
    """Return automatic custody without entering another source checkout."""
    root = Path(repo_root).expanduser().resolve()
    if root.name == MAIN_CHECKOUT_DIRNAME:
        custody = root.parent
    elif root.parent.name == WORKTREES_DIRNAME:
        custody = root.parent.parent
    else:
        custody = root
    for checkout in enclosing_checkouts(root):
        # A standalone plain clone retains Cargo's repo-local output norm.
        # An enclosing checkout is another source owner, even for an unrelated
        # nested clone rather than a Git-linked worktree.
        if checkout != root and host_path_is_within(custody, checkout):
            custody = checkout.parent
    return custody


def repository_custody_root(repo_root: str | Path) -> Path:
    """Return the custody of the repository that owns this checkout.

    A linked worktree belongs to the main checkout of its Git common
    directory, wherever its own source lives. A capacity-selected volume can
    hold a worktree (the landing sandbox), so state that follows the
    repository, such as provisioned toolchains, reads this root and never the
    worktree's location. A bare common directory has no main checkout; the
    worktree's own custody stands.
    """
    root = Path(repo_root).expanduser().resolve()
    directories = git_directories(root)
    if directories is not None:
        git_dir, common_dir = directories
        if git_dir != common_dir and common_dir.name == ".git":
            return custody_root(common_dir.parent)
    return custody_root(root)


def family_worktrees_root(main_checkout: str | Path) -> Path | None:
    """`<root>/worktrees` for a main checkout at `<root>/molt-src`, else None.

    A lane worktree there resolves to the same custody root as the main
    checkout. A clone outside the family layout has no such place: its own
    directory would nest the worktrees inside its source tree.
    """
    root = Path(main_checkout).expanduser().resolve()
    if root.name != MAIN_CHECKOUT_DIRNAME:
        return None
    return root.parent / WORKTREES_DIRNAME


def landing_sandbox_path(
    common_dir: str | Path,
    *,
    artifact: Path,
) -> Path:
    """One repository's integration source, outside every other checkout.

    Queue control remains in the Git common directory. Its source must not
    inherit a primary checkout's Cargo configuration through that directory.
    The common-directory identity keeps unrelated repositories distinct, even
    when they share an external worktree parent or use separate Git metadata.
    """
    common = Path(common_dir).expanduser().resolve()
    external = artifact.resolve(strict=False)
    repository_key = repository_storage_key(common)
    candidate = external / WORKTREES_DIRNAME / f"landing-sandbox-{repository_key}"
    selected = candidate.resolve(strict=False)
    if selected != candidate:
        raise ValueError(
            f"landing sandbox source cannot contain symlink or path indirection: {candidate}"
        )
    if any(owner != selected for owner in enclosing_checkouts(candidate)):
        raise ValueError(
            f"landing sandbox source is inside another source checkout: {candidate}"
        )
    return selected


def lane_slot_path(worktrees_root: str | Path, index: int) -> Path:
    """The slot worktree path with number `index` (1-based)."""
    return Path(worktrees_root) / f"slot-{index:02d}"


def lane_slot_index(path: str | Path, worktrees_root: str | Path | None) -> int | None:
    """The slot number of `path` when it is a lane slot under `worktrees_root`."""
    if worktrees_root is None:
        return None
    candidate = Path(path)
    match = _LANE_SLOT_NAME.fullmatch(candidate.name)
    if match is None or not 1 <= int(match.group(1)) <= LANE_SLOT_LIMIT:
        return None
    if candidate.resolve().parent != Path(worktrees_root).resolve():
        return None
    return int(match.group(1))


def _identity_name(path: str | Path) -> str:
    resolved = Path(path).expanduser().resolve()
    identity = hashlib.sha256(str(resolved).encode("utf-8")).hexdigest()[:12]
    return f"molt-{identity}"


def out_of_tree_scratch_root(source_root: str | Path) -> Path:
    """Return the per-checkout scratch folder under the host temp root.

    `RunContext` exports this folder as the child's `TMPDIR`. A child whose
    temp root already lies in it resolves the same folder, so a parent and its
    children agree on one scratch root.
    """

    name = _identity_name(source_root)
    base = Path(tempfile.gettempdir()).resolve()
    for candidate in (base, *base.parents):
        if candidate.name == name:
            return candidate
    return base / name


def disk_scratch_roots_of(artifact_root: str | Path) -> tuple[Path, Path]:
    """Return both disk scratch roots an artifact root can have.

    A checkout outside the root keeps scratch at `<root>/tmp`; a checkout
    that is its own artifact root keeps it in its out-of-tree folder. An
    observer that sees only the artifact root (a reclaimer, a preflight)
    must look at both.
    """

    root = Path(artifact_root).expanduser().resolve()
    return (root / SCRATCH_DIRNAME, out_of_tree_scratch_root(root))


def scratch_root(
    artifact_root: str | Path,
    source_root: str | Path,
    *,
    storage_root: str | Path | None = None,
) -> Path:
    """Return the scratch root for an artifact root, never inside the checkout.

    Disk storage is `<artifact root>/tmp`, or the per-checkout host temp
    folder when that path falls inside the checkout. An explicitly selected
    disk or memory root holds one folder per artifact root.
    """

    if storage_root is not None:
        return Path(storage_root) / _identity_name(artifact_root)
    scratch = Path(artifact_root) / SCRATCH_DIRNAME
    if host_path_is_within(scratch, source_root):
        return out_of_tree_scratch_root(source_root)
    return scratch
