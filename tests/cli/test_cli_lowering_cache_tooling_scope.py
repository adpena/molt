"""Scoped frontend-tooling fingerprint: unrelated CLI/runtime/link edits must
not cold-start the persisted per-module frontend caches.

The persisted analysis / lowering / import-graph caches key on a *frontend
tooling* fingerprint. Historically that fingerprint hashed the entire
``src/molt/cli`` tree, so touching an unrelated backend/link/daemon/cargo file
(which provably cannot change what a module lowers to) invalidated the lowering
cache and forced a needless cold re-lower of every witness module.

These tests pin the fix: the lowering-scoped fingerprint
(:func:`_frontend_semantic_tooling_fingerprint`) is *invariant* under an edit to
a post-lowering CLI module, while it still changes for any edit that could
genuinely affect lowering (a ``frontend/`` file or a frontend-driver CLI file).
The scope is derived structurally by module-level import reachability from the
frontend/module drivers -- no hand-maintained denylist -- so a post-lowering file
is out of scope precisely because no lowering-reachable module imports it. The
broad :func:`_cache_tooling_fingerprint` -- the input the caches used before the
fix -- is asserted to *still change* on the unrelated edit, so the tests have
teeth: without the scoping the cache keys would move.
"""

from __future__ import annotations

import importlib
import os
from pathlib import Path

import pytest

CF = importlib.import_module("molt.cli.cache_fingerprints")
MC = importlib.import_module("molt.cli.module_cache")


_AUX_FILES = CF._FRONTEND_AUX_SOURCE_RELPATHS
# One provably post-lowering CLI module (not reachable from any lowering seed) and
# one frontend-driver CLI module (a ``frontend_*`` seed) that must keep
# invalidating the caches.
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
    # The broad path helper and the reachability closure are lru-cached per root; a
    # fresh tmp_path is a new key so neither serves a stale set. Clear the
    # content-digest memo so each recompute reads the freshly edited bytes.
    CF._frontend_tooling_source_paths_cached.cache_clear()
    CF._lowering_scope_source_files_cached.cache_clear()
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


def test_frontend_drivers_in_scope_and_post_lowering_excluded() -> None:
    """The structurally-derived scope includes every frontend/module driver and
    excludes the post-lowering backend/link/cargo/toolchain layer.

    This replaces the old hand-maintained orthogonal denylist: instead of asserting
    a curated list is disjoint from drivers, it asserts the reachability-derived
    source set (the real cache key input) has the two required properties. A
    frontend driver falling out of scope would serve a stale lowering; a backend
    file falling into scope would needlessly cold-start it.
    """
    root = CF._compiler_root()
    # Restrict to ``cli/`` basenames: other packages may legitimately reuse a
    # basename (e.g. ``compiler_analysis/backend_ir.py`` is frontend-analysis IR,
    # unrelated to the post-lowering ``cli/backend_ir.py``).
    scoped = {
        path.name
        for path in CF._frontend_semantic_tooling_source_paths(root)
        if path.parent.name == "cli"
    }

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
    missing_drivers = sorted(frontend_drivers - scoped)
    assert not missing_drivers, (
        f"frontend drivers fell out of the lowering scope: {missing_drivers}"
    )

    post_lowering = {
        "backend_cache.py",
        "backend_execution.py",
        "backend_pipeline.py",
        "backend_ir.py",
        "cargo_execution.py",
        "cargo_profiles.py",
        "link_pipeline.py",
        "mlir_backend.py",
        "native_binary.py",
        "native_link_command.py",
        "native_toolchain.py",
        "runtime_build.py",
        "wasm.py",
        "wasm_toolchain.py",
        "toolchain_validation.py",
        "setup_readiness.py",
    }
    leaked = sorted(post_lowering & scoped)
    assert not leaked, f"post-lowering files leaked into the lowering scope: {leaked}"


def test_reachability_follows_driver_imports_but_not_backend() -> None:
    """Reachability genuinely follows a driver's module-level import into a
    non-seed cli helper, while a file nothing imports stays out of scope.

    Builds a fake tree where a ``module_*`` seed imports a plain ``cli`` helper,
    which in turn imports another; both must be pulled into scope. An unrelated
    ``backend_*`` helper that no in-scope file imports must stay excluded. This
    exercises the import-following mechanism the denylist never had.
    """
    import importlib
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        molt = root / "src" / "molt"
        cli = molt / "cli"
        _write(molt / "frontend" / "__init__.py", "MARKER = 1\n")
        for rel in _AUX_FILES:
            _write(molt / rel, "MARKER = 1\n")
        # seed -> helper_a -> helper_b (all must be in scope)
        _write(
            cli / "module_resolution.py",
            "from molt.cli.helper_a import thing\nMARKER = 1\n",
        )
        _write(
            cli / "helper_a.py",
            "from molt.cli.helper_b import other\nMARKER = 1\n",
        )
        _write(cli / "helper_b.py", "other = 1\nthing = 1\n")
        # backend helper that nothing in scope imports
        _write(cli / "backend_orphan.py", "MARKER = 1\n")

        CF._lowering_scope_source_files_cached.cache_clear()
        try:
            reached = {
                Path(p).name
                for p in CF._lowering_scope_source_files_cached(str(root))
            }
        finally:
            CF._lowering_scope_source_files_cached.cache_clear()

    assert "module_resolution.py" in reached  # seed
    assert "helper_a.py" in reached  # imported by seed
    assert "helper_b.py" in reached  # imported transitively
    assert "backend_orphan.py" not in reached  # unreferenced -> excluded


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


def test_import_molt_cli_does_not_load_backend() -> None:
    """``import molt.cli`` must not pull the backend / native-link / cargo layer.

    The reachability-derived lowering scope is only sound if the runtime import
    graph matches the static one: the frontend/module drivers reach ``molt.cli``
    (its ``__init__``), so ``__init__`` importing the backend eagerly would both
    load it on every frontend import *and* drag it back into the fingerprint
    scope. Runs in a fresh subprocess so the assertion is not polluted by modules
    an earlier test already imported.
    """
    import subprocess
    import sys

    root = Path(cf_module_file()).resolve().parents[3]
    probe = (
        "import sys, molt.cli\n"
        "backend = sorted(\n"
        "    m for m in sys.modules\n"
        "    if m.startswith('molt.cli.backend_')\n"
        "    or m.startswith('molt.cli.cargo')\n"
        "    or m in {\n"
        "        'molt.cli.link_pipeline', 'molt.cli.wasm',\n"
        "        'molt.cli.mlir_backend', 'molt.cli.native_toolchain',\n"
        "        'molt.cli.native_binary', 'molt.cli.native_link_command',\n"
        "        'molt.cli.runtime_build', 'molt.cli.setup_readiness',\n"
        "    }\n"
        ")\n"
        "assert not backend, 'backend loaded on import molt.cli: ' + repr(backend)\n"
        "print('OK')\n"
    )
    env = {**os.environ, "PYTHONPATH": str(root / "src")}
    result = subprocess.run(
        [sys.executable, "-c", probe],
        capture_output=True,
        text=True,
        env=env,
    )
    assert result.returncode == 0, (
        f"import-smoke failed:\nstdout={result.stdout}\nstderr={result.stderr}"
    )


def cf_module_file() -> str:
    from molt.cli import cache_fingerprints as cf

    return cf.__file__
