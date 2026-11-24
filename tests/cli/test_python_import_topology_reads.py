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
    PythonImportPath,
)


def _churn_during(
    monkeypatch: pytest.MonkeyPatch, name: str, directory: Path, times: int | None
) -> None:
    """Add an unrelated non-module file to ``directory`` during each call."""
    original = getattr(python_import_topology, name)
    counter = itertools.count()

    def changed_during(
        path: Path, **kwargs: bool
    ) -> tuple[os.DirEntry[str], ...] | PythonImportPath | None:
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

    def add_sibling(path: Path) -> PythonImportPath | None:
        if path == second and not added.exists():
            added.write_text("pass\n", encoding="utf-8")
        return probe(path)

    monkeypatch.setattr(python_import_topology, "_probe", add_sibling)
    if added == first:
        with pytest.raises(PythonImportTopologyChangedError, match=r"a\.py"):
            PythonImportTopology.from_record(record)
    else:
        assert PythonImportTopology.from_record(record).record() == record
