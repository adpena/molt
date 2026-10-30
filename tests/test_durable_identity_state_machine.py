"""Bounded event sequences against the identity and publication transition models.

Native stat/volume/statx answers are the substituted inputs. Files, versioned
records, locks, publication moves and both scratch readers remain real.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import ctypes
from dataclasses import dataclass
import json
import os
from pathlib import Path
import stat
import struct
import sys
import tempfile
from threading import Event
from types import SimpleNamespace

from hypothesis import settings, strategies as st
from hypothesis.stateful import RuleBasedStateMachine, invariant, rule
import pytest

from molt import artifact_publication as publication
from molt import durable_file_identity as durable
from molt import dx
from molt import temporary_artifacts as scratch
from tests.process_guard_common import install_module_view
from tools.import_file import load_module_from_path


def _files(root: Path, *, exclude: Path | None = None) -> dict[Path, bytes]:
    return {
        path: path.read_bytes()
        for path in root.rglob("*")
        if path.is_file() and (exclude is None or exclude not in path.parents)
    }


@dataclass(frozen=True)
class _JournalObservation:
    path: Path
    final: Path
    state: publication.JournalState
    original: bytes


class DurableIdentityMachine(RuleBasedStateMachine):
    def __init__(self):
        super().__init__()
        self.temporary = tempfile.TemporaryDirectory(prefix="molt-identity-model-")
        self.root = Path(self.temporary.name).resolve()
        self.bound_root = self.root / "bound"
        self.bound_root.mkdir()
        self.record = self.root / "family" / "artifact-root.v2.json"
        self.device = 16777240
        self.inode = 5983296
        self.volume = "a" * 32
        self.bound_volume = self.volume
        self.serial = 0
        self.mount = 101
        self.volatile_root = self.root / "volatile"
        self.volatile_root.mkdir()
        self.volatile_binding = dx._ArtifactRootBinding(
            self.volatile_root, self._volatile_identity(self.mount)
        )
        with self._persistent_authority() as patch:
            patch.setattr(dx, "artifact_root_binding_path", lambda root: self.record)
            dx.set_artifact_root_binding(
                self.root, self.bound_root, {"MOLT_EXTERNAL_MIN_FREE_GB": "0"}
            )
        loaded = dx._load_artifact_root_binding(self.record)
        assert isinstance(loaded, dx._ArtifactRootBinding)
        self.binding = loaded
        self.binding_bytes = self.record.read_bytes()
        self.legacy_binding = self.record.with_name("artifact-root.json")
        self.legacy_binding.write_text(
            json.dumps(
                {
                    "schema": "molt.artifact-root-binding.v1",
                    "root": str(self.bound_root),
                    "device": 16777240,
                    "inode": 5983296,
                }
            ),
            encoding="utf-8",
        )
        self.foreign = {self.legacy_binding: self.legacy_binding.read_bytes()}
        self.v2_files: dict[Path, bytes] = {}
        self.journals: list[_JournalObservation] = []
        self.old = load_module_from_path(
            "durable_identity_v1_oracle",
            Path(__file__).parent / "fixtures" / "guard_scratch_v1.py",
        )

    def teardown(self):
        self.temporary.cleanup()
        super().teardown()

    @contextmanager
    def _persistent_authority(self):
        """Replay stat allocation while keeping one explicit volume authority."""
        real_stat, real_lstat, relative_stat = Path.stat, Path.lstat, os.stat
        actual = real_stat(self.bound_root)

        def replay_metadata(metadata):
            if (metadata.st_dev, metadata.st_ino) != (actual.st_dev, actual.st_ino):
                return metadata
            values = list(metadata)
            values[stat.ST_DEV], values[stat.ST_INO] = self.device, self.inode
            extra = {
                name: getattr(metadata, name)
                for name in (
                    "st_atime_ns",
                    "st_mtime_ns",
                    "st_ctime_ns",
                    "st_birthtime",
                    "st_birthtime_ns",
                    "st_file_attributes",
                    "st_reparse_tag",
                )
                if hasattr(metadata, name)
            }
            return os.stat_result(values, extra)

        def replay(query):
            def read(path, *args, **kwargs):
                return replay_metadata(query(path, *args, **kwargs))

            return read

        def volume(fd):
            assert isinstance(fd, int), "the volume query must use the held parent"
            return durable._AppleVolume(42, self.volume)

        def leaf(fd, name):
            metadata = relative_stat(name, dir_fd=fd, follow_symlinks=False)
            return durable._AppleLeaf(42, replay_metadata(metadata).st_ino)

        with pytest.MonkeyPatch.context() as patch:
            # Real held-parent descriptors and relative lookup exercise the
            # protocol; only the native stat allocation/volume inputs vary.
            install_module_view(patch, "sys", sys, durable, platform="darwin")
            install_module_view(
                patch,
                "os",
                os,
                durable,
                stat=replay(relative_stat),
                O_SEARCH=getattr(
                    os, "O_SEARCH", os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
                ),
            )
            patch.setattr(durable, "_apple_volume", volume)
            patch.setattr(durable, "_apple_leaf", leaf)
            patch.setattr(Path, "stat", replay(real_stat))
            patch.setattr(Path, "lstat", replay(real_lstat))
            yield patch

    def _volatile_identity(self, mount):
        class StatxQuery:
            def __call__(self, fd, path, flags, requested, output):
                assert (path, flags, requested) == (b"", 0x1000, 0x4000)
                data = bytearray(256)
                # Linux v6.8 UAPI offsets, independent of the ctypes fields.
                struct.pack_into("=I", data, 0, 0x4000)
                struct.pack_into("=Q", data, 0x90, mount)
                ctypes.memmove(output, bytes(data), len(data))
                return 0

        metadata = self.volatile_root.stat()
        read_text = Path.read_text
        with pytest.MonkeyPatch.context() as patch:
            install_module_view(patch, "sys", sys, durable, platform="linux")
            install_module_view(
                patch,
                "ctypes",
                ctypes,
                durable,
                CDLL=lambda *args, **kwargs: SimpleNamespace(statx=StatxQuery()),
            )
            patch.setattr(durable, "_linux_filesystem_type", lambda fd: 0x01021994)
            patch.setattr(
                Path,
                "read_text",
                lambda path, *args, **kwargs: (
                    "12345678-1234-4567-8123-123456789abc"
                    if str(path) == "/proc/sys/kernel/random/boot_id"
                    else read_text(path, *args, **kwargs)
                ),
            )
            # Device and file ID remain identical across the simulated remount.
            return durable.identity_from_fd(-1, metadata=metadata)

    def _check_persistent_binding(self):
        same = self.volume == self.bound_volume
        event = (
            dx.ArtifactBindingEvent.IDENTITY_MATCHES
            if same
            else dx.ArtifactBindingEvent.IDENTITY_DIFFERS
        )
        expected = dx.ARTIFACT_BINDING_TRANSITIONS[dx.ArtifactBindingState.BOUND, event]
        with self._persistent_authority():
            assert (self.bound_root.stat().st_dev, self.bound_root.stat().st_ino) == (
                self.device,
                5983296,
            )
            if expected is dx.ArtifactBindingState.REFUSED:
                with pytest.raises(dx.DxConfigError, match="artifact-root --bind"):
                    dx._validate_artifact_binding(
                        self.binding, {"MOLT_EXTERNAL_MIN_FREE_GB": "0"}
                    )
            else:
                assert (
                    dx._validate_artifact_binding(
                        self.binding, {"MOLT_EXTERNAL_MIN_FREE_GB": "0"}
                    )
                    is expected
                )

    @rule()
    def renumbering_at_reboot(self):
        self.device = 16777246 if self.device == 16777240 else 16777240
        self._check_persistent_binding()

    @rule()
    def volume_substitution(self):
        self.serial += 1
        self.volume = f"{self.serial:032x}"
        self._check_persistent_binding()

    @rule(increment=st.integers(min_value=1, max_value=4096))
    def tmpfs_remount(self, increment):
        self.mount += increment

    def _check_volatile_binding(self):
        same = self.mount == 101
        expected = dx.ARTIFACT_BINDING_TRANSITIONS[
            dx.ArtifactBindingState.BOUND,
            dx.ArtifactBindingEvent.IDENTITY_MATCHES
            if same
            else dx.ArtifactBindingEvent.IDENTITY_DIFFERS,
        ]
        with pytest.MonkeyPatch.context() as patch:
            patch.setattr(
                dx,
                "file_identity",
                lambda path, *, expected=None: self._volatile_identity(self.mount),
            )
            if expected is dx.ArtifactBindingState.REFUSED:
                with pytest.raises(dx.DxConfigError, match="artifact-root --bind"):
                    dx._validate_artifact_binding(
                        self.volatile_binding, {"MOLT_EXTERNAL_MIN_FREE_GB": "0"}
                    )
            else:
                assert (
                    dx._validate_artifact_binding(
                        self.volatile_binding, {"MOLT_EXTERNAL_MIN_FREE_GB": "0"}
                    )
                    is expected
                )

    def _scratch_env(self, number):
        state = self.root / "host" / "tmp" / "memory_guard"
        return {
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
            "MOLT_MEMORY_GUARD_TOKEN": f"{number:032x}",
            "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / f"{number}.json"),
            "MOLT_SCRATCH_BUDGET_GB": "0.001",
        }

    def _finish_scratch(self, module, lease):
        return module.finish_guard_scratch(
            lease,
            closed=True,
            success=False,
            evidence={"authority": "model-closed-child", "child_returncode": 1},
            timings=module.ScratchFinalizationTimings(),
            retention=module.ScratchRetention(count=1000, bytes=1024**2),
        )

    @rule()
    def concurrent_v1_writer(self):
        self.serial += 2
        ready, finish = Event(), Event()
        old_env, new_env = (
            self._scratch_env(self.serial),
            self._scratch_env(self.serial + 1),
        )

        def write_old():
            lease = self.old.acquire_guard_scratch(self.root, old_env)
            try:
                (lease.target / "legacy").write_bytes(b"v1 payload")
                ready.set()
                assert finish.wait(10), (
                    "current writer failed to release the old writer"
                )
                outcome = self._finish_scratch(self.old, lease)
                assert outcome["retention"]["errors"] == []
                return lease.generation.parent
            finally:
                lease.release()

        with ThreadPoolExecutor(max_workers=1) as executor:
            old_future = executor.submit(write_old)
            old_future.add_done_callback(lambda _: ready.set())
            try:
                assert ready.wait(10), (
                    "older writer did not publish its live allocation"
                )
                if old_future.done():
                    old_future.result()
                lease = scratch.acquire_guard_scratch(self.root, new_env)
                try:
                    (lease.target / "current").write_bytes(b"v2 payload")
                    outcome = self._finish_scratch(scratch, lease)
                    assert outcome["retention"]["errors"] == []
                    current_root = lease.generation.parent
                    before = _files(current_root)
                finally:
                    lease.release()
            finally:
                finish.set()
            legacy_root = old_future.result(timeout=10)
        # The actual v1 producer's final sweep ran after v2 became pending.
        assert _files(current_root) == before
        foreign = _files(legacy_root, exclude=current_root)
        outcome = scratch.reclaim_terminal_scratch(
            current_root, retention=scratch.ScratchRetention(count=1000, bytes=1024**2)
        )
        assert outcome["errors"] == []
        assert _files(legacy_root, exclude=current_root) == foreign
        self.foreign.update(foreign)
        self.v2_files = _files(current_root)

    @rule(installed=st.booleans(), unverifiable=st.booleans())
    def interrupted_publication(self, installed, unverifiable):
        self.serial += 1
        parent = self.root / f"publication-{self.serial}"
        parent.mkdir()
        final = parent / "output"
        final.write_bytes(b"old generation")
        staged = publication.staged_output_path(final)
        staged.write_bytes(b"new generation")
        backup = publication.staged_output_path(final, purpose="backup", suffix=".old")
        transaction = f"{self.serial:032x}"
        journal = parent / f".molt-artifact-publication-{transaction}.json"

        def old_pair(path):
            metadata = path.stat()
            return [metadata.st_dev + int(unverifiable), metadata.st_ino]

        payload = {
            "schema": "molt.artifact-publication.v2",
            "transaction_id": transaction,
            "state": "prepared",
            "journals": [str(journal)],
            "removals": [],
            "replacements": [
                {
                    "staged": str(staged),
                    "final": str(final),
                    "backup": str(backup),
                    "had_final": True,
                    "prior_identity": old_pair(final),
                    "installed_identity": old_pair(staged),
                }
            ],
        }
        journal.write_text(json.dumps(payload), encoding="utf-8")
        original = journal.read_bytes()
        final.replace(backup)
        if installed:
            staged.replace(final)
        if unverifiable:
            with pytest.raises(publication.JournalRecoveryRefused) as caught:
                with publication.publication_locks([final]):
                    pass
            expected = publication.JOURNAL_TRANSITIONS[
                publication.JournalState.OPEN, publication.JournalEvent.REFUSE
            ]
            assert caught.value.state is expected
            assert (
                caught.value.journal == journal and str(journal) in caught.value.remedy
            )
        else:
            observed = []
            recover = publication._recover_transaction

            def observe(paths):
                result = recover(paths)
                observed.append(result)
                return result

            with pytest.MonkeyPatch.context() as patch:
                patch.setattr(publication, "_recover_transaction", observe)
                with publication.publication_locks([final]):
                    assert final.read_bytes() == b"old generation"
            recovered = publication.JOURNAL_TRANSITIONS[
                publication.JournalState.OPEN, publication.JournalEvent.RECOVER_LEGACY
            ]
            expected = publication.JOURNAL_TRANSITIONS[
                recovered, publication.JournalEvent.ROLLBACK
            ]
            assert observed and observed[-1].state is expected
            assert observed[-1].retained == ()
        self.journals.append(_JournalObservation(journal, final, expected, original))

    @invariant()
    def no_substituted_volume_is_accepted(self):
        self._check_persistent_binding()
        self._check_volatile_binding()
        assert self.record.read_bytes() == self.binding_bytes

    @invariant()
    def another_versions_records_are_untouched(self):
        assert all(
            path.read_bytes() == content for path, content in self.foreign.items()
        )
        assert all(
            path.read_bytes() == content for path, content in self.v2_files.items()
        )

    @invariant()
    def no_older_journal_is_stranded(self):
        for observation in self.journals:
            if observation.state is publication.JournalState.REFUSED:
                assert observation.path.read_bytes() == observation.original
                with pytest.raises(publication.JournalRecoveryRefused) as caught:
                    with publication.publication_locks([observation.final]):
                        pass
                assert caught.value.journal == observation.path
                assert str(observation.path) in caught.value.remedy
            else:
                assert observation.state is publication.JournalState.ROLLED_BACK
                assert not observation.path.exists()
                assert not publication._journal_path(
                    observation.path.parent,
                    observation.path.name.removeprefix(
                        ".molt-artifact-publication-"
                    ).removesuffix(".json"),
                ).exists()
                assert observation.final.read_bytes() == b"old generation"


TestDurableIdentityModel = pytest.mark.skipif(
    os.stat not in os.supports_dir_fd,
    reason="the Darwin protocol model requires POSIX parent-relative descriptors; native Windows custody is a separate cell",
)(DurableIdentityMachine.TestCase)
TestDurableIdentityModel.settings = settings(
    max_examples=12,
    stateful_step_count=20,
    deadline=None,
)
