"""The host's store of Rust build products (design 56, section 2.8).

Every checkout used to select its runtime archives and compiler executables
inside its own Cargo target, so a new lane, the landing sandbox, a pinned
session and every agent worktree rebuilt products that another checkout had
already built from identical inputs (a fresh lane's first ``molt build
examples/hello.py`` took 330.8 s). Go keeps one build cache per user that every
checkout shares, and Bazel's disk cache serves every workspace of a project;
this store is that cache for Molt's Rust products. ``molt.dx.product_store_dir``
places it beside the checkout targets of one artifact root.

- An entry holds the products of one input identity: the product's existing
  identity digest, which names sources by content and logical root and never
  by checkout path. A store miss builds once and publishes; every checkout
  with the same identity then reuses the entry without running Cargo.
- Inside an entry, a variant holds what one observed Cargo environment
  (``CargoEnvironmentInputs``) produced, because a variable a build script
  reads can change the bytes without changing the declared identity. A
  reader admits the variant whose recorded environment matches its own.
- Entries are immutable once published. A reader sees an entry whole or not
  at all: publication completes a variant before its selection receipt
  appears, and eviction moves an entry aside before deleting it. So a
  receipt that exists but does not verify is corruption, and it fails loud.
  The only later writes are content-addressed derivations of a runtime
  archive (its callable-symbol projection and custody extraction), published
  exclusively beside its generation, and a host tool's probe evidence, a
  cached check rewritten under the entry's build lock; none recreates a
  removed generation.
- One builder per identity: ``single_flight`` holds the entry's lock across
  the build, so concurrent checkouts with the same inputs build once.
- Retention follows last use, as Go's trim does: each admission renews its
  entry in the store's ledger at most once per
  ``PRODUCT_STORE_RENEWAL_SECONDS``, and each publication evicts the entries
  no build used for ``PRODUCT_STORE_IDLE_SECONDS``. An admission of an absent
  entry is a miss and writes nothing. A renewal inside the
  interval reads the ledger without the lock and writes nothing, so a warm
  build takes no store lock and makes no write.
"""

from __future__ import annotations

from collections.abc import Generator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
import os
from pathlib import Path
import re

from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.product_kinds import PRODUCT_KINDS, ProductKind
from molt.cli.generation_store import (
    GenerationStoreLayout,
    generation_store,
    used_within,
)
from molt.dx import product_store_dir
from molt import input_report
from molt.file_deletion import delete_path

LAYOUT_VERSION = "v1"
# Go's build cache trims entries no build used for five days
# (cmd/go/internal/cache, trimLimit); a publication applies the same window.
PRODUCT_STORE_IDLE_SECONDS = 5 * 24 * 60 * 60.0
# Go renews an entry's mtime at most once an hour (cmd/go/internal/cache,
# mtimeInterval): a renewal inside the hour cannot change what the window
# decides, so it costs no lock and no write.
PRODUCT_STORE_RENEWAL_SECONDS = 60 * 60.0
_DIGEST = re.compile(r"[0-9a-f]{64}")
_ASIDE_PREFIX = ".evicted-"
# Identity locks live outside the entries they guard and are never deleted, as
# Cargo keeps its lock files: eviction moves an entry aside while a waiter may
# hold the lock's inode, and a lock inside the entry would move with it.
_LOCKS = ".locks"


class ProductStoreError(ValueError):
    """A published product store entry exists but does not verify.

    The message names the tool that removes the entry (``molt cache evict``),
    which keeps the store's invariant that readers see an entry whole or not
    at all; removing it by hand under live readers does not.
    """

    def __init__(
        self,
        store: ProductStore,
        identity: str,
        reason: str,
        *,
        variant: Path | None = None,
    ) -> None:
        entry = store.entry(identity)
        super().__init__(
            f"product store entry {variant or entry} is corrupt: {reason}. It is "
            f"regenerable: `molt cache evict {store.kind} {identity}` removes it "
            "and the next build rebuilds it"
        )
        self.entry = entry
        self.kind = store.kind
        self.identity = identity


def _remove_entry(path: Path) -> None:
    """Move one idle entry aside, then delete it; readers never see it partial."""
    aside = path.with_name(f"{_ASIDE_PREFIX}{path.name}.{os.getpid()}")
    os.replace(path, aside)
    removed, error = delete_path(aside)
    if not removed:
        raise OSError(error)


_LEDGER = GenerationStoreLayout(
    label="product",
    lock_name=".product-store.lock",
    ledger_name=".product-store.json",
    ledger_schema="molt.product-store.v1",
    entry=re.compile(r"^[0-9a-f]{64}$"),
    remove=_remove_entry,
)


@dataclass(frozen=True, slots=True)
class ProductStore:
    """The entries of one product kind under one store directory."""

    root: Path

    @classmethod
    def of(
        cls,
        kind: ProductKind,
        project_root: Path,
        env: Mapping[str, str] | None = None,
    ) -> ProductStore:
        if kind not in PRODUCT_KINDS:
            raise ValueError(f"unknown product kind: {kind!r}")
        store = product_store_dir(project_root, os.environ if env is None else env)
        return cls(store / LAYOUT_VERSION / kind)

    @classmethod
    def entry_of(cls, path: Path, store_dir: Path) -> tuple[ProductStore, str] | None:
        """The store and identity of the entry under ``store_dir`` (a
        ``product_store_dir``) that holds ``path``; None when no entry does.

        Both are resolved, so a store reached through a symbolic link maps
        the same entry as its target (review v3, A-m2).
        """
        try:
            parts = path.resolve().relative_to(store_dir.resolve()).parts
        except (OSError, ValueError):
            return None
        if (
            len(parts) < 3
            or parts[0] != LAYOUT_VERSION
            or parts[1] not in PRODUCT_KINDS
            or not _DIGEST.fullmatch(parts[2])
        ):
            return None
        return cls(store_dir.resolve() / parts[0] / parts[1]), parts[2]

    @property
    def kind(self) -> str:
        """The product kind this store holds (its directory's name)."""
        return self.root.name

    def entry(self, identity: str) -> Path:
        if not _DIGEST.fullmatch(identity):
            raise ValueError(f"product identity must be a SHA-256 digest: {identity!r}")
        return self.root / identity

    def build_lock(self, identity: str) -> Path:
        """The lock one builder of ``identity`` holds (``single_flight``)."""
        self.entry(identity)
        return self.root / _LOCKS / f"{identity}.lock"

    def variant(self, identity: str, environment: CargoEnvironmentInputs) -> Path:
        """The variant of ``identity`` built under ``environment``."""
        return self.entry(identity) / environment.digest

    def variants(self, identity: str) -> tuple[Path, ...]:
        """The published variants of ``identity``, in name order."""
        try:
            children = sorted(self.entry(identity).iterdir())
        except FileNotFoundError:
            return ()
        return tuple(
            child
            for child in children
            if _DIGEST.fullmatch(child.name)
            and child.is_dir()
            and not child.is_symlink()
        )

    def _report_build_use(self, identity: str) -> None:
        if not input_report.result_active():
            return
        from molt.build_record import BuildClaimKind, RecordedBuildClaim

        try:
            input_report.report_build_claim(
                RecordedBuildClaim.capture(
                    BuildClaimKind.PRODUCT,
                    self.root,
                    product_identity=identity,
                ).payload()
            )
        except (OSError, ValueError) as exc:
            input_report.refuse(f"product use cannot be recorded: {exc}")

    def record_use(self, identity: str) -> bool:
        """Renew ``identity`` before reading it; False when the store lacks it.

        The check and the renewal hold the ledger lock, where eviction runs, so
        an entry renewed here stays for the whole idle window and no eviction
        races the read. An absent entry is a miss: nothing is created or
        renewed, and the caller builds. A use recorded within the renewal
        interval already keeps the entry for the whole idle window less that
        interval, far longer than any read, so it is left as it is.
        """
        entry = self.entry(identity)
        if used_within(self.root, _LEDGER, identity, PRODUCT_STORE_RENEWAL_SECONDS):
            present = entry.is_dir()
            if present:
                self._report_build_use(identity)
            return present
        if not entry.is_dir():
            return False
        try:
            with generation_store(self.root, _LEDGER) as ledger:
                if not entry.is_dir():
                    return False
                ledger.use(identity)
        except RuntimeError as exc:  # the ledger lock timed out
            raise OSError(str(exc)) from exc
        self._report_build_use(identity)
        return True

    def retain(self, identity: str) -> tuple[str, ...]:
        """Renew ``identity`` and evict the entries no build used for the window."""
        self.entry(identity)
        try:
            with generation_store(self.root, _LEDGER) as ledger:
                ledger.use(identity)
                evicted = ledger.evict_idle(PRODUCT_STORE_IDLE_SECONDS)
                # An eviction interrupted after its move left these behind.
                # Every removal runs under the ledger lock, so none of them
                # is another process's removal still in progress.
                for aside in self.root.glob(f"{_ASIDE_PREFIX}*"):
                    delete_path(aside)
        except RuntimeError as exc:  # the ledger lock timed out
            raise OSError(str(exc)) from exc
        self._report_build_use(identity)
        return evicted

    @contextmanager
    def single_flight(self, identity: str) -> Generator[None]:
        """Hold ``identity``'s build lock while building it.

        A second checkout with the same inputs waits here, then finds the
        entry the first one published. It waits by the one build-lock policy
        (``molt.file_locks.lock_wait_s``): while the holder builds, as Cargo's
        build-directory lock and Bazel's output-base lock wait, with one
        notice naming the holder. The lock is an OS file lock, so a dead
        holder never blocks a waiter.
        """
        from molt.cli.build_locks import BuildLockAcquisitionError

        entry = self.entry(identity)
        with self._held(identity):
            try:
                # Created and renewed under the build lock, in one step under
                # the ledger lock, where eviction runs: an entry a builder
                # holds is never idle, and `molt cache evict`, which takes the
                # build lock first, cannot remove it between this step and the
                # build (review v3, A-m1).
                self.root.mkdir(parents=True, exist_ok=True)
                with generation_store(self.root, _LEDGER) as ledger:
                    entry.mkdir(exist_ok=True)
                    ledger.use(identity)
            except (OSError, RuntimeError) as exc:
                raise BuildLockAcquisitionError(str(exc)) from exc
            yield

    @contextmanager
    def _held(self, identity: str) -> Generator[None]:
        from molt.cli.build_locks import held_build_lock

        with held_build_lock(
            self.build_lock(identity),
            purpose=f"product store ({self.kind} {identity[:12]})",
        ):
            yield

    def evict(self, identity: str) -> bool:
        """Remove ``identity``'s entry now: the operator's tool for a corrupt one.

        It waits for the identity's builder, so no publication is in flight,
        then moves the entry aside and deletes it under the ledger lock, as an
        idle eviction does: a reader sees the entry whole or not at all, and
        the next build of those inputs rebuilds it. It returns False when the
        store holds no such entry, and creates nothing then.
        """
        entry = self.entry(identity)
        if not entry.exists():
            return False
        with self._held(identity):
            try:
                with generation_store(self.root, _LEDGER) as ledger:
                    if not entry.exists():
                        return False
                    _remove_entry(entry)
                    ledger.last_used.pop(identity, None)
            except RuntimeError as exc:  # the ledger lock timed out
                raise OSError(str(exc)) from exc
        return True


def record_stored_file_use(path: Path, store_dir: Path) -> bool:
    """Renew the entry under ``store_dir`` that holds ``path``; False is a miss.

    A cached launch runs files a build selected without admitting them again:
    renewing their entries under the ledger lock keeps eviction from removing
    them during the run. A file that eviction removed, or that lies in no
    entry of this store, is a miss, so the caller builds: renewing nothing
    would let the next publication evict what the launch runs.
    """
    located = ProductStore.entry_of(path, store_dir)
    if located is None:
        return False
    store, identity = located
    return store.record_use(identity) and path.is_file()
