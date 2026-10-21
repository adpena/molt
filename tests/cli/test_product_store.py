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
    with store.single_flight(identity, default_timeout_s=5.0):
        assert _ledger(store)["last_used"][identity] >= time.time() - 60
        assert (store.entry(identity) / ".build.lock").is_file()


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
            with store.single_flight(identity, default_timeout_s=30.0):
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


def test_a_held_build_lock_bounds_the_wait(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli.build_locks import BuildLockAcquisitionError

    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    store = products.ProductStore(tmp_path / "store")
    identity = _digest("a")
    held = threading.Event()
    release = threading.Event()

    def holder() -> None:
        with store.single_flight(identity, default_timeout_s=5.0):
            held.set()
            release.wait(timeout=10)

    thread = threading.Thread(target=holder)
    thread.start()
    try:
        assert held.wait(timeout=10)
        with pytest.raises(BuildLockAcquisitionError, match="another build"):
            with store.single_flight(identity, default_timeout_s=0.2):
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
    store.record_use(identity)
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
    store.record_use(identity)
    assert locks == [] and ledger.read_bytes() == recorded

    clock.now += 1
    store.record_use(identity)
    assert len(locks) == 1
    assert _ledger(store)["last_used"][identity] == clock.now
    # Another identity has no recorded use, so its first use is written.
    store.record_use(_digest("b"))
    assert len(locks) == 2
    assert _ledger(store)["last_used"][_digest("b")] == clock.now


def test_the_renewal_interval_is_far_inside_the_idle_window() -> None:
    """A skipped renewal still leaves the entry its idle window less an hour."""
    assert (
        products.PRODUCT_STORE_RENEWAL_SECONDS * 24
        < products.PRODUCT_STORE_IDLE_SECONDS
    )
