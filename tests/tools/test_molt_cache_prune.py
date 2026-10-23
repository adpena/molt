from __future__ import annotations

import hashlib
import json
import os
import socket
import subprocess
import sys
import time
from itertools import product
from contextlib import contextmanager
from pathlib import Path

import pytest

from molt.cli.wasm_link_cache import (
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_entry,
    _read_wasm_link_cache_entry,
    _wasm_link_cache_entry,
)
from tests.process_guard_common import (
    close_owned_test_process,
    install_module_view,
    run_custody_subject_process,
    start_owned_test_process,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
MOLT_CACHE_PRUNE = REPO_ROOT / "tools" / "molt_cache_prune.py"
WASM_BYTES = b"\x00asm\x01\x00\x00\x00cache-entry"


def _load_molt_cache_prune():
    from molt import cache_eviction

    return cache_eviction


def _cache_key(label: str) -> str:
    """A real SHA-256 cache key; an `xx-` label prefix pins the lock stripe."""
    digest = hashlib.sha256(label.encode("utf-8")).hexdigest()
    stripe, _, _rest = label.partition("-")
    if len(stripe) == 2 and all(char in "0123456789abcdef" for char in stripe):
        return stripe + digest[2:]
    return digest


def _cache_entry(cache_root: Path, label: str):
    return _wasm_link_cache_entry(
        "runtime_tree_shake",
        "runtime-tree-shake-test",
        _cache_key(label),
        cache_root=cache_root / "wasm_link",
    )


def test_pruner_treats_wasm_link_content_keys_as_independent_entries(
    tmp_path: Path,
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "first")
    second = _cache_entry(cache_root, "second")
    for entry in (first, second):
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)

    entries = module._collect_entries(cache_root)

    assert {entry.path for entry in entries} == {first.root, second.root}
    assert all(entry.lock_path is not None for entry in entries)


def test_pruner_preserves_locked_wasm_link_entry_then_removes_it(
    tmp_path: Path,
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    entry = _cache_entry(cache_root, "active")
    with _locked_wasm_link_cache_entry(entry):
        _publish_wasm_link_cache_entry(entry, WASM_BYTES)
        old = time.time() - 86400
        os.utime(entry.root, (old, old))
        active_result = module._prune(
            cache_root, max_bytes=None, max_age_days=0, dry_run=False
        )
        assert active_result["entries_removed"] == 0
        assert entry.artifact.is_file()

    old = time.time() - 86400
    os.utime(entry.root, (old, old))
    released_result = module._prune(
        cache_root, max_bytes=None, max_age_days=0, dry_run=False
    )
    assert released_result["entries_removed"] == 1
    assert not entry.root.exists()


def test_pruner_default_root_reuses_canonical_molt_cache(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "canonical-cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))

    assert module._default_cache_root() == cache_root


def test_wasm_link_cache_locks_use_bounded_stable_stripes(tmp_path: Path) -> None:
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "aa-first")
    same_stripe = _cache_entry(cache_root, "aa-second")
    other_stripe = _cache_entry(cache_root, "bb-third")

    assert first.lock == same_stripe.lock
    assert first.lock != other_stripe.lock
    assert first.lock.name == "aa.lock"
    assert first.key not in first.lock.name
    assert (
        first.lock.parent == cache_root / "wasm_link" / "runtime_tree_shake" / ".locks"
    )


def test_pruner_breaks_equal_mtime_ties_by_normalized_key_path(tmp_path: Path) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "aa-first")
    second = _cache_entry(cache_root, "bb-second")
    for entry in (second, first):
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)
    tied_mtime = time.time() - 60
    for entry in (first, second):
        os.utime(entry.root, (tied_mtime, tied_mtime))

    result = module._prune(
        cache_root,
        max_bytes=max(
            module._entry_size_bytes(first.root),
            module._entry_size_bytes(second.root),
        ),
        max_age_days=None,
        dry_run=False,
    )

    assert result["entries_removed"] == 1
    assert not first.root.exists()
    assert second.root.exists()


def test_budget_comes_from_flags_then_projected_env_then_canonical_defaults() -> None:
    from molt import default_paths

    assert default_paths.resolve_budget(None, None, {}) == (
        float(default_paths.DEFAULT_MOLT_CACHE_MAX_GB),
        int(default_paths.DEFAULT_MOLT_CACHE_MAX_AGE_DAYS),
    )
    projected = {"MOLT_CACHE_MAX_GB": "7.5", "MOLT_CACHE_MAX_AGE_DAYS": "3"}
    assert default_paths.resolve_budget(None, None, projected) == (7.5, 3)
    assert default_paths.resolve_budget(2.0, 1, projected) == (2.0, 1)


def test_pruner_never_evicts_the_selected_home_or_bin(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_molt_cache_prune()
    for name in ("MOLT_HOME", "MOLT_BIN"):
        monkeypatch.delenv(name, raising=False)
    cache_root = tmp_path / "cache"
    tools = cache_root / "home" / "target-root" / "toolchains" / "tool"
    tools.parent.mkdir(parents=True)
    tools.write_bytes(b"provisioned tool state")
    from molt.default_paths import shared_stdlib_cache_entry

    stale, _lock = shared_stdlib_cache_entry(cache_root, "a" * 64)
    stale.mkdir(parents=True)
    (stale / "payload").write_bytes(b"evictable")
    old = time.time() - 30 * 86400
    for path in (tools, tools.parent, cache_root / "home", stale / "payload", stale):
        os.utime(path, (old, old))

    summary = module._prune(cache_root, max_bytes=0, max_age_days=0, dry_run=False)

    assert not stale.exists()
    assert tools.read_bytes() == b"provisioned tool state"
    assert set(summary["protected_state"]) == {
        str((cache_root / name).resolve()) for name in ("home", "locks")
    }


def test_pruner_preserves_unowned_native_names_and_does_not_follow_links(tmp_path):
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    namespace = cache / "shared-stdlib"
    namespace.mkdir(parents=True)
    (namespace / "_molt_generated_old.py").write_bytes(b"old flat namespace")
    (namespace / ".DS_Store").write_bytes(b"unknown metadata")
    (namespace / "unknown-directory").mkdir()
    outside = tmp_path / "unrelated"
    outside.mkdir()
    protected = outside / "owner-data"
    protected.write_bytes(b"outside cache")
    try:
        (namespace / "link").symlink_to(outside, target_is_directory=True)
        (namespace / "dangling").symlink_to(tmp_path / "absent")
    except OSError as exc:
        pytest.skip(f"symlink creation is unavailable: {exc}")
    result = module._prune(cache, max_bytes=0, max_age_days=0, dry_run=False)
    assert result["entries_removed"] == 0
    assert result["entries_failed"] == 0
    assert len(list(namespace.iterdir())) == 5
    assert protected.read_bytes() == b"outside cache"


def test_pruner_accepts_entries_vanished_after_directory_listing(tmp_path, monkeypatch):
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    namespace = cache / "shared-stdlib"
    stale = namespace / ("a" * 64)
    stale.mkdir(parents=True)
    real_children = module._children

    def race(path):
        children = real_children(path)
        if path == namespace:
            stale.rmdir()
        return children

    monkeypatch.setattr(module, "_children", race)
    result = module._prune(cache, max_bytes=0, max_age_days=0, dry_run=False)
    assert result["entries_removed"] == result["entries_failed"] == 0
    assert (cache / "locks" / "prune.lock").is_file()


def _snapshot_tree(root: Path):
    from molt.default_paths import cache_control_root
    from molt.file_hashing import content_change_time_ns

    control_root = cache_control_root(root)
    result = {}
    for path in [root, *sorted(root.rglob("*"))]:
        info = path.lstat()
        # Windows byte-range locks prevent data reads, including by this
        # observer. The control owner is observed through native metadata;
        # logical cache payloads retain the independent byte-content oracle.
        control_file = (
            os.name == "nt" and path.is_relative_to(control_root) and path.is_file()
        )
        if control_file:
            change_time = content_change_time_ns(path, info)
            assert change_time is not None, f"cannot observe cache control: {path}"
        payload = (
            ("symlink", os.readlink(path))
            if path.is_symlink()
            else ("control", (info.st_size, change_time))
            if control_file
            else ("file", path.read_bytes())
            if path.is_file()
            else ("directory", None)
        )
        result[str(path.relative_to(root))] = (
            info.st_dev,
            info.st_ino,
            info.st_mtime_ns,
            payload,
        )
    return result


def _run_pruner(cache: Path, *, dry_run: bool = False):
    command = [
        sys.executable,
        "-B",
        str(MOLT_CACHE_PRUNE),
        "--cache-dir",
        str(cache),
        "--max-gb",
        "0",
        "--max-age-days",
        "0",
    ]
    if dry_run:
        command.append("--dry-run")
    completed = run_custody_subject_process(
        command, capture_output=True, text=True, timeout=30
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    return completed.stdout


@pytest.mark.parametrize("existing_locks", [False, True])
def test_real_pruner_dry_run_creates_no_locks_or_other_files(tmp_path, existing_locks):
    from molt.default_paths import (
        native_stdlib_archive_cache_entry,
        native_stdlib_object_cache_entry,
        shared_stdlib_cache_entry,
    )

    cache = tmp_path / "cache"
    stdlib, stdlib_lock = shared_stdlib_cache_entry(cache, "b" * 64)
    wasm = _cache_entry(cache, "cc-dry-run")
    native, native_lock = native_stdlib_object_cache_entry(cache, "d" * 64)
    content, content_lock = native_stdlib_archive_cache_entry(cache, "e" * 64)
    for root in (stdlib, wasm.root, native, content):
        root.mkdir(parents=True)
        (root / "payload").write_bytes(b"reclaimable")
    if existing_locks:
        for lock in (
            stdlib_lock,
            wasm.lock,
            native_lock,
            content_lock,
        ):
            lock.parent.mkdir(parents=True, exist_ok=True)
            lock.write_bytes(b"unchanged advisory contents")
    before = _snapshot_tree(cache)
    output = _run_pruner(cache, dry_run=True)
    assert "removed.entries=4" in output
    assert _snapshot_tree(cache) == before


def test_pruner_cli_imports_only_light_cache_authorities(tmp_path):
    code = r"""import builtins, json, runpy, sys, time
# Guard bootstrap may already own heavy modules before the subject starts.
baseline = set(sys.modules)
def forbidden(name):
    return name in ('molt.cli', 'molt.frontend', 'molt.dx') or name.startswith(
        ('molt.cli.', 'molt.frontend.', 'molt.dx.'))
original_import = builtins.__import__
def light_import(name, globals=None, locals=None, fromlist=(), level=0):
    requested = [name] + [name + '.' + item for item in (fromlist or ())]
    assert not any(forbidden(item) for item in requested), requested
    return original_import(name, globals, locals, fromlist, level)
builtins.__import__ = light_import
# Sensitivity includes cached imports: preloading dx must not conceal a new dependency.
for statement in ('import molt.dx', 'from molt import dx', 'import molt.cli'):
    try:
        exec(statement, {})
    except AssertionError:
        pass
    else:
        raise AssertionError('import oracle missed ' + statement)
started = time.perf_counter()
sys.argv = [sys.argv[1], '--cache-dir', sys.argv[2], '--dry-run']
try:
    runpy.run_path(sys.argv[0], run_name='__main__')
except SystemExit as exit:
    assert exit.code == 0
finally:
    builtins.__import__ = original_import
unexpected = [name for name in set(sys.modules) - baseline if forbidden(name)]
assert unexpected == [], unexpected
print(json.dumps({'elapsed_seconds': time.perf_counter() - started,
                  'loaded_modules': len(sys.modules)}))
"""
    cache = tmp_path / "nonexistent"
    completed = run_custody_subject_process(
        [sys.executable, "-B", "-c", code, str(MOLT_CACHE_PRUNE), str(cache)],
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    measurement = json.loads(completed.stdout.splitlines()[-1])
    assert measurement["elapsed_seconds"] >= 0
    assert not cache.exists()


_CACHE_OWNER_CODE = r"""
import json, os, socket, sys
from pathlib import Path
mode, cache, project, port = sys.argv[1:]
cache, project = Path(cache), Path(project)
os.environ['MOLT_CACHE'] = str(cache)
os.environ['MOLT_MAX_CONCURRENT_BUILDS'] = '1'
from molt.default_paths import shared_cache_lock_path, shared_stdlib_publish_lock_path

def barrier(entry, lock):
    # The OS lock is held before the child announces readiness. The release
    # wait has no deadline: the parent's test body takes as long as the host
    # allows, and a dead parent closes the socket, so recv returns b''.
    with socket.create_connection(('127.0.0.1', int(port)), timeout=20) as channel:
        channel.sendall((json.dumps({'entry': str(entry) if entry else None,
                                    'lock': str(lock)}) + '\n').encode())
        channel.settimeout(None)
        assert channel.recv(1) == b'R'

if mode == 'slot':
    from molt.cli.cargo_execution import _build_slot, _build_slot_dir
    with _build_slot() as index:
        assert index == 0
        barrier(None, _build_slot_dir() / 'slot-0.lock')
elif mode == 'stdlib':
    from molt.cli.backend_cache import _stdlib_object_cache_path, _shared_stdlib_cache_lock
    from molt.file_publication import staged_file_path
    archive = _stdlib_object_cache_path(cache / 'program.o', 'a' * 64)
    with _shared_stdlib_cache_lock(archive):
        archive.parent.mkdir(parents=True, exist_ok=True)
        archive.write_bytes(b'shared archive generation')
        for suffix in ('.count', '.key', '.manifest.json', '.partition.json',
                       '.sha256', '.symbol-contract.json', '.symbols.json'):
            archive.with_suffix(suffix).write_bytes(suffix.encode())
        staged_file_path(archive).write_bytes(b'in-flight publication')
        barrier(archive.parent, shared_stdlib_publish_lock_path(archive))
else:
    raise AssertionError(mode)
print('RELEASED')
"""


_OWNER_START_DEADLINE_S = 300.0


def _accept_owner(
    server: socket.socket, process: subprocess.Popen[str]
) -> socket.socket:
    """Wait for the owner's connection while the owner is alive.

    Its start imports the CLI, which a loaded host can delay for tens of
    seconds; liveness, not a short deadline, decides. An owner that exits
    first fails the test with its output.
    """
    server.settimeout(0.5)
    deadline = time.monotonic() + _OWNER_START_DEADLINE_S
    while True:
        try:
            channel, _address = server.accept()
            return channel
        except TimeoutError:
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                raise AssertionError(
                    f"cache owner exited {process.returncode} before readiness: "
                    f"{stdout}{stderr}"
                ) from None
            if time.monotonic() > deadline:
                raise


@contextmanager
def _active_cache_owner(mode: str, cache: Path, project: Path):
    with socket.socket() as server:
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        process = start_owned_test_process(
            [
                sys.executable,
                "-B",
                "-c",
                _CACHE_OWNER_CODE,
                mode,
                str(cache),
                str(project),
                str(server.getsockname()[1]),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        channel = None
        try:
            channel = _accept_owner(server, process)
            channel.settimeout(30)
            with channel.makefile("rb") as stream:
                ready = json.loads(stream.readline())
            yield ready
            channel.sendall(b"R")
            stdout, stderr = process.communicate(timeout=30)
            assert process.returncode == 0, stdout + stderr
            assert stdout.strip() == "RELEASED"
        finally:
            if channel is not None:
                channel.close()
            close_owned_test_process(process)


def _assert_external_lock_availability(lock: Path, available: bool):
    code = r"""import sys
from pathlib import Path
from molt.file_locks import _try_acquire_file_lock, _release_file_lock
handle = _try_acquire_file_lock(Path(sys.argv[1]), create=False)
try:
    assert (handle is not None) == (sys.argv[2] == 'available')
finally:
    if handle is not None:
        _release_file_lock(handle)
"""
    result = run_custody_subject_process(
        [
            sys.executable,
            "-B",
            "-c",
            code,
            str(lock),
            "available" if available else "busy",
        ],
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("mode", ["slot", "stdlib"])
def test_real_pruner_respects_other_process_owner_and_reclaims_after_release(
    tmp_path, monkeypatch, mode
):
    from molt.default_paths import shared_stdlib_cache_entry

    cache = tmp_path / "cache"
    project = tmp_path / "project"
    namespace = project / "ns_pkg"
    namespace.mkdir(parents=True)
    (project / "main.py").write_text(
        "import ns_pkg.leaf\nprint(ns_pkg.leaf.VALUE)\n", encoding="utf-8"
    )
    (namespace / "leaf.py").write_text("VALUE = 7\n", encoding="utf-8")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "active-planner"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    monkeypatch.setenv("MOLT_CACHE", str(cache))
    monkeypatch.delenv("MOLT_HOME", raising=False)
    monkeypatch.delenv("MOLT_BIN", raising=False)
    retained = [cache / "locks" / "neighbor.state", cache / "home" / "bin" / "molt"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"durable owner state")
    snapshots = {path: (path.stat().st_ino, path.read_bytes()) for path in retained}
    with _active_cache_owner(mode, cache, project) as ready:
        lock = Path(ready["lock"])
        entry = Path(ready["entry"]) if ready["entry"] else None
        os.utime(lock, (1, 1))
        lock_before = lock.stat()
        entry_before = _snapshot_tree(entry) if entry else None
        first_byte = "00" if entry is None or entry.name[:2] != "00" else "ff"
        inactive, _name = shared_stdlib_cache_entry(cache, first_byte * 32)
        inactive.mkdir(parents=True)
        (inactive / "stale.py").write_bytes(b"old generation")
        unknown = cache / "shared-stdlib" / ".DS_Store"
        unknown.write_bytes(b"harmless unknown child")
        unknown_before = (unknown.stat().st_ino, unknown.read_bytes())
        retired_flat = cache / ("stdlib_shared_" + "b" * 64 + ".a")
        retired_flat.write_bytes(b"legacy active writer may still own this")
        output = _run_pruner(cache)
        assert not inactive.exists() and retired_flat.exists()
        assert (unknown.stat().st_ino, unknown.read_bytes()) == unknown_before
        assert "failed.entries=0" in output
        if entry is not None:
            assert "busy.entries=1" in output
            assert _snapshot_tree(entry) == entry_before
        assert (lock.stat().st_dev, lock.stat().st_ino, lock.stat().st_mtime_ns) == (
            lock_before.st_dev,
            lock_before.st_ino,
            lock_before.st_mtime_ns,
        )
        for path, expected in snapshots.items():
            assert (path.stat().st_ino, path.read_bytes()) == expected
        _assert_external_lock_availability(lock, False)
        before_dry_run = _snapshot_tree(cache)
        _run_pruner(cache, dry_run=True)
        assert _snapshot_tree(cache) == before_dry_run
    _assert_external_lock_availability(lock, True)
    output = _run_pruner(cache)
    assert "busy.entries=0" in output
    assert entry is None or not entry.exists()
    assert lock.stat().st_ino == lock_before.st_ino
    assert all(
        (path.stat().st_ino, path.read_bytes()) == snapshots[path] for path in retained
    )


def test_pruner_removes_abandoned_stdlib_stages_without_archive(tmp_path):
    from molt.cli.backend_cache import _stdlib_object_cache_path
    from molt.file_publication import staged_file_path

    cache = tmp_path / "cache"
    archive = _stdlib_object_cache_path(cache / "program.o", "a" * 64)
    assert archive is not None
    archive.parent.mkdir(parents=True, exist_ok=True)
    stage = staged_file_path(archive)
    stage.write_bytes(b"abandoned unpublished generation")
    assert not archive.exists()
    output = _run_pruner(cache)
    assert "removed.entries=1" in output
    assert not stage.exists() and not archive.parent.exists()


@pytest.mark.parametrize(
    "relative",
    [
        "wasm_link",
        "wasm_link/runtime_tree_shake",
        "wasm_link/runtime_tree_shake/schema",
        "shared-stdlib",
        "native-stdlib-archives-v1",
        "native-stdlib-objects-v1",
    ],
)
def test_pruner_does_not_traverse_namespace_symlinks(tmp_path, relative):
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "valuable").write_bytes(b"unrelated ownership")
    link = cache / relative
    link.parent.mkdir(parents=True)
    try:
        link.symlink_to(outside, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"symlink creation is unavailable: {exc}")
    result = module._prune(cache, max_bytes=0, max_age_days=0, dry_run=False)
    assert result["entries_removed"] == 0
    assert link.is_symlink()
    assert (outside / "valuable").read_bytes() == b"unrelated ownership"


def test_legacy_standalone_index_matches_original_name_predicate():
    module = _load_molt_cache_prune()
    # Exercise empty components, overlapping stems and leading-dot stages.
    # Each singleton matters: a second owner must not mask an overbroad match.
    stems = {
        ".".join(parts)
        for width in (1, 2, 3)
        for parts in product(("", "a", "b"), repeat=width)
    }
    stems.update(("foo.bar", ".foo.bar", "foo..bar", "é.λ"))
    names = stems | {
        prefix + stem + suffix
        for stem in stems
        for prefix in ("", ".")
        for suffix in (".", ".key", ".a.123.tmp", "neighbor.key")
    }
    for owners in [*((stem,) for stem in sorted(stems)), tuple(sorted(stems))]:
        index = module._LegacyStandaloneNames()
        for stem in owners:
            index.add(stem)
        for name in names:
            expected = any(
                name == stem or name.startswith((stem + ".", "." + stem + "."))
                for stem in owners
            )
            assert index.protects(name) == expected, (owners, name)


def test_mixed_legacy_native_writers_are_not_eviction_candidates(tmp_path):
    from molt.default_paths import shared_stdlib_cache_entry

    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    legacy_archive = "a" * 64 + ".artifact-" + "c" * 64 + ".a"
    legacy = [
        cache / ("stdlib_shared_" + "a" * 64 + ".a"),
        cache / (".stdlib_shared_" + "a" * 64 + ".a.123.456.objects.tmp") / "job.json",
        cache / "native-stdlib-objects-v1" / "ab" / ("ab" * 32 + ".entry"),
        cache / "native-stdlib-objects-v1" / "ab" / ("ab" * 32 + ".entry.publish.lock"),
        cache / "explicit.a",
        cache / "explicit.a.publish.lock",
        cache / "explicit.sha256",
        cache / legacy_archive,
        cache / (legacy_archive + ".publish.lock"),
        cache / ("." + legacy_archive + ".123.tmp"),
    ]
    for path in legacy:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"possibly active old producer")
    identity = {path: (path.stat().st_ino, path.read_bytes()) for path in legacy}
    entry, _ = shared_stdlib_cache_entry(cache, "f" * 64)
    entry.mkdir(parents=True)
    (entry / "stdlib.a").write_bytes(b"inactive current generation")
    neighbor = cache / ("a" * 64 + ".artifact-" + "d" * 64 + ".a")
    neighbor.write_bytes(b"completed current artifact without legacy owner")
    result = module._prune(cache, max_bytes=0, max_age_days=0, dry_run=False)
    assert result["entries_removed"] == 2
    assert not neighbor.exists()
    assert not entry.exists()
    assert identity == {
        path: (path.stat().st_ino, path.read_bytes()) for path in legacy
    }
    assert result["legacy_native_state"]


def test_legacy_retention_preserves_an_open_lock_inode_and_active_stage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    cache.mkdir()
    archive = cache / ("stdlib_shared_" + "a" * 64 + ".a")
    lock = archive.with_name(archive.name + ".publish.lock")
    stage = cache / ("." + archive.name + ".123.456.objects.tmp") / "job.json"
    stage.parent.mkdir()
    archive.write_bytes(b"old writer's required archive")
    stage.write_bytes(b"old compilation still owns this stage")
    lock.write_bytes(b"")
    now = 100 * 86400.0
    clock = install_module_view(monkeypatch, "time", time, module, time=lambda: now)
    old = cache / ("b" * 64 + ".artifact-" + "c" * 64 + ".a")
    hot = cache / ("d" * 64 + ".artifact-" + "e" * 64 + ".a")
    old.write_bytes(b"optional inactive current artifact")
    hot.write_bytes(b"optional recently used current artifact")
    for path in (archive, stage, stage.parent, lock, old):
        os.utime(path, (1, 1))
    os.utime(hot, (now, now))
    payloads = {path: path.read_bytes() for path in (archive, stage, hot)}
    identity = lock.stat()
    # A waiter can open the old inode before its current holder releases it.
    # No lock held now does not imply no descriptor can acquire that inode.
    with lock.open("rb") as waiting:
        handle = _try_acquire_file_lock(lock, create=False)
        assert handle is not None
        try:
            result = module._prune(cache, max_bytes=None, max_age_days=7, dry_run=False)
            assert result["entries_removed"] == 1 and not old.exists()
            assert os.path.samestat(identity, os.fstat(waiting.fileno()))
            assert os.path.samestat(identity, lock.stat())
        finally:
            _release_file_lock(handle)
        clock.time = lambda: now + 3600
        result = module._prune(cache, max_bytes=None, max_age_days=7, dry_run=False)
        assert result["entries_removed"] == 0
        assert os.path.samestat(identity, lock.stat())
        assert os.path.samestat(lock.stat(), os.fstat(waiting.fileno()))
        assert {path: path.read_bytes() for path in payloads} == payloads


@pytest.mark.parametrize("family", ["module_analysis", "module_lowering"])
def test_frontend_retention_evicts_old_immutable_leaves_to_budget(tmp_path, family):
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    root = cache / family
    root.mkdir(parents=True)
    old, recent, staging = (root / name for name in ("a.json", "b.json", ".b.tmp"))
    old.write_bytes(b"0123456789")
    recent.write_bytes(b"abcdefghij")
    staging.write_bytes(b"live atomic publication")
    os.utime(old, (1, 1))
    os.utime(recent, (2, 2))
    result = module._prune(cache, max_bytes=10, max_age_days=None, dry_run=False)
    assert result["bytes_remaining"] == 10
    assert not old.exists()
    assert recent.read_bytes() == b"abcdefghij"
    assert staging.read_bytes() == b"live atomic publication"


@pytest.fixture
def retention_cache(tmp_path):
    from molt import cache_eviction

    cache = tmp_path / "cache"
    (cache / "module_lowering").mkdir(parents=True)
    entry = cache / "module_lowering/entry.json"
    entry.write_bytes(b"old immutable cache payload")
    os.utime(entry, (1, 1))
    env = {
        "MOLT_CACHE": str(cache),
        "MOLT_CACHE_PRUNE": "1",
        "MOLT_CACHE_MAX_GB": "0",
        "MOLT_CACHE_MAX_AGE_DAYS": "30",
    }
    return cache_eviction, cache, entry, env


def test_retention_coalesces_under_canonical_lock_and_explicit_prune_is_unthrottled(
    retention_cache, monkeypatch
):
    module, cache, entry, env = retention_cache
    first = module.automatic_retention(env)
    assert first["status"] == "completed" and first["entries_removed"] == 1
    assert not entry.exists()
    entry.write_bytes(b"published after the sweep")
    with monkeypatch.context() as patch:
        patch.setattr(
            _load_molt_cache_prune(),
            "_collect_entries",
            lambda *_args, **_kwargs: pytest.fail(
                "recent automatic sweep must not inventory"
            ),
        )
        assert module.automatic_retention(dict(env))["status"] == "recent"
    assert entry.exists()
    result = _load_molt_cache_prune()._prune(
        cache, max_bytes=0, max_age_days=None, dry_run=False
    )
    assert result["entries_removed"] == 1 and not entry.exists()


def test_retention_interval_is_the_callers_staleness_tolerance(retention_cache):
    # One stamp serves every automatic caller. A CLI build tolerates a day-old
    # sweep (CLI_CACHE_RETENTION_INTERVAL_SECONDS); the proof queue sweeps again
    # after five minutes.
    from molt.default_paths import (
        CACHE_MAINTENANCE_INTERVAL_SECONDS,
        CLI_CACHE_RETENTION_INTERVAL_SECONDS,
        shared_cache_lock_path,
    )

    module, cache, entry, env = retention_cache
    stamp = shared_cache_lock_path(cache, "prune")
    stamp.parent.mkdir(parents=True)
    stamp.write_text(str(time.time() - 3600), encoding="ascii")
    assert (
        CACHE_MAINTENANCE_INTERVAL_SECONDS < 3600 < CLI_CACHE_RETENTION_INTERVAL_SECONDS
    )
    daily = module.automatic_retention(
        env, min_interval_seconds=CLI_CACHE_RETENTION_INTERVAL_SECONDS
    )
    assert daily["status"] == "recent" and entry.exists()
    queue = module.automatic_retention(env)
    assert queue["status"] == "completed" and queue["entries_removed"] == 1
    assert queue["bytes_removed"] == len(b"old immutable cache payload")
    assert not entry.exists()


def test_retention_protects_home_and_bin_from_its_supplied_environment(
    retention_cache, monkeypatch
):
    from molt.default_paths import backend_cache_artifact_filename

    module, cache, home_entry, env = retention_cache
    bin_root = cache / "module_analysis"
    bin_root.mkdir()
    bin_entry = bin_root / "installed.json"
    bin_entry.write_bytes(b"selected executable state")
    evictable = cache / backend_cache_artifact_filename(
        "a" * 64 + "|artifact:" + "b" * 64, ".a"
    )
    evictable.write_bytes(b"optional archive")
    monkeypatch.setenv("MOLT_HOME", str(cache / "ambient-home"))
    monkeypatch.setenv("MOLT_BIN", str(cache / "ambient-bin"))
    ambient = dict(os.environ)
    selected = {
        **env,
        "MOLT_HOME": str(home_entry.parent),
        "MOLT_BIN": str(bin_root),
    }
    result = module.automatic_retention(selected)
    assert result["entries_removed"] == 1
    assert not evictable.exists()
    assert home_entry.read_bytes() == b"old immutable cache payload"
    assert bin_entry.read_bytes() == b"selected executable state"
    assert dict(os.environ) == ambient


def test_retention_busy_custody_never_waits_or_inventories(
    retention_cache, monkeypatch
):
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    module, cache, entry, env = retention_cache
    lock = _try_acquire_file_lock(cache / "locks/prune.lock")
    assert lock is not None
    try:
        with monkeypatch.context() as patch:
            patch.setattr(
                _load_molt_cache_prune(),
                "_collect_entries",
                lambda *_args, **_kwargs: pytest.fail("busy sweep must not inventory"),
            )
            assert module.automatic_retention(env)["status"] == "busy"
        assert entry.exists()
    finally:
        _release_file_lock(lock)
    assert module.automatic_retention(env)["entries_removed"] == 1


def test_retention_throttles_failed_attempt_and_obeys_optout(
    retention_cache, monkeypatch
):
    module, cache, entry, env = retention_cache
    pruner = _load_molt_cache_prune()
    now = time.time()
    # The pruner's private clock; other modules keep the real one.
    clock = install_module_view(monkeypatch, "time", time, pruner, time=lambda: now)
    with monkeypatch.context() as patch:

        def fail(_root, **_kwargs):
            raise OSError("inventory unavailable")

        patch.setattr(pruner, "_collect_entries", fail)
        first = module.automatic_retention(env)
    assert first == {"status": "failed", "error": "OSError: inventory unavailable"}
    assert module.automatic_retention(env)["status"] == "recent"
    clock.time = lambda: now + pruner.AUTOMATIC_PRUNE_INTERVAL_SECONDS
    assert module.automatic_retention({**env, "MOLT_CACHE_PRUNE": "0"}) is None
    assert entry.exists()
    assert module.automatic_retention(env)["entries_removed"] == 1


@pytest.mark.parametrize("failure", [MemoryError, KeyboardInterrupt])
def test_retention_does_not_swallow_resource_or_cancellation_failure(
    retention_cache, monkeypatch, failure
):
    module, _cache, _entry, env = retention_cache

    def fail(_root, **_kwargs):
        raise failure("guarded operation interrupted")

    monkeypatch.setattr(_load_molt_cache_prune(), "_collect_entries", fail)
    with pytest.raises(failure, match="guarded operation interrupted"):
        module.automatic_retention(env)


def test_pruner_selects_completed_backend_files_and_preserves_mutable_namespaces(
    tmp_path,
):
    from molt.default_paths import backend_cache_artifact_filename
    from molt.file_publication import staged_file_path

    cache = tmp_path / "cache"
    cache.mkdir()
    completed = cache / backend_cache_artifact_filename(
        "a" * 64 + "|artifact:" + "b" * 64, ".a"
    )
    completed.write_bytes(b"completed immutable archive")
    stage = staged_file_path(completed, purpose="backend-cache")
    stage.write_bytes(b"live unpublished archive")
    protected = [stage]
    for namespace in ("runtime_wasm_generations", "vendor", "f" * 64):
        payload = cache / namespace / "live-stage"
        payload.parent.mkdir()
        payload.write_bytes(b"producer-owned state")
        protected.append(payload)
    before = {path: (path.stat().st_ino, path.read_bytes()) for path in protected}
    result = _load_molt_cache_prune()._prune(
        cache, max_bytes=0, max_age_days=0, dry_run=False
    )
    assert result["entries_removed"] == 1
    assert not completed.exists()
    assert {
        path: (path.stat().st_ino, path.read_bytes()) for path in protected
    } == before


def test_required_backend_output_survives_eviction_after_each_cache_publication(
    tmp_path, monkeypatch
):
    from molt.cli import backend_cache
    from molt.cli.backend_artifact_contract import (
        BackendArtifactContract,
        BackendArtifactKind,
    )

    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    source = tmp_path / "private.rs"
    payload = b'fn main() { println!("required output"); }\n'
    source.write_bytes(payload)
    cache = tmp_path / "cache"
    module = backend_cache._backend_cache_artifact_path(
        cache, "a" * 64, stdlib_object_cache_key=None, artifact_contract=contract
    )
    function = backend_cache._backend_cache_artifact_path(
        cache, "b" * 64, stdlib_object_cache_key=None, artifact_contract=contract
    )
    real_publish = backend_cache._publish_immutable_backend_cache_artifact
    published = []

    def publish_then_evict(src, dst, **kwargs):
        identity = real_publish(src, dst, **kwargs)
        published.append(dst)
        result = _load_molt_cache_prune()._prune(
            cache, max_bytes=0, max_age_days=0, dry_run=False
        )
        assert result["entries_removed"] == 1
        assert not dst.exists()
        return identity

    monkeypatch.setattr(
        backend_cache, "_publish_immutable_backend_cache_artifact", publish_then_evict
    )
    output = tmp_path / "output.rs"
    error = backend_cache._stage_backend_output_and_caches(
        tmp_path,
        source,
        output,
        cache_path=module,
        cache_key="a" * 64,
        stdlib_object_cache_key=None,
        function_cache_path=function,
        warnings=[],
        output_already_synced=False,
        state_path=tmp_path / "state.json",
        artifact_contract=contract,
    )
    assert error is None
    assert published == [module, function]
    assert output.read_bytes() == payload
    assert not source.exists()  # Producer cleanup happens only after both transfers.


@pytest.mark.parametrize(
    "relative,allowed",
    [
        ("home/build/app/shared-stdlib-link/key/stdlib.a", True),
        ("wasm_link/schema/key/owned.a", False),
        ("module_lowering/entry.json", False),
        ("unclassified/owned.a", False),
    ],
)
def test_required_archive_custody_uses_durable_home_not_eligibility_registry(
    tmp_path, relative, allowed
):
    from molt.default_paths import validate_required_stdlib_output

    root = tmp_path / "cache"
    source = root / "shared-stdlib" / ("a" * 64) / "stdlib.a"
    output = root / relative
    if allowed:
        validate_required_stdlib_output(output, source)
    else:
        with pytest.raises(ValueError, match="canonical cache/home"):
            validate_required_stdlib_output(output, source)
    validate_required_stdlib_output(tmp_path / "outside" / "owned.a", source)


def test_canonical_home_physical_target_remains_protected_when_selected_home_changes(
    tmp_path, monkeypatch
):
    from molt.default_paths import (
        protected_cache_roots,
        validate_required_stdlib_output,
    )

    root = tmp_path / "cache"
    target = root / "module_lowering"
    target.mkdir(parents=True)
    (root / "home").symlink_to(target, target_is_directory=True)
    monkeypatch.setenv("MOLT_HOME", str(tmp_path / "different-home"))
    owned = target / "required.json"
    owned.write_bytes(b"required output")
    source = root / "shared-stdlib" / ("a" * 64) / "stdlib.a"
    validate_required_stdlib_output(owned, source)
    assert root / "module_lowering" in protected_cache_roots(root)
    _load_molt_cache_prune()._prune(root, max_bytes=0, max_age_days=0, dry_run=False)
    assert owned.read_bytes() == b"required output"
    # Removing canonical home protection exposes the same real eligible leaf.
    # This control prevents a non-eligible nested fixture from passing vacuously.
    (root / "home").unlink()
    _load_molt_cache_prune()._prune(root, max_bytes=0, max_age_days=0, dry_run=False)
    assert not owned.exists()


@pytest.mark.parametrize("hit_after_inventory", [False, True])
def test_real_cache_hit_survives_cold_entry_and_stale_eviction_inventory(
    tmp_path: Path, monkeypatch, hit_after_inventory: bool
) -> None:
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    hot, cold = _cache_entry(cache, "aa-hot"), _cache_entry(cache, "bb-cold")
    old = time.time() - 86400
    for index, entry in enumerate((hot, cold)):
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)
        os.utime(entry.root, (old + index, old + index))
    content_before = {
        path: (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_ctime_ns)
        for path in (hot.artifact, hot.metadata)
    }
    inventory = module._collect_entries(cache)

    def hit():
        with _locked_wasm_link_cache_entry(hot):
            assert _read_wasm_link_cache_entry(hot).data == WASM_BYTES
        if not hit_after_inventory:
            refreshed = hot.root.stat().st_mtime_ns
            with _locked_wasm_link_cache_entry(hot):
                assert _read_wasm_link_cache_entry(hot).status == "hit"
            assert hot.root.stat().st_mtime_ns == refreshed, "no hot-read write churn"

    if hit_after_inventory:
        collect = module._collect_entries

        def stale_inventory(root, **kwargs):
            assert root == cache
            hit()
            return inventory

        monkeypatch.setattr(module, "_collect_entries", stale_inventory)
    else:
        hit()
    result = module._prune(
        cache,
        max_bytes=module._entry_size_bytes(hot.root),
        max_age_days=None,
        dry_run=False,
    )
    assert result["entries_removed"] == 1
    assert hot.artifact.read_bytes() == WASM_BYTES
    assert not cold.root.exists()
    assert result["bytes_remaining"] <= module._entry_size_bytes(hot.root)
    assert {
        path: (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_ctime_ns)
        for path in content_before
    } == content_before
    if hit_after_inventory:
        monkeypatch.setattr(module, "_collect_entries", collect)


def test_corrupt_cache_read_does_not_refresh_eviction_owner(tmp_path: Path) -> None:
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    entry = _cache_entry(cache, "bad")
    with _locked_wasm_link_cache_entry(entry):
        _publish_wasm_link_cache_entry(entry, WASM_BYTES)
    entry.artifact.write_bytes(WASM_BYTES + b"corruption")
    old = time.time() - 86400
    os.utime(entry.root, (old, old))
    before = entry.root.stat().st_mtime_ns
    with _locked_wasm_link_cache_entry(entry):
        assert _read_wasm_link_cache_entry(entry).status == "corrupt"
    assert entry.root.stat().st_mtime_ns == before
    result = module._prune(cache, max_bytes=0, max_age_days=None, dry_run=False)
    assert result["entries_removed"] == 1


def test_lockless_hit_never_refreshes_a_replaced_or_busy_entry(tmp_path: Path) -> None:
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    path = cache / "module_lowering" / "entry.json"
    path.parent.mkdir(parents=True)
    old = time.time() - 86400
    path.write_bytes(b"first")
    os.utime(path, (old, old))
    observed = path.stat()
    replacement = path.with_suffix(".tmp")
    replacement.write_bytes(b"replacement")
    os.utime(replacement, (old, old))
    replacement.replace(path)
    module.refresh_cache_entry_recency(path, cache_root=cache, observed=observed)
    assert path.stat().st_mtime == pytest.approx(old)
    observed = path.stat()
    lock = _acquire_file_lock(
        cache / "locks" / "prune.lock", timeout_s=1, timeout_message="test lock"
    )
    try:
        module.refresh_cache_entry_recency(path, cache_root=cache, observed=observed)
        assert path.stat().st_mtime_ns == observed.st_mtime_ns
    finally:
        _release_file_lock(lock)
    module.refresh_cache_entry_recency(path, cache_root=cache, observed=observed)
    assert path.stat().st_mtime_ns > observed.st_mtime_ns
    assert path.read_bytes() == b"replacement"


@pytest.mark.parametrize("directory", [False, True])
def test_recency_updates_only_admitted_entry_timestamp(
    tmp_path: Path, directory: bool
) -> None:
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    path = cache / "module_lowering" / "entry"
    path.parent.mkdir(parents=True)
    if directory:
        path.mkdir()
        payload = path / "payload"
    else:
        payload = path
    payload.write_bytes(b"preserve contents")
    old = time.time_ns() - 86400 * 1_000_000_000
    os.utime(path, ns=(old, old))
    observed = path.lstat()
    payload_before = payload.stat()
    module.refresh_cache_entry_recency(path, cache_root=cache, observed=observed)
    assert path.stat().st_mtime_ns > observed.st_mtime_ns
    assert path.stat().st_atime_ns == observed.st_atime_ns
    assert payload.read_bytes() == b"preserve contents"
    if directory:
        assert payload.stat().st_mtime_ns == payload_before.st_mtime_ns


@pytest.mark.skipif(
    os.name == "nt", reason="Windows pins replacement by handle sharing"
)
def test_recency_update_pins_generation_across_path_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    path = cache / "module_lowering" / "entry"
    path.parent.mkdir(parents=True)
    path.write_bytes(b"admitted")
    replacement = path.with_suffix(".replacement")
    replacement.write_bytes(b"replacement")
    old = time.time_ns() - 86400 * 1_000_000_000
    os.utime(path, ns=(old, old))
    os.utime(replacement, ns=(old, old))
    observed = path.stat()
    replacement_before = replacement.stat()
    native_utime = os.utime

    def replace_before_timestamp(target, *args, **kwargs):
        replacement.replace(path)
        return native_utime(target, *args, **kwargs)

    install_module_view(monkeypatch, "os", os, module, utime=replace_before_timestamp)
    module.refresh_cache_entry_recency(path, cache_root=cache, observed=observed)
    assert path.read_bytes() == b"replacement"
    assert path.stat().st_mtime_ns == replacement_before.st_mtime_ns


@pytest.mark.skipif(os.name != "nt", reason="Windows directory reparse-point boundary")
def test_recency_never_refreshes_a_junction_target(tmp_path: Path) -> None:
    from tests.process_guard_common import run_guarded_test_process

    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    cache.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    path = cache / "junction"
    run_guarded_test_process(
        ["cmd", "/c", "mklink", "/J", str(path), str(outside)],
        prefix="MOLT_TEST_CACHE_RECENCY",
        check=True,
        capture_output=True,
    )
    old = time.time_ns() - 86400 * 1_000_000_000
    os.utime(outside, ns=(old, old))
    before = outside.stat()
    # Even a stale caller observation of the previous direct directory cannot
    # turn the no-follow handle into authority over the junction's target.
    module.refresh_cache_entry_recency(path, observed=before)
    assert outside.stat().st_mtime_ns == before.st_mtime_ns


def test_global_budget_combines_all_source_record_kinds_and_file_entries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    module = _load_molt_cache_prune()
    cache = tmp_path / "cache"
    store = records.source_record_store(cache / SHARED_SOURCE_RECORDS_DIRECTORY)
    clock = [100 * 86400 * 1_000_000_000]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    kinds: tuple[records.RecordKind, ...] = (
        "import-scan",
        "intrinsic-facts",
        "module-analysis",
        "module-lowering",
    )
    for kind, size in zip(kinds, (40, 30, 20, 10), strict=True):
        assert store.put(kind, "shared-key", b"x" * size, generation="g")
        clock[0] += 1_000_000_000
    entry = _cache_entry(cache, "old-file")
    with _locked_wasm_link_cache_entry(entry):
        _publish_wasm_link_cache_entry(entry, WASM_BYTES)
    file_time = 99 * 86400
    os.utime(entry.root, (file_time, file_time))
    physical_file_bytes = sum(
        path.stat().st_size for path in entry.root.rglob("*") if path.is_file()
    )
    database_identity = store.path.stat().st_ino

    result = module._prune(cache, max_bytes=60, max_age_days=None, dry_run=False)

    assert result["entries_before"] == 5
    assert result["entries_removed"] == 2
    assert result["entries_failed"] == 0
    assert result["bytes_before"] == physical_file_bytes + 100
    assert result["bytes_remaining"] == 60
    assert result["bytes_removed"] == physical_file_bytes + 40
    assert result["source_records_before"] == 4
    assert result["source_records_removed"] == 1
    assert result["source_record_payload_bytes_removed"] == 40
    assert result["source_record_physical_before"] is not None
    assert result["source_record_physical_after"] is not None
    assert not result["source_record_maintenance_failed"]
    assert store.path.stat().st_ino == database_identity
    assert not entry.root.exists()
    for kind in kinds:
        observed = store.get(kind, "shared-key", generation="g", max_bytes=None)
        assert (observed is None) == (kind == "import-scan")


def test_source_record_dry_run_and_age_policy_preserve_database_owner(
    tmp_path: Path,
) -> None:
    import sqlite3
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    store = records.source_record_store(cache / SHARED_SOURCE_RECORDS_DIRECTORY)
    assert store.put("module-lowering", "old", b"payload", generation="g")
    with sqlite3.connect(store.path) as connection:
        connection.execute(
            "UPDATE records SET used_ns = ?",
            (time.time_ns() - 2 * 86400 * 1_000_000_000,),
        )
        before = connection.execute("SELECT * FROM records").fetchall()
    identity = store.path.stat().st_ino
    dry = _load_molt_cache_prune()._prune(
        cache, max_bytes=None, max_age_days=1, dry_run=True
    )
    assert dry["source_records_removed"] == 1
    with sqlite3.connect(store.path) as connection:
        assert connection.execute("SELECT * FROM records").fetchall() == before
    actual = _load_molt_cache_prune()._prune(
        cache, max_bytes=None, max_age_days=1, dry_run=False
    )
    assert actual["source_records_removed"] == 1
    assert store.path.stat().st_ino == identity
    assert store.get("module-lowering", "old", generation="g", max_bytes=None) is None


@pytest.mark.parametrize("change", ["refresh", "replace"])
@pytest.mark.parametrize("future_timestamp", [False, True])
def test_source_record_prune_cannot_delete_a_changed_observation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str, future_timestamp: bool
) -> None:
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    store = records.source_record_store(cache / SHARED_SOURCE_RECORDS_DIRECTORY)
    clock = [100 * 86400 * 1_000_000_000]
    install_module_view(monkeypatch, "time", time, records, time_ns=lambda: clock[0])
    assert store.put("module-analysis", "k", b"observed", generation="g")
    candidate = store.get("module-analysis", "k", generation="g", max_bytes=None)
    assert candidate is not None
    if future_timestamp:
        clock[0] += 100 * 86400 * 1_000_000_000
        assert store.put("intrinsic-facts", "future", b"future", generation="g")
        clock[0] -= 100 * 86400 * 1_000_000_000
    remove = records.SourceRecordMaintenance.remove
    injected = False

    def racing_remove(maintenance, observed):
        nonlocal injected
        if not any(row.key == "k" for row in observed):
            return remove(maintenance, observed)
        assert not injected, "a refreshed/replaced row must not reenter the sweep"
        injected = True
        clock[0] += 301 * 1_000_000_000
        if change == "refresh":
            assert candidate.refresh()
        else:
            assert store.put("module-analysis", "k", b"replacement", generation="g")
        return remove(maintenance, observed)

    monkeypatch.setattr(records.SourceRecordMaintenance, "remove", racing_remove)
    result = _load_molt_cache_prune()._prune(
        cache, max_bytes=0, max_age_days=None, dry_run=False
    )
    assert result["source_records_removed"] == int(future_timestamp)
    assert result["entries_busy"] == 1
    assert result["entries_failed"] == 0
    remaining = store.get("module-analysis", "k", generation="g", max_bytes=None)
    assert remaining is not None
    assert remaining.payload == (b"replacement" if change == "replace" else b"observed")


def test_corrupt_source_database_is_reported_and_never_unlinked(tmp_path: Path) -> None:
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    root = tmp_path / "cache"
    database = root / SHARED_SOURCE_RECORDS_DIRECTORY / records.STORE_NAME
    database.parent.mkdir(parents=True)
    database.write_bytes(b"corrupt optional records")
    before = (database.stat().st_ino, database.read_bytes())
    result = _load_molt_cache_prune()._prune(
        root, max_bytes=0, max_age_days=0, dry_run=False
    )
    assert result["source_record_maintenance_failed"]
    assert result["entries_failed"] == 1
    assert result["entries_removed"] == 0
    assert (database.stat().st_ino, database.read_bytes()) == before


def test_source_record_busy_migration_is_reported_separately(tmp_path: Path) -> None:
    import sqlite3
    from molt.cli.source_record_store import STORE_NAME
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    record_root = cache / SHARED_SOURCE_RECORDS_DIRECTORY
    record_root.mkdir(parents=True)
    database = record_root / STORE_NAME
    connection = sqlite3.connect(database, isolation_level=None)
    try:
        connection.execute("PRAGMA auto_vacuum=INCREMENTAL")
        connection.execute("PRAGMA journal_mode=WAL")
        # An actual historical store awaiting the metadata migration. A peer
        # writer owns its transaction; the nonwaiting pruner must defer it.
        connection.executescript("""
            CREATE TABLE records(kind TEXT,key TEXT,generation TEXT,payload BLOB,
                PRIMARY KEY(kind,key)) WITHOUT ROWID;
            CREATE INDEX records_generation ON records(generation);
            CREATE TABLE generations(generation TEXT PRIMARY KEY,used_ns INTEGER) WITHOUT ROWID;
            CREATE TABLE trims(singleton INTEGER PRIMARY KEY,trimmed_ns INTEGER);
            INSERT INTO records VALUES('module-lowering','old','g',X'616263');
        """)
        identity = database.stat().st_ino
        connection.execute("BEGIN IMMEDIATE")
        result = _load_molt_cache_prune()._prune(
            cache, max_bytes=0, max_age_days=None, dry_run=False
        )
        assert result["source_record_maintenance_busy"] is True
        assert result["source_record_maintenance_failed"] is False
        assert result["entries_failed"] == result["entries_removed"] == 0
        assert connection.execute("SELECT payload FROM records").fetchall() == [
            (b"abc",)
        ]
        assert database.stat().st_ino == identity
        connection.execute("COMMIT")
        completed = _load_molt_cache_prune()._prune(
            cache, max_bytes=0, max_age_days=None, dry_run=False
        )
        assert completed["source_record_maintenance_busy"] is False
        assert completed["entries_failed"] == 0
        assert completed["source_records_removed"] == 1
        assert connection.execute("SELECT payload FROM records").fetchall() == []
        assert database.stat().st_ino == identity
    finally:
        connection.close()


def test_dry_run_reports_an_unmigrated_store_without_failing(tmp_path: Path) -> None:
    # A dry run opens the store read-only, so it cannot migrate a historical
    # schema; that is an unmigrated store to report, not a failed entry. The
    # writing sweep migrates it and manages its rows.
    import sqlite3
    from molt.cli.source_record_store import STORE_NAME
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    record_root = cache / SHARED_SOURCE_RECORDS_DIRECTORY
    record_root.mkdir(parents=True)
    connection = sqlite3.connect(record_root / STORE_NAME, isolation_level=None)
    try:
        connection.executescript("""
            CREATE TABLE records(kind TEXT,key TEXT,generation TEXT,payload BLOB,
                PRIMARY KEY(kind,key)) WITHOUT ROWID;
            CREATE TABLE generations(generation TEXT PRIMARY KEY,used_ns INTEGER) WITHOUT ROWID;
            CREATE TABLE trims(singleton INTEGER PRIMARY KEY,trimmed_ns INTEGER);
            INSERT INTO records VALUES('module-lowering','old','g',X'616263');
        """)
    finally:
        connection.close()
    module = _load_molt_cache_prune()

    dry = module._prune(cache, max_bytes=0, max_age_days=None, dry_run=True)
    assert dry["source_record_store_unmigrated"] is True
    assert dry["source_record_maintenance_failed"] is False
    assert dry["entries_failed"] == 0
    assert "source_records.unmigrated=True" in module.prune_report_lines(
        cache, dry, max_gb=0, max_age_days=30, dry_run=True
    )

    swept = module._prune(cache, max_bytes=0, max_age_days=None, dry_run=False)
    assert swept["source_record_store_unmigrated"] is False
    assert swept["entries_failed"] == 0 and swept["source_records_removed"] == 1


def test_row_batches_commit_before_interleaved_file_eviction(tmp_path, monkeypatch):
    import sqlite3
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    store = records.source_record_store(cache / SHARED_SOURCE_RECORDS_DIRECTORY)
    for key in ("first", "second", "third"):
        assert store.put("module-lowering", key, key.encode(), generation="g")
    peer = sqlite3.connect(store.path, isolation_level=None)
    try:
        for key, stamp in (("first", 10), ("second", 30), ("third", 50)):
            peer.execute(
                "UPDATE records SET used_ns=? WHERE key=?", (stamp * 10**9, key)
            )
        entry = _cache_entry(cache, "middle-file")
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)
        os.utime(entry.root, (20, 20))
        module = _load_molt_cache_prune()
        remove_file = module._remove_entry
        observed = []

        def observe_file(candidate, dry_run):
            assert candidate.path == entry.root
            # This real competing writer must acquire immediately. The earlier
            # row is committed and later rows have not been reordered past us.
            peer.execute("BEGIN IMMEDIATE")
            try:
                remaining = peer.execute(
                    "SELECT key FROM records ORDER BY key"
                ).fetchall()
                assert remaining == [("second",), ("third",)]
                observed.append(remaining)
            finally:
                peer.execute("ROLLBACK")
            return remove_file(candidate, dry_run)

        monkeypatch.setattr(module, "_remove_entry", observe_file)
        result = module._prune(cache, max_bytes=None, max_age_days=0, dry_run=False)
        assert result["entries_removed"] == 4 and result["entries_failed"] == 0
        assert result["source_records_removed"] == 3
        assert len(observed) == 1 and not entry.root.exists()
        assert peer.execute("SELECT key FROM records").fetchall() == []
    finally:
        peer.close()


def test_later_zero_delete_sweep_continues_bounded_physical_reclamation(tmp_path):
    import sqlite3
    from molt.cli import source_record_store as records
    from molt.default_paths import SHARED_SOURCE_RECORDS_DIRECTORY

    cache = tmp_path / "cache"
    store = records.source_record_store(cache / SHARED_SOURCE_RECORDS_DIRECTORY)
    for index in range(256):
        assert store.put("module-lowering", str(index), b"x" * 8192, generation="g")
    peer = sqlite3.connect(store.path)
    try:

        def pages():
            cursor = peer.execute("PRAGMA page_count")
            try:
                return cursor.fetchone()[0]
            finally:
                cursor.close()

        before = pages()
        first = _load_molt_cache_prune()._prune(
            cache, max_bytes=0, max_age_days=None, dry_run=False
        )
        middle = pages()
        assert first["source_records_removed"] == 256 and first["entries_failed"] == 0
        assert 0 < before - middle <= 256
        second = _load_molt_cache_prune()._prune(
            cache, max_bytes=0, max_age_days=None, dry_run=False
        )
        after = pages()
        assert second["source_records_removed"] == 0 and second["entries_failed"] == 0
        assert 0 < middle - after <= 256
        assert peer.execute("SELECT count(*) FROM records").fetchone() == (0,)
    finally:
        peer.close()
