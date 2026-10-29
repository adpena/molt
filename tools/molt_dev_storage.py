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
    Regenerable,
    StorageClass,
    git_directories,
    custody_root,
    repository_storage_key,
    storage_classes,
    storage_rule,
    PackageEnvironmentKind,
)
from molt.dx import artifact_root, storage_path
from molt.file_locks import _try_acquire_file_lock, _release_file_lock
from molt.file_publication import resolve_owned_path
from tools.driver_errors import DriverError


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


def inventory(repo: Path, environment: Mapping[str, str]) -> list[dict[str, object]]:
    rows = []
    for kind in storage_classes():
        rule = storage_rule(kind)
        path = storage_path(repo, kind, environment)
        rows.append(
            {
                "class": kind,
                "path": str(path),
                "volume": rule.volume,
                "owner": rule.owner,
                "retention": asdict(rule.retention),
                "durable": isinstance(rule.retention, DurableEvidence),
            }
        )
    return rows


def trim(
    repo: Path,
    environment: Mapping[str, str],
    *,
    apply: bool,
    include_compile_cache: bool = True,
    policy_source: Path | None = None,
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
    from tools import molt_dev_controller
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
                molt_dev_controller._retire_idle(
                    directory, current=current, now=time.time(), apply=apply
                )
            )
        else:
            result.extend(
                molt_dev_controller._retire_idle(
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


MigrationAction = Literal["compact", "move-worktree", "keep"]


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


def _safe_source(path: Path, *, allowed: tuple[Path, ...]) -> Path:
    selected = resolve_owned_path(path)
    if ".codex" in selected.parts or not any(
        selected == root or root in selected.parents for root in allowed
    ):
        raise ValueError(f"outside Molt migration custody: {selected}")
    return selected


def migration_plan(
    repo: Path, environment: Mapping[str, str], *, legacy_temp: Path | None = None
) -> list[Migration]:
    """Finite old producers only; never enumerate sibling projects or VM disks."""
    from tools import controller_source
    from tools.coordination_records import (
        load_records,
        active_task_references,
        LOG_ROOT,
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
        if source.exists() and source != target:
            _safe_source(source, allowed=allowed)
            rows.append(
                Migration(
                    kind, source, target, action, allocated_bytes(source), reason, locks
                )
            )

    old_custody = custody_root(primary)
    old_external = old_custody.parent if old_custody == primary else old_custody
    for sandbox in (
        common / "molt" / "landing-sandbox",
        old_external
        / "worktrees"
        / f"landing-sandbox-{repository_storage_key(common)}",
    ):
        target = storage_path(
            primary, "landing-sandbox", environment, common_dir=common
        )
        if sandbox == common / "molt" / "landing-sandbox":
            target = artifact / "worktrees" / (target.name + "-legacy")
        add(
            "landing-sandbox",
            sandbox,
            target,
            "move-worktree",
            "registered clean source only; runner/integration locks required",
            (runner, integration),
        )
    controllers = common / "molt" / "controllers"
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
            add(
                "controllers",
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
        "integration-receipts",
        common / "molt-integrate",
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
        registry = checkout / LOG_ROOT
        if not registry.is_dir() or registry.is_symlink():
            continue
        for task in sorted(registry.iterdir()):
            if not task.is_dir() or task.is_symlink():
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
                    "lane-evidence",
                    task,
                    storage_path(primary, "lane-evidence", environment) / "archive",
                    "keep" if active else "compact",
                    "active/referenced/unknown coordination"
                    if active
                    else "terminal evidence above checkout budget",
                    (common / "molt" / "lane-start" / "slots.lock",),
                )
    # Old cache may contain diagnostic material: archive, never infer that all
    # unregistered preimage files are safe to delete.
    from molt.default_paths import expand_user_path

    old_cache = expand_user_path("~/.cache/molt", environment=environment)
    if old_cache.exists():
        rows.append(
            Migration(
                "compile-cache",
                old_cache,
                artifact / "evidence" / "legacy-cache",
                "keep",
                allocated_bytes(old_cache),
                "legacy cache needs its producer locks; new writes use canonical MOLT_CACHE",
            )
        )
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
        legacy_temp = resolve_owned_path(legacy_temp)
        if ".codex" in legacy_temp.parts:
            raise DriverError("legacy temp inspection cannot enter Codex custody")
        # These names alone prove neither ownership nor liveness. Preserve the
        # payload and report the decision; canonical new temp uses run scratch.
        for path in sorted(legacy_temp.iterdir()):
            if (
                path.name.startswith(("molt-", "linker-crash-"))
                and not path.is_symlink()
            ):
                rows.append(
                    Migration(
                        "temporary",
                        path,
                        storage_path(primary, "temporary", environment) / "legacy",
                        "keep",
                        allocated_bytes(path),
                        "legacy temp has no lock custody; integrator must reconcile active writers",
                    )
                )
    return rows


def migrate(
    repo: Path,
    environment: Mapping[str, str],
    *,
    apply: bool,
    legacy_temp: Path | None = None,
) -> list[dict[str, object]]:
    from tools.coordination_records import load_records, active_task_references

    result = []
    for item in migration_plan(repo, environment, legacy_temp=legacy_temp):
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
            elif item.action == "keep":
                row["status"] = "kept"
            elif not apply and item.action == "move-worktree":
                row["status"] = _move_sandbox(
                    repo, item.source, item.destination, apply=False
                )
            elif apply:
                with checkout_state_retirement(item.source) as exclusive:
                    if not exclusive:
                        row["status"] = "state-in-use"
                    elif item.action == "compact":
                        keep = []
                        if item.kind == "lane-evidence":
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
                                if item.kind == "lane-evidence"
                                else "integration-receipts"
                                if item.kind == "integration-receipts"
                                else "diagnostics",
                                keep=keep,
                            )
                        )
                        row["status"] = "compacted"
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
        result = trim(repo, os.environ, apply=args.apply)
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
