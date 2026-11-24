"""Import facts survive unrelated churn in their directories; changed facts do not."""

from __future__ import annotations

import itertools
import os
from pathlib import Path

import pytest

from molt.cli import python_import_topology
from molt.cli.python_import_topology import (
    PythonImportTopology,
    PythonImportTopologyChangedError,
)


def _churn_during(
    monkeypatch: pytest.MonkeyPatch, name: str, directory: Path, times: int | None
) -> None:
    """Add an unrelated non-module file to ``directory`` during each call."""
    original = getattr(python_import_topology, name)
    counter = itertools.count()

    def changed_during(
        path: Path, **kwargs: bool | python_import_topology._DirectoryState | None
    ) -> tuple[os.DirEntry[str], ...] | python_import_topology._ProbeObservation:
        index = next(counter)
        if times is None or index < times:
            (directory / f"unrelated-{index}.txt").write_text("", encoding="utf-8")
        return original(path, **kwargs)

    monkeypatch.setattr(python_import_topology, name, changed_during)


def test_unrelated_entries_during_a_listing_change_no_import_fact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A shared temporary or project directory gains entries at any moment;
    # the listing read under that churn must still yield the import facts.
    (tmp_path / "mod.py").write_text("", encoding="utf-8")
    _churn_during(monkeypatch, "_read_entries", tmp_path, times=3)
    topology = PythonImportTopology()
    assert topology.contains_exact(tmp_path, "mod.py", "file")
    assert not topology.contains_exact(tmp_path, "pkg", "directory")
    assert topology.source_child_names(tmp_path) == {"mod"}
    topology.validate()


def test_unrelated_entries_during_a_probe_change_no_import_fact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "mod.py").write_text("", encoding="utf-8")
    _churn_during(monkeypatch, "_probe", tmp_path, times=3)
    topology = PythonImportTopology()
    assert topology.is_file(tmp_path / "mod.py")
    assert not topology.is_dir(tmp_path / "absent")
    topology.validate()


def test_a_queried_name_that_appears_ends_the_operation(tmp_path: Path) -> None:
    topology = PythonImportTopology()
    assert not topology.contains_exact(tmp_path, "late.py", "file")
    assert topology.source_child_names(tmp_path) == frozenset()
    (tmp_path / "late.py").write_text("", encoding="utf-8")
    with pytest.raises(
        PythonImportTopologyChangedError, match="topology changed during operation"
    ):
        topology.validate()


def test_a_directory_that_never_holds_still_fails_with_its_own_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _churn_during(monkeypatch, "_read_entries", tmp_path, times=None)
    topology = PythonImportTopology()
    with pytest.raises(PythonImportTopologyChangedError, match="never held still"):
        topology.contains_exact(tmp_path, "mod.py", "file")
    # The failure ends the operation for every later query.
    with pytest.raises(PythonImportTopologyChangedError, match="never held still"):
        topology.validate()


@pytest.mark.parametrize("created_name", ["a.py", "unrelated.txt"])
def test_recorded_sibling_probes_distinguish_relevant_churn_during_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, created_name: str
) -> None:
    first = tmp_path / "a.py"
    second = tmp_path / "b.py"
    second.write_text("pass\n", encoding="utf-8")
    topology = PythonImportTopology()
    assert not topology.is_file(first)
    assert topology.is_file(second)
    record = topology.record()
    probe = python_import_topology._probe
    added = tmp_path / created_name

    def add_sibling(
        path: Path, *, parent: python_import_topology._DirectoryState | None
    ) -> python_import_topology._ProbeObservation:
        if path == second and not added.exists():
            added.write_text("pass\n", encoding="utf-8")
        return probe(path, parent=parent)

    monkeypatch.setattr(python_import_topology, "_probe", add_sibling)
    if added == first:
        with pytest.raises(PythonImportTopologyChangedError, match=r"a\.py"):
            PythonImportTopology.from_record(record)
    else:
        assert PythonImportTopology.from_record(record).record() == record


@pytest.mark.skipif(os.name == "nt", reason="Windows uses full path resolution")
def test_replay_resolves_a_direct_child_through_its_fenced_parent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "module.py"
    source.write_text("pass\n", encoding="utf-8")
    owner = PythonImportTopology()
    positive = owner.probe(source)
    assert positive is not None
    assert positive.path == source.resolve(strict=True)
    assert owner.probe(tmp_path / "missing.py") is None
    record = owner.record()

    resolve = Path.resolve

    def guard_leaf(path: Path, strict: bool = False) -> Path:
        assert path != source, "direct child repeated its parent's resolution"
        return resolve(path, strict=strict)

    monkeypatch.setattr(Path, "resolve", guard_leaf)
    assert PythonImportTopology.from_record(record).record() == record


def test_dangling_leaf_link_revalidates_when_its_target_appears(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    imports = tmp_path / "imports"
    imports.mkdir()
    target = tmp_path / "target.py"
    link = imports / "alias.py"
    try:
        link.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"symlinks unavailable: {exc}")
    monkeypatch.setattr(python_import_topology, "DIGEST_RACY_WINDOW_NS", 0)
    owner = PythonImportTopology()
    assert owner.probe(link) is None
    record = owner.record()
    before = imports.stat()
    target.write_text("pass\n", encoding="utf-8")
    assert imports.stat().st_mtime_ns == before.st_mtime_ns
    assert link.resolve(strict=True) == target.resolve(strict=True)
    with pytest.raises(PythonImportTopologyChangedError, match=r"alias\.py"):
        owner.validate()
    with pytest.raises(PythonImportTopologyChangedError, match=r"alias\.py"):
        PythonImportTopology.from_record(record)
