"""The perf methodology lock: its content, its import boundary, its decisions."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
for _root in (REPO_ROOT / "tools", REPO_ROOT / "src"):
    if str(_root) not in sys.path:
        sys.path.insert(0, str(_root))

import perf_methodology as pm  # noqa: E402
from molt.cli.python_import_resolution import (  # noqa: E402
    LocalPythonModuleResolver,
    PythonImportPolicy,
    local_import_targets,
)


def test_methodology_content_matches_its_locked_version() -> None:
    methodology = pm.load()
    assert pm.content_sha256(methodology) == methodology.locked_sha256, (
        "the perf methodology files changed: run "
        "`python3 tools/perf_methodology.py --bump` if the change can move a "
        "measured number or a verdict, otherwise `--relock`"
    )


def _direct_local_imports(path: Path) -> set[str]:
    resolver = LocalPythonModuleResolver(
        (REPO_ROOT / "tools", REPO_ROOT / "src", REPO_ROOT),
        source_roots=(REPO_ROOT / "tools", REPO_ROOT / "src"),
    )
    policy = PythonImportPolicy(
        module_level_only=False,
        include_parent_packages=False,
        fail_on_nonliteral_dynamic_import=True,
        purpose="source_dependency",
    )
    imported: set[str] = set()
    for module in local_import_targets(path, resolver, policy):
        source = resolver.source_for_module(module)
        if source is not None:
            imported.add(source.relative_to(REPO_ROOT).as_posix())
    return imported


def test_methodology_imports_only_methodology_or_declared_mechanics() -> None:
    methodology = pm.load()
    declared = set(methodology.paths) | set(methodology.mechanics)
    undecided = {
        path: sorted(_direct_local_imports(REPO_ROOT / path) - declared)
        for path in methodology.paths
        if path.endswith(".py")
    }
    assert not {path: rest for path, rest in undecided.items() if rest}, (
        "declare each import in config/perf_methodology.toml as methodology "
        "(it can move a number or a verdict) or mechanics (with the reason it "
        "cannot)"
    )


def test_every_declared_path_exists_and_every_mechanics_entry_is_used() -> None:
    methodology = pm.load()
    for path in (*methodology.paths, *methodology.mechanics):
        assert (REPO_ROOT / path).is_file(), path
    imported = set().union(
        *(
            _direct_local_imports(REPO_ROOT / path)
            for path in methodology.paths
            if path.endswith(".py")
        )
    )
    assert sorted(set(methodology.mechanics) - imported) == []


def test_content_digest_covers_every_methodology_byte(tmp_path: Path) -> None:
    methodology = pm.load()
    for path in methodology.paths:
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REPO_ROOT / path, target)
    original = pm.content_sha256(methodology, root=tmp_path)
    assert original == pm.content_sha256(methodology)
    changed = tmp_path / methodology.paths[0]
    changed.write_bytes(changed.read_bytes() + b"\n")
    assert pm.content_sha256(methodology, root=tmp_path) != original


def test_bump_and_relock_rewrite_only_the_lock(tmp_path: Path) -> None:
    authority = tmp_path / "perf_methodology.toml"
    shutil.copyfile(pm.AUTHORITY, authority)
    before = pm.load(authority)
    pm._rewrite_lock(authority, version=before.version + 1, digest="e" * 64)
    after = pm.load(authority)
    assert (after.version, after.locked_sha256) == (before.version + 1, "e" * 64)
    assert (after.paths, dict(after.mechanics)) == (
        before.paths,
        dict(before.mechanics),
    )
