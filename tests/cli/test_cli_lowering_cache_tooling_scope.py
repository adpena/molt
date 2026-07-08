"""Scoped frontend-tooling fingerprint: unrelated CLI/runtime/link edits must
not cold-start the persisted per-module frontend caches.

The persisted analysis / lowering / import-graph caches key on a *frontend
tooling* fingerprint. Historically that fingerprint hashed the entire
``src/molt/cli`` tree, so touching an unrelated backend/link/daemon/cargo file
(which provably cannot change what a module lowers to) invalidated the lowering
cache and forced a needless cold re-lower of every witness module.

These tests pin the fix: the lowering-scoped fingerprint
(:func:`_frontend_semantic_tooling_fingerprint`) is *invariant* under an edit to
a post-lowering / orthogonal CLI module, while it still changes for any edit that
could genuinely affect lowering (a ``frontend/`` file or a non-orthogonal CLI
file). The broad :func:`_cache_tooling_fingerprint` -- the input the caches used
before the fix -- is asserted to *still change* on the unrelated edit, so the
tests have teeth: without the scoping fix the cache keys would move.
"""

from __future__ import annotations

import importlib
from pathlib import Path

import pytest

CF = importlib.import_module("molt.cli.cache_fingerprints")
MC = importlib.import_module("molt.cli.module_cache")


_AUX_FILES = CF._FRONTEND_AUX_SOURCE_RELPATHS
# One provably post-lowering CLI module (in the orthogonal denylist) and one
# frontend-driver CLI module that must keep invalidating the caches.
_ORTHOGONAL_CLI = "link_pipeline.py"
_NON_ORTHOGONAL_CLI = "frontend_pipeline.py"


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _build_fake_molt_tree(root: Path) -> Path:
    """Minimal ``src/molt`` tree exercising both scoped and broad fingerprints."""
    molt = root / "src" / "molt"
    cli = molt / "cli"
    # A representative slice of cli/*.py: orthogonal (denylisted) files plus
    # frontend-relevant ones. The broad fingerprint hashes the whole cli dir;
    # the scoped fingerprint drops the orthogonal names.
    _write(cli / _ORTHOGONAL_CLI, "# post-lowering linking\nMARKER = 1\n")
    _write(cli / "backend_execution.py", "# post-lowering backend\nMARKER = 1\n")
    _write(cli / "native_toolchain.py", "# toolchain provisioning\nMARKER = 1\n")
    _write(cli / _NON_ORTHOGONAL_CLI, "# frontend driver\nMARKER = 1\n")
    _write(cli / "module_cache.py", "# per-module cache authority\nMARKER = 1\n")
    _write(cli / "module_resolution.py", "# import resolution\nMARKER = 1\n")
    # frontend/ tree: subpackages import each other, so it is kept whole.
    _write(molt / "frontend" / "__init__.py", "MARKER = 1\n")
    _write(molt / "frontend" / "lowering" / "core.py", "MARKER = 1\n")
    _write(molt / "frontend" / "sema" / "__init__.py", "MARKER = 1\n")
    _write(molt / "frontend" / "visitors" / "expressions.py", "MARKER = 1\n")
    for rel in _AUX_FILES:
        _write(molt / rel, "MARKER = 1\n")
    return molt


@pytest.fixture
def fake_tree(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    molt = _build_fake_molt_tree(tmp_path)
    monkeypatch.setattr(CF, "_compiler_root", lambda: tmp_path)
    # The broad path helper is lru-cached per root; a fresh tmp_path is a new key
    # so it never serves a stale dir list. Clear the content-digest memo so each
    # recompute reads the freshly edited bytes.
    CF._frontend_tooling_source_paths_cached.cache_clear()
    CF._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    return molt


def _recompute() -> tuple[str, str]:
    CF._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    return (
        CF._frontend_semantic_tooling_fingerprint(),
        CF._cache_tooling_fingerprint(),
    )


def test_orthogonal_cli_edit_does_not_change_scoped_fingerprint(
    fake_tree: Path,
) -> None:
    scoped_before, broad_before = _recompute()

    # Unrelated edit: a post-lowering / linking CLI module.
    (fake_tree / "cli" / _ORTHOGONAL_CLI).write_text(
        "# post-lowering linking (edited)\nMARKER = 2\n",
        encoding="utf-8",
    )
    scoped_after, broad_after = _recompute()

    # The fix: the lowering-scoped fingerprint is unchanged -> the persisted
    # lowering result is reused, not cold-started.
    assert scoped_after == scoped_before
    # Teeth: the broad fingerprint -- what the caches keyed on before the fix --
    # *does* change, proving the unrelated edit used to invalidate the cache.
    assert broad_after != broad_before


def test_frontend_edit_still_changes_scoped_fingerprint(fake_tree: Path) -> None:
    scoped_before, _ = _recompute()

    # A genuine lowering-relevant edit inside frontend/.
    (fake_tree / "frontend" / "lowering" / "core.py").write_text(
        "MARKER = 2\n", encoding="utf-8"
    )
    scoped_after, _ = _recompute()

    assert scoped_after != scoped_before


def test_non_orthogonal_cli_edit_still_changes_scoped_fingerprint(
    fake_tree: Path,
) -> None:
    scoped_before, _ = _recompute()

    # A non-orthogonal CLI module (a frontend driver) must still invalidate.
    (fake_tree / "cli" / _NON_ORTHOGONAL_CLI).write_text(
        "# frontend driver (edited)\nMARKER = 2\n", encoding="utf-8"
    )
    scoped_after, _ = _recompute()

    assert scoped_after != scoped_before


def test_analysis_and_lowering_cache_keys_ignore_orthogonal_cli_edit(
    fake_tree: Path,
) -> None:
    """The real per-module cache key (module_cache authority) is wired to the
    scoped fingerprint: an orthogonal CLI edit leaves it stable.

    This fails if the wiring reverts to the broad fingerprint (which moves on the
    same edit, per the teeth assertion below), so it guards the fix end-to-end.
    """
    module_path = fake_tree / "frontend" / "lowering" / "core.py"

    def analysis_key() -> str:
        CF._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
        return MC._module_analysis_cache_key(
            module_path,
            kind="module_analysis_cache",
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="full",
            target_python=MC._DEFAULT_TARGET_PYTHON_VERSION,
            capability_config_digest="",
        )

    key_before = analysis_key()
    broad_before = CF._cache_tooling_fingerprint()

    (fake_tree / "cli" / _ORTHOGONAL_CLI).write_text(
        "# post-lowering linking (edited)\nMARKER = 9\n",
        encoding="utf-8",
    )

    key_after = analysis_key()
    CF._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    broad_after = CF._cache_tooling_fingerprint()

    assert key_after == key_before
    # Teeth: had the key kept using the broad fingerprint it would have moved.
    assert broad_after != broad_before


def test_orthogonal_denylist_is_real_and_disjoint_from_frontend_drivers() -> None:
    """Every denylisted basename is a real cli module, and none of them are
    frontend-compute drivers -- guards against a rename silently under-scoping or
    a frontend file being mis-denylisted (which would serve a stale lowering)."""
    cli_dir = CF._compiler_root() / "src" / "molt" / "cli"
    real_cli = {path.name for path in cli_dir.glob("*.py")}
    denylist = CF._POST_LOWERING_ORTHOGONAL_CLI_BASENAMES

    missing = sorted(denylist - real_cli)
    assert not missing, f"orthogonal denylist names not present in cli/: {missing}"

    frontend_drivers = {
        "frontend_execution.py",
        "frontend_pipeline.py",
        "frontend_parallel.py",
        "frontend_worker.py",
        "frontend_integration.py",
        "module_cache.py",
        "module_graph.py",
        "module_graph_cache.py",
        "module_graph_discovery.py",
        "module_import_scanner.py",
        "module_resolution.py",
        "module_source.py",
        "module_dependencies.py",
        "module_registry.py",
        "module_stdlib_policy.py",
        "target_python.py",
        "cache_fingerprints.py",
        "models.py",
    }
    overlap = sorted(denylist & frontend_drivers)
    assert not overlap, f"frontend drivers wrongly marked orthogonal: {overlap}"


def test_scoped_fingerprint_differs_from_broad_on_real_tree() -> None:
    """On the real source tree the scoped and broad fingerprints are distinct --
    the scope genuinely drops the orthogonal cli files (and uses a separate scope
    tag), so wiring a cache to one vs the other is observable."""
    assert (
        CF._frontend_semantic_tooling_fingerprint() != CF._cache_tooling_fingerprint()
    )


def test_admission_policy_files_stay_in_scope() -> None:
    """external_native.py MUST remain in the lowering fingerprint: frontend_pipeline
    imports its `_resolve_import_admission_policy`, which feeds `direct_call_modules`
    (a lowering-affecting input). Excluding it would risk STALE lowering. This guards
    the correctness carve-out from silently reopening."""
    from pathlib import Path

    from molt.cli import cache_fingerprints as cf

    root = Path(cf.__file__).resolve().parents[3]
    names = {p.name for p in cf._frontend_semantic_tooling_source_paths(root)}
    assert "external_native.py" in names, (
        "external_native.py fell out of the lowering fingerprint scope -> stale-lowering risk"
    )
    # sanity: the genuinely post-lowering files stay excluded (perf win intact)
    assert "link_pipeline.py" not in names
    assert "backend_cache.py" not in names
