"""Shared user-state paths, including verified bundle bootstrap.

packaging/bootstrap.py admits this module with its complete source closure,
`RELEASE_SOURCE_MODULES`, before any `molt` package is importable. A module
import outside the standard library joins that closure in the same change.
"""

from __future__ import annotations

import functools
import hashlib
import ntpath
import os
import re
import sys
from collections.abc import Mapping
from pathlib import Path
from dataclasses import dataclass, replace
import json
from typing import Literal

from .process_environment import ProcessEnvironment
from .custody_layout import (
    Regenerable,
    StorageLocation,
    enclosing_checkouts,
    storage_rule,
)


def executable_environment_value(
    environment: Mapping[str, str],
    key: str,
    default: str = "",
) -> str:
    """Read executable-selection inputs under the caller's captured name rule."""
    selected = (
        environment
        if isinstance(environment, ProcessEnvironment)
        else ProcessEnvironment(environment)
    )
    return selected.get(key, default)


def expand_user_path(
    path: str | Path, *, environment: Mapping[str, str] | None = None
) -> Path:
    """Expand a user selector using the selected environment, never ambient keys.

    With no explicit mapping, retain pathlib's host behavior. POSIX named users
    (and a missing HOME) use the system account database, as pathlib does;
    Windows uses USERPROFILE or HOMEDRIVE/HOMEPATH and captured USERNAME.
    """
    raw = os.fspath(path)
    if not raw.startswith("~"):
        return Path(raw)
    if environment is None:
        return Path(raw).expanduser()
    environment = ProcessEnvironment(environment)
    separators = "/\\" if os.name == "nt" else "/"
    end = next(
        (index for index, char in enumerate(raw) if char in separators), len(raw)
    )
    user, suffix = raw[1:end], raw[end:]
    if os.name == "nt":
        home = executable_environment_value(environment, "USERPROFILE")
        if not home:
            homepath = executable_environment_value(environment, "HOMEPATH")
            if not homepath:
                raise ValueError("selected environment has no user home for tilde path")
            home = ntpath.join(
                executable_environment_value(environment, "HOMEDRIVE"), homepath
            )
        if user:
            current_user = executable_environment_value(environment, "USERNAME")
            if user != current_user:
                if ntpath.basename(home.rstrip("\\/")) != current_user:
                    raise ValueError(
                        "selected environment cannot resolve named user home"
                    )
                home = ntpath.join(ntpath.dirname(home.rstrip("\\/")), user)
    else:
        home = environment.get("HOME") if not user else None
        if home is None:
            import pwd

            try:
                home = (
                    pwd.getpwnam(user) if user else pwd.getpwuid(os.getuid())
                ).pw_dir
            except KeyError as exc:
                raise ValueError(
                    "cannot resolve system account for tilde path"
                ) from exc
    if "\0" in home:
        raise ValueError("selected user home contains a NUL byte")
    expanded = home + suffix if os.name == "nt" else home.rstrip(separators) + suffix
    if expanded.startswith("~"):
        raise ValueError("selected user home did not resolve tilde path")
    return Path(expanded or os.sep)


# The directory inside a WASI SDK install prefix that holds the SDK itself.
SDK_DIRNAME = "sdk"


def explicit_wasi_sdk_prefix(
    environment: Mapping[str, str], *, cwd: Path | None = None
) -> Path | None:
    """The install prefix `WASI_SDK_PATH` or `WASI_SDK_PREFIX` selects, or None.

    Either variable may name the prefix or its `sdk` directory. The one
    reading of the explicit selection: `llvm_toolchain` selects it and
    `checkout_state` declares its use control files.
    """
    sdk = environment.get("WASI_SDK_PATH") or environment.get("WASI_SDK_PREFIX")
    if not sdk:
        return None
    prefix = expand_user_path(sdk, environment=environment)
    if not prefix.is_absolute():
        prefix = (Path.cwd() if cwd is None else cwd) / prefix
    prefix = prefix.absolute()
    return prefix.parent if prefix.name == SDK_DIRNAME else prefix


def _default_home_str(environ: Mapping[str, str] | None = None) -> str | None:
    try:
        return os.fspath(
            Path.home()
            if environ is None
            else expand_user_path("~", environment=environ)
        )
    except (RuntimeError, ValueError):
        return None


def _path_override(env: Mapping[str, str], key: str) -> str | None:
    raw = executable_environment_value(env, key)
    return os.fspath(expand_user_path(raw, environment=env)) if raw else None


ARTIFACT_ROOT_ENV = "MOLT_EXT_ROOT"


def configured_artifact_root_text(env: Mapping[str, str] | None = None) -> str | None:
    """Return the operator's ``MOLT_EXT_ROOT`` text exactly, or None when unset.

    For cache keys on hot paths: it touches no filesystem. The cached consumer
    anchors and resolves the value once per distinct key.
    """
    view = os.environ if env is None else env
    raw = executable_environment_value(view, ARTIFACT_ROOT_ENV).strip()
    return raw or None


@functools.lru_cache(maxsize=128)
def _default_user_state_cached(
    kind: Literal["compile-cache", "installation"],
    values: tuple[tuple[str, str], ...],
    cwd_str: str,
    platform_name: str,
    home: str | None,
) -> StorageLocation:
    # Keep precedence and family selection inside the memoized authority.
    # Expand lower-priority selectors only when selected.
    env = dict(values)
    operation_cwd = Path(cwd_str)
    rule = storage_rule(kind)
    if kind == "installation" and (selected := _path_override(env, "MOLT_HOME")):
        path = Path(selected)
        return StorageLocation(
            kind,
            path if path.is_absolute() else (operation_cwd / path).absolute(),
            rule,
        )
    selected = _path_override(env, "MOLT_CACHE")
    if selected is None:
        external_text = configured_artifact_root_text(env)
        external = (
            os.fspath(expand_user_path(external_text, environment=env))
            if external_text
            else None
        )
        if external is None and kind == "compile-cache":
            # The family binding is DX's authority, including its identity and
            # missing-volume checks. DX is outside the bootstrap closure; import
            # it only when selection is actually needed at runtime.
            checkouts = enclosing_checkouts(operation_cwd)
            if checkouts:
                from .dx import configured_artifact_root

                bound = configured_artifact_root(env, relative_to=checkouts[0])
                external = os.fspath(bound) if bound is not None else None
        external_path = Path(external) if external else operation_cwd
        if not external_path.is_absolute():
            external_path = operation_cwd / external_path
        if external:
            selected = os.fspath(
                external_path.joinpath(*storage_rule("compile-cache").relative)
            )
        else:
            rule = replace(rule, volume="checkout")
            windows = platform_name == "win32"
            base = _path_override(env, "LOCALAPPDATA" if windows else "XDG_CACHE_HOME")
            if base is not None:
                selected = os.fspath(Path(base) / ("Molt" if windows else "molt"))
            elif home is None:
                selected = os.fspath(
                    external_path.joinpath(*storage_rule("compile-cache").relative)
                )
            elif windows:
                selected = os.fspath(Path(home) / "AppData" / "Local" / "Molt")
            else:
                selected = os.fspath(Path(home) / ".cache" / "molt")
    path = Path(selected)
    path = path if path.is_absolute() else (operation_cwd / path).absolute()
    if kind == "installation":
        path = cache_home_root(path)
    return StorageLocation(kind, path, rule)


def user_state_location(
    kind: Literal["compile-cache", "installation"],
    *,
    environ: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> StorageLocation:
    env = ProcessEnvironment(os.environ if environ is None else environ)
    values = tuple(
        (key, executable_environment_value(env, key))
        for key in (
            "MOLT_CACHE",
            "MOLT_HOME",
            "MOLT_EXT_ROOT",
            "XDG_CACHE_HOME",
            "LOCALAPPDATA",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "USERNAME",
        )
        if key in env
    )
    selected = _default_user_state_cached(
        kind,
        values,
        os.fspath(Path.cwd() if cwd is None else cwd),
        sys.platform,
        _default_home_str(environ),
    )
    # DX is outside the bootstrap closure. An active DX write check already
    # loaded its observer; expose the typed selection without importing DX
    # merely for optional development instrumentation.
    observer = getattr(sys.modules.get("molt.dx"), "_storage_projection", None)
    if observer is not None:
        observer(kind, selected.path)
    return selected


def _default_molt_cache(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    return user_state_location("compile-cache", environ=environ, cwd=cwd).path


_CACHE_RETENTION = storage_rule("compile-cache").retention
assert isinstance(_CACHE_RETENTION, Regenerable)
assert _CACHE_RETENTION.max_bytes is not None
DEFAULT_MOLT_CACHE_MAX_GB = str(int(_CACHE_RETENTION.max_bytes / 1024**3))
DEFAULT_MOLT_CACHE_MAX_AGE_DAYS = str(int(_CACHE_RETENTION.max_idle_s / 86400))


def resolve_budget(
    max_gb: float | None,
    max_age_days: int | None,
    environ: Mapping[str, str],
) -> tuple[float, int]:
    """Return the cache budget: flags, then the projected env, then canonical user-state defaults.

    RunContext projects MOLT_CACHE_MAX_GB and MOLT_CACHE_MAX_AGE_DAYS from these
    defaults, so the pruner, the digest and toolchain-fact caches and every DX
    entry point share one budget.
    """
    gb = (
        max_gb
        if max_gb is not None
        else float(environ.get("MOLT_CACHE_MAX_GB") or DEFAULT_MOLT_CACHE_MAX_GB)
    )
    days = (
        max_age_days
        if max_age_days is not None
        else int(
            environ.get("MOLT_CACHE_MAX_AGE_DAYS") or DEFAULT_MOLT_CACHE_MAX_AGE_DAYS
        )
    )
    return gb, days


CACHE_MAINTENANCE_INTERVAL_SECONDS = 300.0
# A CLI build applies the cache budget at most once a day, as Go trims its build
# cache and Cargo runs automatic gc: a sweep costs a full inventory, and an
# interactive build loop must not pay it every few minutes.
CLI_CACHE_RETENTION_INTERVAL_SECONDS = 24 * 3600.0
SHARED_SOURCE_RECORDS_DIRECTORY = "toolchain_source_records"
# Immutable content-addressed import-scan and module-analysis records
# (molt.cli.frontend_record_files).
FRONTEND_RECORD_FILES_DIRECTORY = "frontend_records"
TOOLCHAIN_SYMBOL_FACTS_CACHE_DIRECTORY = "toolchain_symbol_facts"
TOOLCHAIN_SYMBOL_FACTS_CACHE_SCHEMA_VERSION = 6
RUNTIME_WASM_CACHE_DIRECTORY = "runtime-wasm-generations-v2"
RUNTIME_WASM_CACHE_RETIREMENT_SCOPE = "runtime-wasm-family"
WASM_LINK_CACHE_DIRECTORY = "wasm_link"
WASM_LINK_CACHE_FAMILIES = frozenset(
    {"runtime_tree_shake", "split_app_optimize", "final_link"}
)
NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY = "native-stdlib-objects-v1"
NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY = "native-stdlib-archives-v1"
SHARED_STDLIB_CACHE_DIRECTORY = "shared-stdlib"
BUILD_ARTIFACTS_CACHE_DIRECTORY = "build"


def toolchain_symbol_facts_cache_entry(
    cache_root: Path, version: int, digest: str
) -> Path:
    """One immutable fact record; older known schemas are cleanup-only."""
    if (
        type(version) is not int
        or not 1 <= version <= TOOLCHAIN_SYMBOL_FACTS_CACHE_SCHEMA_VERSION
    ):
        raise ValueError("symbol facts cache schema must be a known positive version")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ValueError("symbol facts cache key must be a lowercase SHA-256")
    return (
        cache_root
        / TOOLCHAIN_SYMBOL_FACTS_CACHE_DIRECTORY
        / f"v{version}"
        / f"{digest}.json"
    )


def backend_cache_artifact_filename(source_key: str, suffix: str) -> str:
    """Filename projection shared by compiler publication and retention."""
    return (
        source_key.replace("|artifact:", ".artifact-").replace("|stdlib:", ".stdlib-")
        + suffix
    )


def is_backend_cache_artifact_name(name: str) -> bool:
    # Canonical cache keys and contract identities are SHA-256. Reserved private
    # publication stages and unknown/control namespaces cannot match this form.
    return (
        re.fullmatch(
            r"[0-9a-f]{64}\.artifact-[0-9a-f]{64}(?:\.stdlib-[0-9a-f]{64})?"
            r"\.(?:[a-z][a-z0-9]*|symbols\.json)",
            name,
        )
        is not None
    )


def backend_cache_sync_receipt_path(artifact: Path) -> Path:
    """Origin/byte generation receipt owned by the immutable artifact publisher."""
    return artifact.with_name(artifact.name + ".sync.v5.json")


def backend_cache_artifact_for_sync_receipt(receipt: Path) -> Path | None:
    """Decode only the current publisher's receipt namespace."""
    name, separator, _ = receipt.name.rpartition(".sync.")
    if not separator or not is_backend_cache_artifact_name(name):
        return None
    artifact = receipt.with_name(name)
    return artifact if backend_cache_sync_receipt_path(artifact) == receipt else None


# A keyed lock family names one of this many stripes (4,096), so its lock
# files stay bounded: per-key names left about 157k files in the compile
# cache's lock directory and 65k checkout-state locks (HF-942). A stripe only
# ever serializes two holders; no path takes two keys of one family at once.
LOCK_STRIPE_HEX_DIGITS = 3


# Per-key lock names the stripes replaced; the storage tooling and the daily
# cache retention unlink their unheld files (file_locks.retire_unheld_lock_files).
RETIRED_CACHE_LOCK_NAMES = re.compile(
    r"(?:immutable-publish-[0-9a-f]{16}|compile\.[0-9a-f]{64})\.lock"
)
RETIRED_CHECKOUT_STATE_LOCK_NAMES = re.compile(r"[0-9a-f]{64}\.lock")
# A retired lock file this recently changed may still be another process's.
RETIRED_LOCK_MIN_IDLE_S = 86400.0


def lock_stripe(key: str) -> str:
    """The stripe of ``key`` within its lock family."""
    return hashlib.sha256(key.encode("utf-8")).hexdigest()[:LOCK_STRIPE_HEX_DIGITS]


def backend_cache_publication_lock_name(artifact: Path) -> str:
    """Publication and eviction serialize on the artifact's stripe."""
    return f"immutable-publish.{lock_stripe(str(artifact.resolve()))}"


HOT_STATE_RECORD = "hot-state.v1.json"
# The home volume keeps at least this much free whatever the hot root holds
# (HF-942: the internal SSD was 93-96% full while it served builds).
HOT_STATE_MIN_FREE_BYTES = 50 * 1024**3
HOT_STATE_SCHEMA = "molt.hot-state.v1"


@dataclass(frozen=True)
class HotState:
    """Where one artifact root keeps its small, constantly touched state."""

    root: Path
    budget_bytes: int


_HOT_STATES: dict[Path, HotState | None] = {}


def hot_state(artifact_root: Path) -> HotState | None:
    """The internal-disk root of ``artifact_root``'s hot state, or None (HF-942).

    One record in the artifact root (``hot-state.v1.json``, written by
    ``molt_dev.py storage migrate``) names it. Lock directories and the file
    digest store live there, under the same relative paths they have below
    the artifact root, so placement changes a prefix and never a name. A
    root without the record (a test cache, a plain clone, one volume) keeps
    them in place. Resolved once per process; a malformed record is an error.
    """
    key = artifact_root.absolute()
    if key in _HOT_STATES:
        return _HOT_STATES[key]
    record = key / HOT_STATE_RECORD
    try:
        text = record.read_text(encoding="utf-8")
    except FileNotFoundError:
        _HOT_STATES[key] = None
        return None
    data = json.loads(text)
    root = Path(data["root"]) if isinstance(data, dict) and "root" in data else None
    budget = data.get("budget_bytes") if isinstance(data, dict) else None
    if (
        data.get("schema") != HOT_STATE_SCHEMA
        or root is None
        or not root.is_absolute()
        or type(budget) is not int
        or budget <= 0
    ):
        raise ValueError(f"malformed hot-state record: {record}")
    state = _HOT_STATES[key] = HotState(root, budget)
    return state


def forget_hot_state(artifact_root: Path) -> None:
    """Drop this process's resolution, after the record was written."""
    _HOT_STATES.pop(artifact_root.absolute(), None)


def hot_path(artifact_root: Path, *relative: str) -> Path:
    """``artifact_root / relative``, below the hot-state root when it has one."""
    state = hot_state(artifact_root)
    return (artifact_root if state is None else state.root).joinpath(*relative)


FRONTEND_RECORD_FILES_LAYOUT = "v1"
FRONTEND_RECORD_INDEX_SUFFIX = ".idx"
FRONTEND_RECORD_PACK_INDEX_SUFFIX = ".idx2"


def frontend_record_files_root(cache_root: Path) -> Path:
    """The record files' root, below the hot-state root when bound (HF-942)."""
    return hot_path(
        cache_root.parent,
        cache_root.name,
        FRONTEND_RECORD_FILES_DIRECTORY,
        FRONTEND_RECORD_FILES_LAYOUT,
    )


def cache_control_root(cache_root: Path) -> Path:
    """Persistent lock ownership within one selected cache namespace."""
    return hot_path(cache_root.parent, cache_root.name, "locks")


def file_digest_cache_path(cache_root: Path) -> Path:
    """The host's stat-keyed file digest store within one cache namespace."""
    return hot_path(cache_root.parent, cache_root.name, "file-digests", "v3.sqlite")


def toolchain_fact_store_path(cache_root: Path) -> Path:
    """The host's toolchain probe answers (`molt.toolchain_facts`) within one cache namespace."""
    return hot_path(cache_root.parent, cache_root.name, "toolchain-facts", "v4.sqlite")


def shared_cache_lock_path(cache_root: Path, name: str) -> Path:
    """Project the shared producer/reader/evictor lock in durable control state."""
    return cache_control_root(cache_root) / f"{name}.lock"


def backend_shared_lock_path(cache_root: Path, name: str) -> Path:
    """A stdlib-family lock the Rust backend also takes, at its fixed path.

    The backend names ``<cache_root>/locks/<family>.<xx>.lock``
    (``shared_stdlib_cache/publish/paths.rs``); it does not read the hot-state
    record, so these stay below the cache root (HF-970: after HF-942 moved
    ``cache_control_root`` to the hot volume, Python and the backend took
    different files for one stdlib entry and no longer excluded each other).
    """
    return cache_root / "locks" / f"{name}.lock"


def runtime_wasm_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One retired runtime WASM cache family and its publication/use stripe.

    The runtime WASM pair now lives in the product store (kind
    ``wasm-runtime``); nothing publishes here. The pruner keeps retiring the
    families older builds left, by last use and under the stripe an older
    process still holds while it reads one.
    """
    if not isinstance(key, str) or re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("runtime WASM cache key must be a lowercase SHA-256")
    return (
        cache_root / RUNTIME_WASM_CACHE_DIRECTORY / key,
        shared_cache_lock_path(cache_root, f"runtime-wasm-family.{key[:2]}"),
    )


def wasm_link_cache_paths(
    cache_root: Path, family: str, schema: str, key: str
) -> tuple[Path, Path]:
    """Project one WASM entry and its stable bounded publication lock stripe."""
    if family not in WASM_LINK_CACHE_FAMILIES:
        raise ValueError(f"unknown wasm linker cache family: {family}")
    if (
        not isinstance(schema, str)
        or re.fullmatch(r"[a-z0-9][a-z0-9-]*", schema) is None
    ):
        raise ValueError("wasm linker cache schema must be a lowercase identifier")
    if not isinstance(key, str) or re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("wasm linker cache key must be a lowercase SHA-256")
    family_root = cache_root / family
    # These 256 files must survive every eviction: an open waiter may still
    # refer to a lock inode after its current owner releases it.
    return family_root / schema / key, family_root / ".locks" / f"{key[:2]}.lock"


def shared_stdlib_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One archive generation, including sidecars and private publication stages."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("shared stdlib entry key must be a lowercase SHA-256")
    return (
        cache_root / SHARED_STDLIB_CACHE_DIRECTORY / key,
        backend_shared_lock_path(cache_root, f"shared-stdlib.{key[:2]}"),
    )


def native_stdlib_archive_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One archive generation keyed by its ordered constituent inputs (content)."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("native stdlib archive key must be a lowercase SHA-256")
    return (
        cache_root / NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY / key,
        backend_shared_lock_path(cache_root, f"native-stdlib-archive.{key[:2]}"),
    )


def build_artifacts_entry(cache_root: Path, name: str) -> tuple[Path, Path]:
    """One build configuration's intermediates and the lock its build holds.

    ``name`` is ``<entry stem>-<16-hex artifacts identity>``
    (``build_output_layout.artifacts_identity``). The lock sits beside the
    entry, never inside it: eviction removes the tree with no open handle in
    it (Windows refuses that), and the lock's file outlives the tree, so a
    build waiting on it never holds a lock on a deleted file.
    """
    if re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9._-]*-[0-9a-f]{16}", name) is None:
        raise ValueError(f"not a build artifacts root name: {name!r}")
    base = cache_root / BUILD_ARTIFACTS_CACHE_DIRECTORY
    return base / name, base / f"{name}.lock"


def native_stdlib_object_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One independently reusable constituent object and its publication stages."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("native stdlib object key must be a lowercase SHA-256")
    return (
        cache_root / NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY / key,
        backend_shared_lock_path(cache_root, f"native-stdlib-object.{key[:2]}"),
    )


def _stdlib_archive_cache_entry(archive: Path) -> tuple[Path, Path] | None:
    if archive.name != "stdlib.a":
        return None
    for namespace, select in (
        (SHARED_STDLIB_CACHE_DIRECTORY, shared_stdlib_cache_entry),
        (NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY, native_stdlib_archive_cache_entry),
    ):
        if archive.parent.parent.name == namespace:
            return select(archive.parent.parent.parent, archive.parent.name)
    return None


def stdlib_archive_cache_root(archive: Path) -> Path | None:
    """The owner of a program-keyed or closed-content archive generation."""
    entry = _stdlib_archive_cache_entry(archive)
    return None if entry is None else entry[0].parent.parent


def shared_stdlib_publish_lock_path(archive: Path) -> Path:
    """Shared entry stripes; standalone explicit archives retain their own lock."""
    entry = _stdlib_archive_cache_entry(archive)
    if entry is not None:
        return entry[1]
    return archive.with_name(f"{archive.name}.publish.lock")


def cache_home_root(cache_root: Path) -> Path:
    """The default durable user home, never an evictable cache entry."""
    return cache_root / "home"


@functools.lru_cache(maxsize=128)
def _default_molt_home_cached(path: str, cwd_str: str) -> Path:
    selected = Path(path)
    return selected if selected.is_absolute() else (Path(cwd_str) / selected).absolute()


def _default_molt_home(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    # Installation custody keeps its original explicit selectors and user
    # location. A new family binding for regenerable cache cannot move it.
    selected = user_state_location("installation", environ=environ, cwd=cwd).path
    return _default_molt_home_cached(
        str(selected), os.fspath(Path.cwd() if cwd is None else cwd)
    )


def _default_installed_runtime_store(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    """Mutable installed runtime payloads, independent of frontend provisioning."""
    env = os.environ if environ is None else environ
    selected = _path_override(env, "MOLT_INSTALLED_RUNTIME_STORE")
    if selected is None:
        return _default_molt_home(environ=environ, cwd=cwd) / "installed-runtime"
    path = Path(selected)
    return (
        path
        if path.is_absolute()
        else ((Path.cwd() if cwd is None else cwd) / path).absolute()
    )


@functools.lru_cache(maxsize=128)
def _default_molt_bin_cached(path: str, cwd_str: str) -> Path:
    selected = Path(path)
    return selected if selected.is_absolute() else (Path(cwd_str) / selected).absolute()


def _default_molt_bin(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    env = os.environ if environ is None else environ
    selected = _path_override(env, "MOLT_BIN")
    if selected is None:
        selected = os.fspath(_default_molt_home(environ=environ, cwd=cwd) / "bin")
    return _default_molt_bin_cached(
        selected, os.fspath(Path.cwd() if cwd is None else cwd)
    )


def protected_cache_roots(
    cache_root: Path, *, environ: Mapping[str, str] | None = None
) -> tuple[Path, ...]:
    """Return the top-level cache children that hold the selected home or bin.

    The default Molt home lives in the cache and owns installed tool state
    (``<MOLT_HOME>/target-root``) and the bin directory; neither is an evictable
    cache entry. Defaults bind to the cache being pruned, even when
    ``--cache-dir`` names another namespace than the process's ``MOLT_CACHE``.
    """
    root = cache_root.resolve(strict=False)
    environment = dict(os.environ if environ is None else environ)
    environment["MOLT_CACHE"] = os.fspath(root)
    protected = {cache_control_root(root), cache_home_root(root)}
    for selected in (
        cache_home_root(root),
        _default_molt_home(environ=environment),
        _default_molt_bin(environ=environment),
    ):
        for candidate in (selected.absolute(), selected.resolve(strict=False)):
            try:
                relative = candidate.relative_to(root)
            except ValueError:
                continue
            protected.add(root / relative.parts[0] if relative.parts else root)
    return tuple(sorted(protected, key=os.fspath))
