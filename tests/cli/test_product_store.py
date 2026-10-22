"""The host's product store: placement, variants, retention and single flight."""

from __future__ import annotations

import json
import os
from pathlib import Path
import threading
import time

import pytest

from molt.cli import product_store as products
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.dx import PRODUCT_STORE_DIRNAME, product_store_dir
from molt.process_environment import ProcessEnvironment


def _environment(value: str) -> CargoEnvironmentInputs:
    return CargoEnvironmentInputs.capture(
        ProcessEnvironment({"MOLT_TEST_INPUT": value}),
        ("MOLT_TEST_INPUT",),
        local_unit_count=1,
    )


def _digest(seed: str) -> str:
    return (seed * 64)[:64]


def test_every_checkout_and_session_target_of_one_root_shares_one_store(
    tmp_path: Path,
) -> None:
    root = tmp_path / "artifact-root"
    root.mkdir()
    base = {"MOLT_EXT_ROOT": str(root), "MOLT_PREFER_EXTERNAL_ARTIFACTS": "1"}
    shared = root / "target" / PRODUCT_STORE_DIRNAME
    for project in (tmp_path / "lane-a", tmp_path / "lane-b"):
        assert product_store_dir(project, base) == shared
        for target in (
            root / "target" / "checkouts" / "lane-a-0123456789ab",
            root / "target" / "checkouts" / "lane-a-0123456789ab" / "sessions" / "s",
        ):
            env = {**base, "CARGO_TARGET_DIR": str(target)}
            assert product_store_dir(project, env) == shared


def test_an_operator_owned_target_keeps_its_own_store(tmp_path: Path) -> None:
    root = tmp_path / "artifact-root"
    root.mkdir()
    elsewhere = tmp_path / "isolated-target"
    env = {
        "MOLT_EXT_ROOT": str(root),
        "MOLT_PREFER_EXTERNAL_ARTIFACTS": "1",
        "CARGO_TARGET_DIR": str(elsewhere),
    }
    assert product_store_dir(tmp_path / "lane", env) == (
        elsewhere / PRODUCT_STORE_DIRNAME
    )


def test_a_plain_clone_keeps_its_store_in_its_own_target(
    tmp_path: Path, developer_host_context: None
) -> None:
    project = tmp_path / "clone"
    assert product_store_dir(project, {}) == project / "target" / PRODUCT_STORE_DIRNAME


def test_store_kinds_are_closed_and_identities_are_digests(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="unknown product kind"):
        products.ProductStore.of("runtime", tmp_path, {})  # type: ignore[arg-type]
    store = products.ProductStore(tmp_path / "store")
    for bad in ("", "../escape", "A" * 64, "a" * 63):
        with pytest.raises(ValueError, match="SHA-256"):
            store.entry(bad)


def test_variants_are_named_by_their_cargo_environment(tmp_path: Path) -> None:
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    first, second = _environment("one"), _environment("two")
    assert store.variant(identity, first) == store.entry(identity) / first.digest
    assert store.variant(identity, first) != store.variant(identity, second)
    assert store.variants(identity) == ()
    for environment in (second, first):
        store.variant(identity, environment).mkdir(parents=True)
    # Foreign names, files and links are never variants.
    (store.entry(identity) / "not-a-digest").mkdir()
    (store.entry(identity) / ("f" * 64)).write_text("file", encoding="utf-8")
    (store.entry(identity) / ("e" * 64)).symlink_to(store.variant(identity, first))
    assert store.variants(identity) == tuple(
        sorted((store.variant(identity, first), store.variant(identity, second)))
    )


def _ledger(store: products.ProductStore) -> dict:
    return json.loads((store.root / ".product-store.json").read_text("utf-8"))


def test_retention_evicts_only_entries_no_build_used_for_the_window(
    tmp_path: Path,
) -> None:
    store = products.ProductStore(tmp_path / "store")
    idle, used, recent = _digest("a"), _digest("b"), _digest("c")
    for identity in (idle, used, recent):
        (store.variant(identity, _environment("x"))).mkdir(parents=True)
        store.record_use(identity)
    ledger = _ledger(store)
    now = time.time()
    ledger["last_used"][idle] = now - products.PRODUCT_STORE_IDLE_SECONDS - 60
    ledger["last_used"][recent] = now - products.PRODUCT_STORE_IDLE_SECONDS + 3600
    (store.root / ".product-store.json").write_text(json.dumps(ledger), "utf-8")
    # An eviction interrupted after its move leaves an aside entry behind.
    (store.root / f".evicted-{_digest('d')}.123").mkdir()

    assert store.retain(used) == (idle,)

    assert not store.entry(idle).exists()
    assert store.entry(used).is_dir() and store.entry(recent).is_dir()
    assert sorted(path.name for path in store.root.iterdir()) == sorted(
        [used, recent, ".product-store.json", ".product-store.lock"]
    )
    assert set(_ledger(store)["last_used"]) == {used, recent}


def test_an_entry_a_builder_holds_is_renewed_before_its_lock(tmp_path: Path) -> None:
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    with store.single_flight(identity):
        assert _ledger(store)["last_used"][identity] >= time.time() - 60
        # The lock lives outside the entry, so eviction never moves it.
        assert store.build_lock(identity).is_file()
        assert not store.build_lock(identity).is_relative_to(store.entry(identity))


def test_one_builder_per_identity_and_the_other_reuses_its_publication(
    tmp_path: Path,
) -> None:
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    published = store.entry(identity) / "published"
    builds: list[str] = []
    entered = threading.Barrier(2)
    errors: list[BaseException] = []

    def builder(name: str) -> None:
        try:
            entered.wait(timeout=10)
            with store.single_flight(identity):
                # The store is read again under the lock, as the build does.
                if published.exists():
                    return
                builds.append(name)
                time.sleep(0.2)
                published.write_text(name, encoding="utf-8")
        except BaseException as exc:  # noqa: BLE001 - reported below
            errors.append(exc)

    threads = [threading.Thread(target=builder, args=(name,)) for name in "ab"]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=30)
    assert not errors
    assert len(builds) == 1
    assert published.read_text(encoding="utf-8") == builds[0]


def test_an_operator_bound_fails_a_waiter_with_a_build_lock_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli.build_locks import BuildLockAcquisitionError

    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.2")
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    held = threading.Event()
    release = threading.Event()

    def holder() -> None:
        with store.single_flight(identity):
            held.set()
            release.wait(timeout=10)

    thread = threading.Thread(target=holder)
    thread.start()
    try:
        assert held.wait(timeout=10)
        with pytest.raises(BuildLockAcquisitionError, match="Timed out after 0.2s"):
            with store.single_flight(identity):
                pass
    finally:
        release.set()
        thread.join(timeout=10)


def test_eviction_moves_an_entry_aside_before_deleting_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / ("a" * 64)
    (entry / "variant").mkdir(parents=True)
    seen: list[Path] = []
    monkeypatch.setattr(
        products, "delete_path", lambda path: (seen.append(path), (True, ""))[1]
    )
    products._remove_entry(entry)
    assert not entry.exists()
    assert len(seen) == 1 and seen[0].name.startswith(".evicted-" + "a" * 64)
    assert seen[0].parent == tmp_path and seen[0].is_dir()
    assert os.path.exists(seen[0] / "variant")


class _Clock:
    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now


def test_a_use_inside_the_renewal_interval_takes_no_lock_and_writes_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Go renews a cache entry at most hourly; a warm build here writes nothing."""
    from molt.cli import generation_store

    clock = _Clock(1_000_000.0)
    monkeypatch.setattr(generation_store, "time", clock)
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    for name in (identity, _digest("b")):
        store.entry(name).mkdir(parents=True)
    assert store.record_use(identity)
    ledger = store.root / ".product-store.json"
    recorded = ledger.read_bytes()
    assert _ledger(store)["last_used"][identity] == clock.now
    locks: list[Path] = []
    acquire = generation_store._acquire_file_lock

    def counted(path, **kwargs):
        locks.append(path)
        return acquire(path, **kwargs)

    monkeypatch.setattr(generation_store, "_acquire_file_lock", counted)
    clock.now += products.PRODUCT_STORE_RENEWAL_SECONDS
    assert store.record_use(identity)
    assert locks == [] and ledger.read_bytes() == recorded

    clock.now += 1
    assert store.record_use(identity)
    assert len(locks) == 1
    assert _ledger(store)["last_used"][identity] == clock.now
    # Another identity has no recorded use, so its first use is written.
    assert store.record_use(_digest("b"))
    assert len(locks) == 2
    assert _ledger(store)["last_used"][_digest("b")] == clock.now


def test_a_use_of_an_absent_entry_is_a_miss_that_writes_nothing(
    tmp_path: Path,
) -> None:
    """A build of new inputs reads the store before it builds (HF-390 review v2).

    The read renews nothing it cannot find and creates nothing, so the caller
    builds. An entry removed while its recent use is still recorded is a miss
    too, though that renewal takes no lock.
    """
    store = products.ProductStore(tmp_path / "store")
    assert not store.record_use(_digest("a"))
    assert not store.root.exists()

    store.entry(_digest("b")).mkdir(parents=True)
    assert store.record_use(_digest("b"))
    assert not store.record_use(_digest("a"))
    assert set(_ledger(store)["last_used"]) == {_digest("b")}
    assert sorted(path.name for path in store.root.iterdir()) == sorted(
        [_digest("b"), ".product-store.json", ".product-store.lock"]
    )

    store.entry(_digest("b")).rmdir()
    assert not store.record_use(_digest("b"))
    assert not store.entry(_digest("b")).exists()


def test_a_builder_creates_and_renews_its_entry_in_one_locked_step(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No eviction falls between a builder's entry and its renewal (review v2 m1).

    An idle entry whose builder arrives just as another publication evicts
    idle entries must still be there, renewed, while the builder holds it.
    """
    from contextlib import contextmanager

    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    store.entry(identity).mkdir(parents=True)
    assert store.record_use(identity)
    ledger = _ledger(store)
    ledger["last_used"][identity] = (
        time.time() - products.PRODUCT_STORE_IDLE_SECONDS - 60
    )
    (store.root / ".product-store.json").write_text(json.dumps(ledger), "utf-8")
    real = products.generation_store
    evictions: list[tuple[str, ...]] = []

    @contextmanager
    def evicting_first(root, layout):
        # A rival publication's eviction wins the ledger lock first.
        if not evictions:
            with real(root, layout) as rival:
                evictions.append(rival.evict_idle(products.PRODUCT_STORE_IDLE_SECONDS))
        with real(root, layout) as ledger:
            yield ledger

    monkeypatch.setattr(products, "generation_store", evicting_first)
    with store.single_flight(identity):
        assert store.entry(identity).is_dir()
        assert _ledger(store)["last_used"][identity] >= time.time() - 60
    assert evictions == [(identity,)]


def test_a_stored_file_maps_to_its_entry_and_no_other_file_does(
    tmp_path: Path,
) -> None:
    """A cached launch renews the entry that holds each file it runs.

    Only ``<.molt-products>/<layout>/<known kind>/<digest>`` names an entry; a
    file anywhere else (an installed toolchain's) is used as it is.
    """
    root = tmp_path / PRODUCT_STORE_DIRNAME / products.LAYOUT_VERSION
    store = products.ProductStore(root / "native-runtime")
    stored = store.entry(_digest("a")) / _digest("b") / ".gen" / "runtime.a"
    assert products.ProductStore.entry_of(stored) == (store, _digest("a"))
    for outside in (
        tmp_path / "runtime.a",
        tmp_path / "other" / products.LAYOUT_VERSION / "native-runtime" / _digest("a"),
        tmp_path / PRODUCT_STORE_DIRNAME / "v0" / "native-runtime" / _digest("a"),
        root / "unknown-kind" / _digest("a") / "runtime.a",
        root / "native-runtime" / "not-a-digest" / "runtime.a",
    ):
        assert products.ProductStore.entry_of(outside) is None, outside

    stored.parent.mkdir(parents=True)
    stored.write_bytes(b"runtime")
    assert products.record_stored_file_use(stored)
    assert _digest("a") in _ledger(store)["last_used"]
    installed = tmp_path / "installed" / "runtime.a"
    installed.parent.mkdir()
    installed.write_bytes(b"runtime")
    assert products.record_stored_file_use(installed)
    assert not products.record_stored_file_use(installed.with_name("missing.a"))
    assert store.evict(_digest("a"))
    assert not products.record_stored_file_use(stored)
    assert not store.entry(_digest("a")).exists()


def test_evict_waits_for_the_builder_and_removes_the_entry_whole(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The operator's tool for a corrupt entry keeps the store's invariants.

    It waits for a builder of the identity, moves the entry aside and deletes
    it under the ledger lock, and keeps the identity's lock, which a waiter
    may hold.
    """
    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    store = products.ProductStore(tmp_path / "store")
    identity, other = _digest("a"), _digest("b")
    for name in (identity, other):
        variant = store.variant(name, _environment("x"))
        variant.mkdir(parents=True)
        (variant / "receipt").write_text("corrupt", encoding="utf-8")
        store.record_use(name)
    held, release = threading.Event(), threading.Event()
    released_at: list[float] = []

    def builder() -> None:
        with store.single_flight(identity):
            held.set()
            release.wait(timeout=10)
            released_at.append(time.monotonic())

    thread = threading.Thread(target=builder)
    thread.start()
    assert held.wait(timeout=10)
    threading.Timer(0.3, release.set).start()
    try:
        assert store.evict(identity)
        evicted_at = time.monotonic()
    finally:
        release.set()
        thread.join(timeout=10)
    assert released_at and evicted_at >= released_at[0]
    assert not store.entry(identity).exists()
    assert store.entry(other).is_dir()
    assert not list(store.root.glob(".evicted-*"))
    assert set(_ledger(store)["last_used"]) == {other}
    assert store.build_lock(identity).is_file()
    # Nothing left to evict, and an absent store is not created.
    assert not store.evict(identity)
    absent = products.ProductStore(tmp_path / "absent")
    assert not absent.evict(identity) and not absent.root.exists()


def test_the_cache_evict_command_removes_the_entry_a_build_named(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import sys

    from molt.cli import entrypoint
    from molt.source_root import compiler_source_root

    store = products.ProductStore.of("host-tool", compiler_source_root())
    identity = _digest("c")
    store.variant(identity, _environment("x")).mkdir(parents=True)
    error = products.ProductStoreError(store, identity, "a fixture corruption")
    assert f"`molt cache evict host-tool {identity}`" in str(error)
    for evicted in (True, False):
        monkeypatch.setattr(
            sys, "argv", ["molt", "cache", "evict", "host-tool", identity, "--json"]
        )
        assert entrypoint.main() == 0
        payload = json.loads(capsys.readouterr().out)
        assert payload["command"] == "cache evict" and payload["status"] == "ok"
        assert payload["data"] == {
            "entry": str(store.entry(identity)),
            "evicted": evicted,
        }
    assert not store.entry(identity).exists()
    monkeypatch.setattr(
        sys, "argv", ["molt", "cache", "evict", "host-tool", "not-a-digest", "--json"]
    )
    assert entrypoint.main() != 0
    assert "SHA-256" in capsys.readouterr().out


def test_an_interrupted_eviction_is_swept_under_the_ledger_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Every removal runs under the ledger lock, so a sweep never races one."""
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    store.variant(identity, _environment("x")).mkdir(parents=True)
    aside = store.root / f".evicted-{_digest('d')}.123"
    aside.mkdir()
    held_during_sweep: list[bool] = []
    delete = products.delete_path

    def observed(path: Path):
        probe = _try_acquire_file_lock(store.root / ".product-store.lock")
        held_during_sweep.append(probe is None)
        if probe is not None:
            _release_file_lock(probe)
        return delete(path)

    monkeypatch.setattr(products, "delete_path", observed)
    store.retain(identity)
    assert held_during_sweep == [True]
    assert not aside.exists()


_BUILDER_SCRIPT = """
import sys, time
from pathlib import Path
from molt.cli.product_store import ProductStore

root, identity, go = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3])
store = ProductStore(root)
deadline = time.monotonic() + 30
while not go.exists() and time.monotonic() < deadline:
    time.sleep(0.005)
with store.single_flight(identity):
    published = store.entry(identity) / "published"
    if not published.exists():
        with open(root / "builds.log", "a", encoding="utf-8") as log:
            log.write(f"{sys.argv[4]}\\n")
        time.sleep(0.3)
        published.write_text(sys.argv[4], encoding="utf-8")
print("done", flush=True)
"""


def test_two_processes_with_one_identity_build_it_once(tmp_path: Path) -> None:
    """Single flight across processes, as two checkouts run it: the OS lock.

    Threads in one process meet the in-process mutex first; two interpreters
    share only the OS file lock, so this proves what two checkouts rely on.
    """
    import subprocess
    import sys

    from tests.process_guard_common import (
        close_owned_test_process,
        start_owned_test_process,
    )

    store = products.ProductStore(tmp_path / "store")
    store.root.mkdir(parents=True)
    identity, go = _digest("a"), tmp_path / "go"
    builders = [
        start_owned_test_process(
            [
                sys.executable,
                "-c",
                _BUILDER_SCRIPT,
                str(store.root),
                identity,
                str(go),
                name,
            ],
            stdout=subprocess.PIPE,
            text=True,
        )
        for name in ("first", "second")
    ]
    try:
        go.touch()
        for builder in builders:
            assert builder.wait(timeout=60) == 0
            assert builder.stdout is not None
            assert builder.stdout.read().strip() == "done"
    finally:
        for builder in builders:
            close_owned_test_process(builder)
    builds = (store.root / "builds.log").read_text(encoding="utf-8").split()
    assert len(builds) == 1
    assert (store.entry(identity) / "published").read_text(encoding="utf-8") == (
        builds[0]
    )
