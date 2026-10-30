"""Storage inventory, last-use retirement and explicit legacy migration.

Policy and placement belong to molt.custody_layout/molt.dx. This is an executor
under the dev driver, not another root or retention registry. Planning is read
only; migration never runs as a test, startup hook or side effect of GC.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping, Callable
from contextlib import ExitStack
from dataclasses import asdict, dataclass
import json
import hashlib
import os
from pathlib import Path
import shutil
import stat
import time
import tempfile
from typing import Literal

from molt.checkout_state import (
    checkout_state_last_use,
    checkout_state_retirement,
    compact_evidence,
    PROTECTION_WINDOW_S,
)
from molt.custody_layout import (
    DurableEvidence,
    ExternalProviderRetention,
    Regenerable,
    StorageClass,
    git_directories,
    storage_classes,
    storage_rule,
    PackageEnvironmentKind,
)
from molt.dx import artifact_root, storage_path, storage_location
from molt.file_locks import (
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.file_publication import resolve_owned_path
from tools.driver_errors import DriverError

# Controller retention: the newest CONTROLLER_KEEP controllers stay; an older one
# retires only after CONTROLLER_IDLE_S without use and no live shared source owner.
_CONTROLLER_RETENTION = storage_rule("controllers").retention
assert isinstance(_CONTROLLER_RETENTION, Regenerable)
CONTROLLER_KEEP = _CONTROLLER_RETENTION.keep_recent
CONTROLLER_IDLE_S = _CONTROLLER_RETENTION.max_idle_s


def allocated_bytes(root: Path) -> int:
    """Allocated bytes, without following links or traversing other volumes."""
    if not root.exists():
        return 0
    device = root.lstat().st_dev
    total = 0
    stack = [root]
    while stack:
        path = stack.pop()
        info = path.lstat()
        if info.st_dev != device:
            raise ValueError(f"storage inventory crossed a volume: {path}")
        total += getattr(info, "st_blocks", (info.st_size + 511) // 512) * 512
        if stat.S_ISDIR(info.st_mode):
            stack.extend(path.iterdir())
    return total


def indexed_payload(path: Path) -> bool:
    """A retired raw directory holds only a durable, externally backed index."""
    from molt.checkout_state import EVIDENCE_INDEX_NAME
    from molt.exact_json import read_exact

    if path.is_symlink():
        return False
    if path.is_dir():
        children = list(path.iterdir())
        return bool(children) and all(indexed_payload(child) for child in children)
    if path.name != EVIDENCE_INDEX_NAME and not path.name.endswith(
        "." + EVIDENCE_INDEX_NAME
    ):
        return False
    try:
        record = read_exact(path, max_bytes=64 * 1024, label="payload index")
        if (
            not isinstance(record, dict)
            or record.get("schema") != "molt.evidence-index.v1"
            or record.get("class")
            not in {"lane-evidence", "diagnostics", "integration-receipts"}
        ):
            return False
        source = Path(record["source"])
        if path != (
            source / EVIDENCE_INDEX_NAME
            if source.is_dir()
            else source.with_name(source.name + "." + EVIDENCE_INDEX_NAME)
        ):
            return False
        archive = Path(record["archive"])
        manifest = read_exact(
            archive.with_suffix(".index.json"),
            max_bytes=64 * 1024 * 1024,
            label="payload manifest",
        )
        return (
            archive.is_file()
            and isinstance(manifest, dict)
            and all(manifest.get(key) == value for key, value in record.items())
        )
    except (OSError, ValueError, TypeError, KeyError):
        return False


def _retain_source_index(source: Path, directory: Path) -> Path:
    """Move a verified source index to surviving custody, then drop an empty shell."""
    import uuid
    from molt.checkout_state import evidence_index_path
    from molt.file_publication import durable_namespace_replace

    index = directory / f"{uuid.uuid4().hex}.source.json"
    directory.mkdir(parents=True, exist_ok=True)
    durable_namespace_replace(evidence_index_path(source), index)
    if source.is_dir():
        # A late uncooperative writer leaves both its new signal and the
        # completed archive intact; never recursively remove this shell.
        source.rmdir()
    return index


def harvest_lane_payload(
    repo: Path, task: str, paths: list[str], environment: Mapping[str, str]
) -> list[dict[str, str]]:
    """Preserve unknown ignored signal through the canonical verified compactor.

    Callers own the lane lifecycle lock. Each source also takes its existing
    storage lifetime lock; active guards and native locks preserve live writers.
    Recovery indices remain in source and the surviving common Git directory.
    """
    from tools.coordination_records import task_artifacts_dir
    from tools.disk_guard import _guard_protection, _scratch_inventory, _region_locked
    from molt.exact_json import write_exact

    directories = git_directories(repo)
    if directories is None:
        raise DriverError("payload harvest needs the lane's Git custody")
    destination = task_artifacts_dir(repo, task, environment).parent / "ignored"
    guards = _guard_protection((repo, artifact_root(repo, environment)))
    result = []
    with ExitStack() as custody:
        # Acquire the whole batch before moving anything: no partial harvest
        # when a second root is held by another cooperating writer.
        sources = []
        for relative in sorted(
            set(paths), key=lambda value: (len(Path(value).parts), value)
        ):
            source = repo / relative.rstrip("/")
            if any(parent == source or parent in source.parents for parent in sources):
                continue  # Git can name an ignored child and its whole directory.
            if (
                source.is_junction()
                or not source.absolute().is_relative_to(repo.absolute())
                or ".." in Path(relative).parts
            ):
                raise DriverError(
                    f"payload namespace needs custody reconciliation: {source}"
                )
            resolve_owned_path(source.parent)
            leaf_link = source.is_symlink()
            if not custody.enter_context(
                checkout_state_retirement(source.parent if leaf_link else source)
            ):
                raise DriverError(f"payload state is in use: {source}")
            if not leaf_link and guards.protects(
                source, artifact_root(repo, environment), recorded_owner=repo
            ):
                raise DriverError(f"payload guard is in use: {source}")
            if not leaf_link and source.is_dir():
                observation = _scratch_inventory(source, time.monotonic() + 60)
                if observation.lock_held or observation.guards:
                    raise DriverError(f"payload native writer is in use: {source}")
            elif source.name.endswith(".lock") or source.name in {
                "lock",
                ".cargo-lock",
                ".cargo-build-lock",
            }:
                if leaf_link or source.stat().st_size or _region_locked(source):
                    raise DriverError(
                        f"payload lock needs custody reconciliation: {source}"
                    )
            sources.append(source)
        for source in sources:
            archive = compact_evidence(
                source, destination, kind="lane-evidence", preserve_leaf_link=True
            )
            # A sidecar of an ignored file need not itself be ignored. Move
            # every source index to surviving Git custody, preserving its
            # contents and avoiding new untracked files in the lane.
            index = _retain_source_index(
                source, directories[1] / "molt" / "lane-payload-index"
            )
            result.append(
                {
                    "source": str(source),
                    "archive": str(archive),
                    "index": str(index),
                }
            )
        if result:
            import uuid

            write_exact(
                directories[1]
                / "molt"
                / "lane-payload-index"
                / f"{uuid.uuid4().hex}.json",
                {
                    "schema": "molt.lane-payload.v1",
                    "task": task,
                    "checkout": str(repo),
                    "payloads": result,
                },
            )
    return result


def inventory(repo: Path, environment: Mapping[str, str]) -> list[dict[str, object]]:
    rows = []
    for kind in storage_classes():
        selected = storage_location(repo, kind, environment)
        rule, path = selected.rule, selected.path
        row: dict[str, object] = {
            "class": kind,
            "path": str(path),
            "volume": rule.volume,
            "owner": rule.owner,
            "retention": asdict(rule.retention),
            "durable": isinstance(rule.retention, DurableEvidence),
        }
        if isinstance(rule.retention, ExternalProviderRetention):
            providers = external_providers(repo, environment)
            row["providers"] = providers
            row["bytes"] = sum(
                member["bytes"]
                for member in providers
                if isinstance(member.get("bytes"), int)
            )
        if kind == "sccache":
            from molt.cargo_execution_policy import sccache_server_temp_dir

            temporary = sccache_server_temp_dir(environment)
            row["server_temp"] = None if temporary is None else str(temporary)
        rows.append(row)
    return rows


def external_providers(
    repo: Path, environment: Mapping[str, str]
) -> list[dict[str, object]]:
    """Observe native providers, without changing their configuration or custody."""
    from molt.rust_toolchain import cargo_home_path, rustup_home
    from molt.process_environment import ProcessEnvironment
    from tools.molt_dev_common import _run_fast_captured_command

    policy = storage_rule("external-provider").retention
    assert isinstance(policy, ExternalProviderRetention)
    paths = [
        ("cargo", cargo_home_path(repo, environment), policy.cargo),
        ("rustup", rustup_home(environment, repo), policy.rustup),
    ]
    rows: list[dict[str, object]] = []
    npm = shutil.which("npm", path=environment.get("PATH", os.defpath))
    if npm is None:
        rows.append(
            {
                "provider": "npm",
                "status": "provider-unavailable",
                "retention": policy.npm,
            }
        )
    else:
        # npm owns npmrc, environment and platform precedence. Do not parse a
        # second configuration authority or invent a home after a failed probe.
        proc = _run_fast_captured_command(
            [npm, "config", "get", "cache", "--offline", "--logs-max=0"],
            cwd=repo,
            env=ProcessEnvironment(environment),
            timeout=30,
        )
        if (
            proc.returncode
            or not proc.stdout.strip()
            or not Path(proc.stdout.strip()).is_absolute()
        ):
            rows.append(
                {
                    "provider": "npm",
                    "status": "configuration-unknown",
                    "retention": policy.npm,
                    "detail": proc.stderr,
                }
            )
        else:
            paths.append(("npm", Path(proc.stdout.strip()), policy.npm))
    for provider, path, retention in paths:
        row: dict[str, object] = {
            "provider": provider,
            "owner": provider,
            "path": None if path is None else str(path),
            "retention": retention,
            "status": "observed" if path is not None else "home-unknown",
        }
        if path is not None:
            try:
                row["bytes"] = allocated_bytes(path)
            except OSError as exc:
                row["status"] = "size-unknown"
                row["detail"] = str(exc)
        rows.append(row)
    return rows


def retire_idle_controllers(
    directory: Path, *, current: Path, now: float, apply: bool = True
) -> list[dict[str, object]]:
    """Keep current/recent versions; lock and recheck older idle generations."""
    from tools import controller_source
    from tools.molt_dev_common import _warn

    entries = []
    rows = []
    for entry in directory.iterdir():
        if entry == current:
            rows.append(
                {"class": "controllers", "path": str(entry), "action": "current"}
            )
            continue
        try:
            admitted = controller_source.controller_for_source(entry, allow_staged=True)
        except DriverError:
            admitted = None
        if admitted is None:
            rows.append(
                {
                    "class": "controllers",
                    "path": str(entry),
                    "action": "unproven-preimage",
                }
            )
            continue
        marker = entry / controller_source.RECORD_NAME
        try:
            entries.append((marker.stat().st_mtime, entry, entry.name == admitted.key))
        except FileNotFoundError:
            continue
    entries.sort(reverse=True)
    recent = set(
        [entry for _, entry, published in entries if published][
            : max(0, CONTROLLER_KEEP - 1)
        ]
    )
    for last_use, entry, _published in entries:
        row = {
            "class": "controllers",
            "path": str(entry),
            "last_use": last_use,
            "action": "recent",
        }
        rows.append(row)
        if entry in recent or now - last_use < CONTROLLER_IDLE_S:
            continue
        lock_path = controller_source.source_lock_path(entry)
        try:
            handle = _try_acquire_file_lock(lock_path, create=apply)
        except FileNotFoundError:
            handle = None
        if handle is None:
            row["action"] = (
                "state-in-use"
                if lock_path.exists()
                else "unavailable-lock"
                if apply
                else "retire"
            )
            continue
        try:
            with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
                fresh = (entry / controller_source.RECORD_NAME).stat().st_mtime
                if now - fresh < CONTROLLER_IDLE_S:
                    row["action"] = "use-changed"
                else:
                    row["action"] = "retire"
                    if apply:
                        shutil.rmtree(entry)
        except OSError as exc:
            row.update(action="failed", error=str(exc))
            _warn(f"controller retention left {entry}: {exc}")
        finally:
            _release_file_lock(handle)
    return rows


def trim(
    repo: Path,
    environment: Mapping[str, str],
    *,
    apply: bool,
    include_compile_cache: bool = True,
    policy_source: Path | None = None,
    legacy_temp: Path | None = None,
    include_shared_preimages: bool = False,
) -> list[dict[str, object]]:
    """Run existing provider GC and version retention, never sweep evidence."""
    from molt.cache_eviction import _prune
    from molt.default_paths import resolve_budget
    from molt.tool_releases import load_tool_releases, tool_prefix
    from molt.llvm_toolchain import load_llvm_releases, wasi_sdk_install_prefix
    from molt.binaryen_toolchain import load_binaryen_manifest
    from molt.checkout_state import read_checkout_owner
    from typing import get_args
    from tools.molt_dev_common import _COMMANDS
    from tools import controller_source
    from tools.disk_guard import _guard_protection

    result = []
    source = repo if policy_source is None else policy_source
    max_gb, max_days = resolve_budget(None, None, environment)
    if include_compile_cache:
        result.append(
            dict(
                _prune(
                    storage_path(repo, "compile-cache", environment),
                    max_bytes=int(max_gb * 1024**3),
                    max_age_days=max_days,
                    dry_run=not apply,
                    environ=environment,
                )
            )
        )
    directory = storage_path(repo, "controllers", environment)
    if directory.is_dir():
        entries = []
        for entry in directory.iterdir():
            try:
                admitted = controller_source.controller_for_source(entry)
            except DriverError:
                admitted = None
            if admitted is not None:
                entries.append(entry)
        if entries:
            current = max(
                entries,
                key=lambda path: (path / ".molt-controller.json").stat().st_mtime,
            )
            result.extend(
                retire_idle_controllers(
                    directory, current=current, now=time.time(), apply=apply
                )
            )
        else:
            result.extend(
                retire_idle_controllers(
                    directory, current=directory, now=time.time(), apply=apply
                )
            )
    tool_root = storage_path(repo, "toolchains", environment)
    guards = _guard_protection((repo, artifact_root(repo, environment), tool_root))
    current = set(
        tool_prefix(tool_root, release)
        for release in load_tool_releases(source).values()
    )
    llvm = load_llvm_releases(source)
    current.update(
        tool_root / "toolchains" / f"llvm-{release.version}"
        for release in llvm.releases
    )
    current.update(
        wasi_sdk_install_prefix(tool_root, asset) for asset in llvm.wasi_sdk.targets
    )
    current.add(
        tool_root
        / "toolchains"
        / f"binaryen-version_{load_binaryen_manifest(source).release.version}"
    )
    from tools.disk_guard import registered_worktree_roots

    for kind in get_args(PackageEnvironmentKind):
        roots = [
            storage_path(checkout, kind, environment)
            for checkout in (registered_worktree_roots(repo) or (repo,))
        ]
        result.extend(
            retire_regenerable(
                roots,
                kind=kind,
                now=time.time(),
                apply=apply,
                live_protection=lambda path: guards.protects(
                    path, artifact_root(repo, environment), recorded_owner=repo
                ),
            )
        )
    versions = tool_root / "toolchains"
    if versions.is_dir():
        # Only installed, attested version producers qualify as regenerable.
        roots = [
            path
            for parent in (
                versions,
                versions / "sources",
                versions / "downloads",
                versions / "build",
            )
            if parent.is_dir()
            for path in parent.iterdir()
            if not path.is_symlink()
            and (owner := read_checkout_owner(path)) is not None
            and owner.kind == "toolchains"
            and owner.path == tool_root
        ]
        result.extend(
            retire_regenerable(
                roots,
                kind="toolchains",
                now=time.time(),
                apply=apply,
                current=frozenset(current),
                live_protection=lambda path: guards.protects(
                    path, tool_root, recorded_owner=repo
                ),
            )
        )
    uv_root = storage_path(repo, "uv-cache", environment)
    uv_policy = storage_rule("uv-cache").retention
    assert isinstance(uv_policy, Regenerable)
    assert uv_policy.max_bytes is not None
    size = allocated_bytes(uv_root)
    if size:
        # uv's internal objects and locks are exclusively its protocol. Never
        # unlink them to enforce Molt's budget. Report unsatisfied residual.
        if apply:
            proc = _COMMANDS.run(
                ["uv", "cache", "prune", "--cache-dir", str(uv_root)],
                cwd=repo,
                env=environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
            if proc.returncode:
                raise DriverError(proc.stderr)
        result.append(
            {
                "class": "uv-cache",
                "action": "provider-prune",
                "bytes": size,
                "budget": uv_policy.max_bytes,
                "remaining": allocated_bytes(uv_root) if apply else size,
            }
        )
    for kind in ("pip-cache", "ruff-cache"):
        cache = storage_path(repo, kind, environment)
        if cache.is_dir():
            if cache.name != storage_rule(kind).relative[0]:
                result.append(
                    {
                        "class": kind,
                        "path": str(cache),
                        "action": "custom-placement-needs-use-protocol",
                    }
                )
                continue
            result.extend(
                retire_regenerable([cache], kind=kind, now=time.time(), apply=apply)
            )
    # Repository preimages have one Git custody owner. Historical user-cache
    # and host-temp preimages require the operator's explicit inventory scope;
    # a test/SDK checkout never takes custody of the account's global history.
    result.extend(
        migrate(
            repo,
            environment,
            apply=apply,
            legacy_temp=legacy_temp,
            repository_only=not include_shared_preimages,
        )
    )
    return result


def retire_regenerable(
    roots: list[Path],
    *,
    kind: StorageClass,
    now: float,
    apply: bool,
    current: frozenset[Path] = frozenset(),
    live_protection: Callable[[Path], bool] | None = None,
) -> list[dict[str, object]]:
    """LRU byte/idle budget under the same locks held by command execution.

    Discovery is explicit and class-specific. This routine cannot authorize
    evidence deletion. Unknown links and current selections remain untouched.
    An entire budget may remain busy; the result reports that fact.
    """
    policy = storage_rule(kind).retention
    if not isinstance(policy, Regenerable):
        raise ValueError(f"durable evidence cannot be retired: {kind}")
    entries = sorted(
        (
            (checkout_state_last_use(root), root, allocated_bytes(root))
            for root in roots
            if (root.is_dir() or root.is_file()) and not root.is_symlink()
        ),
        reverse=True,
    )
    total = sum(size for _, _, size in entries)
    kept = current | frozenset(root for _, root, _ in entries[: policy.keep_recent])
    rows = []
    for last_use, root, size in reversed(entries):
        from molt.checkout_state import _managed_coordinate, read_checkout_owner

        coordinate = _managed_coordinate(root)
        owner = read_checkout_owner(root)
        if (
            coordinate is None
            or coordinate.kind != kind
            or owner is None
            or owner.kind != kind
        ):
            rows.append(
                {
                    "class": kind,
                    "path": str(root),
                    "bytes": size,
                    "last_use": last_use,
                    "action": "unproven-preimage",
                }
            )
            continue
        if kind in {"pip-cache", "ruff-cache"}:
            from molt.checkout_state import read_checkout_owner, CheckoutOwner

            if read_checkout_owner(root) != CheckoutOwner(root.parent, kind):
                rows.append(
                    {
                        "class": kind,
                        "path": str(root),
                        "bytes": size,
                        "last_use": last_use,
                        "action": "unproven-preimage",
                    }
                )
                continue
        reason = "current-or-recent" if root in kept else "within-budget"
        eligible = root not in kept and (
            now - last_use >= policy.max_idle_s
            or (policy.max_bytes is not None and total > policy.max_bytes)
        )
        # A two-hour grace covers the existing one-hour use-stamp resolution.
        eligible = eligible and now - last_use >= PROTECTION_WINDOW_S
        if eligible:
            with checkout_state_retirement(root) as exclusive:
                if not exclusive:
                    reason = "state-in-use"
                elif live_protection is not None and live_protection(root):
                    reason = "guard-in-use"
                elif checkout_state_last_use(root) != last_use:
                    reason = "use-changed"
                else:
                    reason = "retire"
                    if apply:
                        owned = resolve_owned_path(root)
                        if owned.is_dir():
                            shutil.rmtree(owned)
                        else:
                            owned.unlink()
                    total -= size
        rows.append(
            {
                "class": kind,
                "path": str(root),
                "bytes": size,
                "last_use": last_use,
                "action": reason,
            }
        )
    return rows


MigrationAction = Literal[
    "compact", "move-worktree", "retire-worktree", "already-moved", "keep"
]


def _tree_identity(root: Path) -> dict[str, object]:
    """Exact content/type oracle for a clean source relocation; no links followed."""
    result = {}
    for path in sorted(root.rglob("*")):
        metadata = path.lstat()
        name = path.relative_to(root).as_posix()
        if stat.S_ISLNK(metadata.st_mode):
            result[name] = ("link", os.readlink(path))
        elif stat.S_ISREG(metadata.st_mode):
            with path.open("rb") as source:
                result[name] = (
                    "file",
                    stat.S_IMODE(metadata.st_mode),
                    hashlib.file_digest(source, "sha256").hexdigest(),
                )
        elif stat.S_ISDIR(metadata.st_mode):
            result[name] = ("directory", stat.S_IMODE(metadata.st_mode))
        else:
            raise DriverError(f"nonportable sandbox payload: {path}")
    return result


def _move_sandbox(
    repo: Path, source: Path, destination: Path, *, apply: bool = True
) -> str:
    """Relocate a registered, clean worktree using its actual filesystem plan.

    Git's move is a rename and cannot cross volumes. The cross-volume plan
    copies, verifies, repairs Git's registration, verifies again and publishes
    a recovery index before removing the regenerable old checkout.
    """
    from tools.molt_dev_common import _COMMANDS
    from molt.disk_capacity import require_storage_capacity
    from molt.exact_json import write_exact
    from molt.file_publication import durable_namespace_replace

    expected = git_directories(repo)
    actual = git_directories(source)
    if (
        expected is None
        or actual is None
        or actual[1] != expected[1]
        or not (source / ".git").is_file()
    ):
        return "unregistered-source"

    def clean(path: Path) -> bool:
        status = _COMMANDS.run(
            [
                "git",
                "--no-optional-locks",
                "-C",
                str(path),
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored",
            ],
            cwd=repo,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        return status.returncode == 0 and not status.stdout.strip()

    if destination.exists() or not clean(source):
        return "dirty-source-or-destination-exists"
    backing = next(
        parent
        for parent in (destination.parent, *destination.parents)
        if parent.is_dir()
    )
    if not apply:
        return (
            "ready-cross-volume-copy"
            if source.stat().st_dev != backing.stat().st_dev
            else "ready-git-move"
        )
    require_storage_capacity(
        (destination,), kind="landing-sandbox", additional_bytes=allocated_bytes(source)
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    if source.stat().st_dev == destination.parent.stat().st_dev:
        moved = _COMMANDS.run(
            ["git", "-C", str(repo), "worktree", "move", str(source), str(destination)],
            cwd=repo,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        if moved.returncode:
            raise DriverError(moved.stderr)
        return "moved"
    before = _tree_identity(source)
    staging = Path(
        tempfile.mkdtemp(prefix=".sandbox-migration-", dir=destination.parent)
    )
    # Preserve an interrupted copy as recoverable signal; no cleanup on failure.
    shutil.copytree(source, staging, dirs_exist_ok=True, symlinks=True)
    if (
        _tree_identity(staging) != before
        or _tree_identity(source) != before
        or not clean(source)
    ):
        raise DriverError(
            f"sandbox changed while relocating; preserved {source} and {staging}"
        )
    durable_namespace_replace(staging, destination)
    repair = _COMMANDS.run(
        ["git", "-C", str(repo), "worktree", "repair", str(destination)],
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if (
        repair.returncode
        or git_directories(destination) != actual
        or not clean(destination)
    ):
        raise DriverError(
            f"sandbox registration repair failed; preserved {source} and {destination}: {repair.stderr}"
        )
    if _tree_identity(destination) != before or _tree_identity(source) != before:
        raise DriverError(
            f"sandbox identity changed after repair; preserved both copies: {destination}"
        )
    write_exact(
        source.with_name(source.name + ".storage-index.json"),
        {
            "schema": "molt.sandbox-relocation.v1",
            "source": str(source),
            "destination": str(destination),
            "files": before,
        },
    )
    shutil.rmtree(source)
    return "moved-across-volumes"


@dataclass(frozen=True)
class Migration:
    kind: StorageClass
    source: Path
    destination: Path
    action: MigrationAction
    bytes: int
    reason: str
    locks: tuple[Path, ...] = ()
    last_use: float | None = None


def _legacy_activity(path: Path, *, read_access: bool = True) -> tuple[float, bool]:
    """Full metadata observation; old creation time never proves idleness.

    Legacy writers have no use stamp. Read access to regular payload counts;
    directory atime does not, since an inventory itself reads directories.
    Native locks, nonterminal guards and special files keep the whole root.
    """
    from tools.disk_guard import _scratch_inventory, _region_locked

    if path.is_dir():
        observed = _scratch_inventory(path, time.monotonic() + 60)
        newest, held = observed.mtime, observed.lock_held or bool(observed.guards)
        entries = path.rglob("*")
    else:
        newest, held = path.lstat().st_mtime, False
        entries = (path,)
    for child in entries:
        info = child.lstat()
        newest = max(newest, info.st_mtime)
        if stat.S_ISREG(info.st_mode):
            if (
                read_access
                and child.name
                not in {
                    "coordination.json",
                    "MOVED.txt",
                    "storage-index.json",
                    ".molt-controller.json",
                }
                and not child.name.endswith(".lock")
            ):
                newest = max(newest, info.st_atime)
            if child.name.endswith(".lock") or child.name in {
                "lock",
                ".cargo-lock",
                ".cargo-build-lock",
            }:
                held |= _region_locked(child)
        elif not (stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode)):
            held = True
    return newest, held


def _moved_payload(task: Path, artifact: Path) -> tuple[Path | None, str]:
    """Accept the integrator's finite human MOVED.txt protocol, fail closed."""
    index = task / "MOVED.txt"
    try:
        lines = index.read_text(encoding="utf-8").splitlines()
        expected = artifact / "evidence" / "lanes" / "molt-src" / task.name
        targets = [Path(line.strip()) for line in lines if line.startswith("/")]
        if (
            len(targets) != 1
            or targets[0] != expected
            or resolve_owned_path(expected) != expected.absolute()
            or not expected.is_dir()
        ):
            return None, "unknown MOVED.txt destination"
        entries = [line.strip() for line in lines if line.startswith("  ")]
        if not entries or any(
            Path(entry).name != entry
            or entry in {".", ".."}
            or not (expected / entry).exists()
            for entry in entries
        ):
            return None, "incomplete MOVED.txt payload"
        return expected, "integrator payload retained at MOVED.txt destination"
    except (OSError, ValueError):
        return None, "unreadable MOVED.txt index"


def _retire_replaced_sandbox(
    repo: Path, source: Path, destination: Path, *, apply: bool
) -> tuple[str, Path | None]:
    """Archive a superseded queue checkout before releasing its Git registration."""
    from tools.molt_dev_common import _COMMANDS
    from molt.exact_json import write_exact

    actual, expected = git_directories(source), git_directories(repo)
    if (
        actual is None
        or expected is None
        or actual[1] != expected[1]
        or not (source / ".git").is_file()
    ):
        return "unregistered-source", None
    if (actual[0] / "locked").exists():
        return "worktree-locked", None
    branch = _COMMANDS.run(
        ["git", "-C", str(source), "symbolic-ref", "--quiet", "HEAD"],
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if branch.returncode != 1:
        return "source-holds-branch-or-unknown", None
    status = _COMMANDS.run(
        [
            "git",
            "--no-optional-locks",
            "-C",
            str(source),
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if status.returncode or status.stdout.strip():
        return "dirty-source", None
    if not apply:
        return "ready-archive-and-retire-worktree", None
    before = _tree_identity(source)
    archive = compact_evidence(
        source, destination, kind="integration-receipts", remove_source=False
    )
    if _tree_identity(source) != before:
        raise DriverError(
            f"sandbox changed after archival; preserved source and archive: {source}"
        )
    # The index survives Git removal and is published before it; a failed Git
    # removal leaves both source and archive for recovery.
    write_exact(
        expected[1] / "molt" / "retired-evidence" / (archive.stem + ".json"),
        {
            "schema": "molt.replaced-sandbox.v1",
            "source": str(source),
            "archive_index": str(archive.with_suffix(".index.json")),
        },
    )
    removed = _COMMANDS.run(
        ["git", "-C", str(repo), "worktree", "remove", "--force", str(source)],
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if removed.returncode:
        raise DriverError(f"archived sandbox removal deferred: {removed.stderr}")
    return "archived-and-retired-worktree", archive


def _safe_source(path: Path, *, allowed: tuple[Path, ...]) -> Path:
    selected = resolve_owned_path(path)
    if ".codex" in selected.parts or not any(
        selected == root or root in selected.parents for root in allowed
    ):
        raise ValueError(f"outside Molt migration custody: {selected}")
    return selected


def migration_plan(
    repo: Path,
    environment: Mapping[str, str],
    *,
    legacy_temp: Path | None = None,
    repository_only: bool = False,
) -> list[Migration]:
    """Finite old producers only; never enumerate sibling projects or VM disks."""
    from tools import controller_source
    from tools.coordination_records import (
        load_records,
        active_task_references,
    )
    from tools.disk_guard import registered_worktree_roots

    directories = git_directories(repo)
    if directories is None:
        raise DriverError("storage migration requires a Git checkout")
    common = directories[1]
    primary = common.parent
    artifact = artifact_root(repo, environment)
    allowed = (primary, primary.parent / "worktrees")
    rows = []
    now = time.time()
    runner = common / "molt" / "landing-queue" / "runner.lock"
    integration = common / "molt-integrate.lock"

    def add(
        kind: StorageClass,
        source: Path,
        target: Path,
        action: MigrationAction,
        reason: str,
        locks: tuple[Path, ...] = (),
    ) -> None:
        if source.is_symlink() or source.is_junction():
            rows.append(
                Migration(
                    kind,
                    source,
                    target,
                    "keep",
                    allocated_bytes(source),
                    "indirect preimage; target custody preserved",
                    locks,
                )
            )
            return
        if source.exists() and source != target:
            if kind != "replaced-compile-cache":
                _safe_source(source, allowed=allowed)
            last_use, held = _legacy_activity(
                source, read_access=action != "retire-worktree"
            )
            policy = storage_rule(kind).retention
            if (
                action != "keep"
                and isinstance(policy, DurableEvidence)
                and policy.compact_after_s is not None
            ):
                if held:
                    action, reason = "keep", "native lock, guard or special file in use"
                elif now - last_use < max(PROTECTION_WINDOW_S, policy.compact_after_s):
                    action, reason = "keep", "within last-use retention window"
            rows.append(
                Migration(
                    kind,
                    source,
                    target,
                    action,
                    allocated_bytes(source),
                    reason,
                    locks,
                    last_use,
                )
            )

    sandbox_kinds: tuple[StorageClass, ...] = (
        "replaced-git-landing-sandbox",
        "replaced-family-landing-sandbox",
    )
    for kind in sandbox_kinds:
        sandbox = storage_path(primary, kind, environment, common_dir=common)
        add(
            kind,
            sandbox,
            storage_path(primary, "integration-receipts", environment)
            / "replaced-sandbox",
            "retire-worktree",
            "archive stale detached checkout and release Git registration under runner/integration locks",
            (runner, integration),
        )
    controllers = storage_path(
        primary, "replaced-controllers", environment, common_dir=common
    )
    if controllers.is_dir() and not controllers.is_symlink():
        for source in sorted(controllers.iterdir()):
            marker = source / controller_source.RECORD_NAME
            try:
                record = json.loads(marker.read_text(encoding="utf-8"))
                from tools.git_identity import require_git_object_id

                require_git_object_id(record["commit"], label="legacy controller")
                admitted = (
                    set(record) == {"schema", "key", "commit"}
                    and record["schema"] == "molt.integration-controller.v1"
                    and record["key"] == source.name
                    and len(source.name) == 32
                    and all(char in "0123456789abcdef" for char in source.name)
                )
            except (OSError, ValueError, KeyError, TypeError):
                admitted = False
            if indexed_payload(source):
                continue
            add(
                "replaced-controllers",
                source,
                storage_path(primary, "diagnostics", environment)
                / "legacy-controllers",
                "compact" if admitted else "keep",
                "regenerate immutable source/environment at selected artifact placement"
                if admitted
                else "unrecognized controller preimage",
                (controller_source.source_lock_path(source),),
            )
    add(
        "replaced-integration-receipts",
        storage_path(
            primary, "replaced-integration-receipts", environment, common_dir=common
        ),
        storage_path(primary, "integration-receipts", environment),
        "compact",
        "durable receipt archive",
        (integration,),
    )
    checkouts = registered_worktree_roots(primary) or (primary,)
    records = [record for checkout in checkouts for record in load_records(checkout)]
    by_directory = {record.path.parent: record for record in records}
    references = active_task_references(records)
    evidence_policy = storage_rule("lane-evidence").retention
    assert isinstance(evidence_policy, DurableEvidence)
    for checkout in checkouts:
        registry = storage_path(checkout, "replaced-lane-evidence", environment)
        if not registry.is_dir() or registry.is_symlink():
            continue
        for task in sorted(registry.iterdir()):
            if not task.is_dir() or task.is_symlink():
                continue
            if (task / "MOVED.txt").exists():
                moved, reason = _moved_payload(task, artifact)
                rows.append(
                    Migration(
                        "replaced-lane-evidence",
                        task,
                        moved or task,
                        "already-moved" if moved is not None else "keep",
                        allocated_bytes(moved or task),
                        reason,
                    )
                )
                continue
            if indexed_payload(task):
                continue
            record = by_directory.get(task)
            size = allocated_bytes(task)
            active = (
                record is None
                or record.active
                or record.status == "invalid"
                or record.task in references
            )
            if size > evidence_policy.checkout_bytes:
                add(
                    "replaced-lane-evidence",
                    task,
                    storage_path(primary, "lane-evidence", environment) / "archive",
                    "keep" if active else "compact",
                    "unknown coordination record"
                    if record is None
                    else "invalid coordination record"
                    if record.status == "invalid"
                    else "active coordination"
                    if record.active
                    else "referenced by active coordination"
                    if record.task in references
                    else "terminal evidence above checkout budget",
                    (common / "molt" / "lane-start" / "slots.lock",),
                )
    # Old cache may contain diagnostic material: archive, never infer that all
    # unregistered preimage files are safe to delete.
    old_cache = storage_path(primary, "replaced-compile-cache", environment)
    if not repository_only and old_cache.exists():
        # Preserve even unknown legacy signal losslessly. Locks and last use
        # still fence removal; external provider homes never enter this plan.
        add(
            "replaced-compile-cache",
            old_cache,
            artifact / "evidence" / "legacy-cache",
            "compact",
            "lossless legacy Molt cache retention",
        )
    if not repository_only:
        tools = storage_path(primary, "toolchains", environment)
        rows.append(
            Migration(
                "toolchains",
                tools,
                tools,
                "keep",
                allocated_bytes(tools),
                "execution placement retained by measured root/data comparison; version retention is separate",
            )
        )
    if legacy_temp is not None:
        rows.extend(_legacy_temp_plan(primary, environment, legacy_temp))
    return rows


def _legacy_temp_plan(
    primary: Path, environment: Mapping[str, str], legacy_temp: Path
) -> list[Migration]:
    from molt.checkout_state import is_evidence_index

    rows = []
    legacy_temp = resolve_owned_path(legacy_temp)
    if ".codex" in legacy_temp.parts:
        raise DriverError("legacy temp inspection cannot enter Codex custody")
    # Names select Molt signal, never a disposable class. The same verified
    # archive and index boundary preserves unknown legacy bytes before removal.
    for path in sorted(legacy_temp.iterdir()):
        if not path.name.startswith(("molt", "linker-crash-")):
            continue
        if is_evidence_index(path) or indexed_payload(path):
            continue
        if path.is_symlink() or not path.name.startswith(
            ("molt-", "molt_", "moltd-", "linker-crash-")
        ):
            rows.append(
                Migration(
                    "replaced-temporary",
                    path,
                    path,
                    "keep",
                    allocated_bytes(path),
                    "unknown temp producer or link; custody reconciliation required",
                )
            )
            continue
        last_use, held = _legacy_activity(path)
        policy = storage_rule("replaced-temporary").retention
        assert (
            isinstance(policy, DurableEvidence) and policy.compact_after_s is not None
        )
        eligible = not held and time.time() - last_use >= max(
            PROTECTION_WINDOW_S, policy.compact_after_s
        )
        rows.append(
            Migration(
                "replaced-temporary",
                path,
                storage_path(primary, "scratch-evidence", environment) / "legacy",
                "compact" if eligible else "keep",
                allocated_bytes(path),
                "idle legacy temp; lossless archival before raw retirement"
                if eligible
                else "native lock, guard or special entry"
                if held
                else "recent last use",
                (),
                last_use,
            )
        )
    return rows


def migrate(
    repo: Path,
    environment: Mapping[str, str],
    *,
    apply: bool,
    legacy_temp: Path | None = None,
    repository_only: bool = False,
) -> list[dict[str, object]]:
    from tools.coordination_records import load_records, active_task_references

    from tools.disk_guard import _guard_protection

    guards = _guard_protection((repo, artifact_root(repo, environment)))
    result = []
    plan = migration_plan(
        repo, environment, legacy_temp=legacy_temp, repository_only=repository_only
    )
    for item in plan:
        row = asdict(item)
        row.update(
            source=str(item.source),
            destination=str(item.destination),
            locks=[str(path) for path in item.locks],
        )
        row["status"] = "planned"
        with ExitStack() as stack:
            handles = []
            for path in item.locks:
                try:
                    handle = _try_acquire_file_lock(path, create=apply)
                except FileNotFoundError:
                    handle = None
                handles.append(handle)
            for handle in handles:
                if handle is not None:
                    stack.callback(_release_file_lock, handle)
            if any(
                handle is None and path.exists()
                for handle, path in zip(handles, item.locks)
            ):
                row["status"] = "live-lock"
            elif item.action == "already-moved":
                row["status"] = "already-moved"
            elif item.action == "keep":
                row["status"] = "kept"
            elif guards.protects(
                item.source, artifact_root(repo, environment), recorded_owner=repo
            ):
                row["status"] = "guard-in-use"
            elif not apply and item.action == "retire-worktree":
                row["status"], _ = _retire_replaced_sandbox(
                    repo, item.source, item.destination, apply=False
                )
            elif not apply and item.action == "move-worktree":
                row["status"] = _move_sandbox(
                    repo, item.source, item.destination, apply=False
                )
            elif apply:
                with checkout_state_retirement(item.source) as exclusive:
                    if not exclusive:
                        row["status"] = "state-in-use"
                    elif item.last_use is not None and _legacy_activity(
                        item.source, read_access=item.action != "retire-worktree"
                    ) != (item.last_use, False):
                        row["status"] = "use-changed-or-native-writer"
                    elif item.action == "retire-worktree":
                        row["status"], archive = _retire_replaced_sandbox(
                            repo, item.source, item.destination, apply=True
                        )
                        if archive is not None:
                            row["archive"] = str(archive)
                    elif item.action == "compact":
                        keep = []
                        if item.kind == "replaced-lane-evidence":
                            from tools.disk_guard import registered_worktree_roots
                            from tools.coordination_records import LOG_ROOT

                            checkout = item.source.parents[2]
                            checkouts = registered_worktree_roots(checkout) or (
                                checkout,
                            )
                            admitted = [
                                stack.enter_context(
                                    checkout_state_retirement(owner / LOG_ROOT)
                                )
                                for owner in checkouts
                            ]
                            if not all(admitted):
                                row["status"] = "registry-in-use"
                                result.append(row)
                                continue
                            records = [
                                record
                                for owner in checkouts
                                for record in load_records(owner)
                            ]
                            record = next(
                                (
                                    record
                                    for record in records
                                    if record.path.parent == item.source
                                ),
                                None,
                            )
                            if (
                                record is None
                                or record.active
                                or record.task in active_task_references(records)
                            ):
                                row["status"] = "active-or-referenced"
                                result.append(row)
                                continue
                            keep = [
                                path.relative_to(item.source)
                                for path in item.source.rglob("*")
                                if not path.is_symlink()
                                and path.is_file()
                                and path.stat().st_size <= 1024 * 1024
                                and (
                                    path.suffix == ".md"
                                    or path.name
                                    in {
                                        "coordination.json",
                                        "env.sh",
                                        "env.ps1",
                                        "progress.log",
                                    }
                                )
                            ]
                        row["archive"] = str(
                            compact_evidence(
                                item.source,
                                item.destination,
                                kind="lane-evidence"
                                if item.kind == "replaced-lane-evidence"
                                else "integration-receipts"
                                if item.kind == "replaced-integration-receipts"
                                else "diagnostics",
                                keep=keep,
                            )
                        )
                        row["status"] = "compacted"
                        if item.kind == "replaced-temporary":
                            directories = git_directories(repo)
                            assert directories is not None
                            row["index"] = str(
                                _retain_source_index(
                                    item.source,
                                    directories[1] / "molt" / "retired-evidence",
                                )
                            )
                    elif item.action == "move-worktree":
                        from tools.disk_guard import _guard_protection

                        guards = _guard_protection(
                            (repo, artifact_root(repo, environment))
                        )
                        if guards.protects(
                            item.source,
                            artifact_root(repo, environment),
                            recorded_owner=repo,
                        ):
                            row["status"] = "guard-in-use"
                        else:
                            row["status"] = _move_sandbox(
                                repo, item.source, item.destination
                            )
            result.append(row)
    return result


def cmd_storage(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    if args.storage_command == "inventory":
        result = inventory(repo, os.environ)
    elif args.storage_command == "trim":
        result = trim(
            repo,
            os.environ,
            apply=args.apply,
            legacy_temp=args.legacy_temp_root,
            include_shared_preimages=True,
        )
    else:
        result = migrate(
            repo, os.environ, apply=args.apply, legacy_temp=args.legacy_temp_root
        )
    print(
        json.dumps(
            {
                "schema": "molt.storage.v1",
                "mode": "apply" if getattr(args, "apply", False) else "dry-run",
                "entries": result,
            },
            indent=2,
        )
    )
    return 0


def add_arguments(subparsers: argparse._SubParsersAction) -> None:
    parser = subparsers.add_parser(
        "storage", help="typed storage inventory and explicit migration"
    )
    commands = parser.add_subparsers(dest="storage_command", required=True)
    for name in ("inventory", "migrate", "trim"):
        command = commands.add_parser(name)
        command.add_argument("--repo", default=".")
        if name != "inventory":
            command.add_argument("--legacy-temp-root", type=Path)
            modes = command.add_mutually_exclusive_group()
            modes.add_argument(
                "--apply",
                action="store_true",
                help="explicitly execute the reviewed migration",
            )
            modes.add_argument(
                "--dry-run", action="store_true", help="read-only plan (default)"
            )
        command.set_defaults(func=cmd_storage)
