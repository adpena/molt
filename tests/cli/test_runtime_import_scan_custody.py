from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
from molt.cli.models import (
    ImportScanMode,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _RuntimeImportScanCustody,
)
from molt.cli.module_import_scanner import (
    _DynamicRelativeImportDiscovery,
    _collect_import_star_modules,
    _collect_imports,
    _collect_imports_for_graph,
    _sealed_import_modules,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.compiler_analysis.python_binding_flow import python_ast_digest
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    ModuleImportState,
    StaticImportRequest,
    StaticMetadataValue,
    UnresolvedStaticImportError,
    UNKNOWN_VALUE,
    _PythonAstDigestAdmission,
)


def _custody(
    tmp_path: Path,
    source: str = "__package__ = choose_package()\nfrom . import child\n",
) -> tuple[Path, _RuntimeImportScanCustody]:
    owner = (tmp_path / "entry.py").resolve()
    target = (tmp_path / "target.py").resolve()
    owner.write_text(source, encoding="utf-8")
    target.write_text("", encoding="utf-8")
    return owner, _RuntimeImportScanCustody(
        owners=(("pkg.entry", owner),),
        catalog=(("pkg.entry", owner), ("admitted.target", target)),
        owner_ast_digests=(("pkg.entry", python_ast_digest(ast.parse(source))),),
    )


@pytest.mark.parametrize("statement", ["from . import child", "from . import *"])
def test_dynamic_package_uses_complete_catalog_not_lexical_package(
    tmp_path: Path, statement: str
) -> None:
    source = f"__package__ = choose_package()\n{statement}\n"
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    imports = _collect_imports(
        tree,
        "pkg.entry",
        runtime_import_custody=custody,
        source_path=owner,
    )
    assert set(imports) == set(custody.modules)
    if statement.endswith("*"):
        assert set(
            _collect_import_star_modules(
                tree,
                "pkg.entry",
                runtime_import_custody=custody,
                source_path=owner,
            )
        ) == set(custody.modules)


@pytest.mark.parametrize("wrong_owner", ["name", "path", "missing"])
def test_runtime_custody_cannot_escape_source_owner(
    tmp_path: Path, wrong_owner: str
) -> None:
    owner, custody = _custody(tmp_path)
    tree = ast.parse("__package__ = choose_package()\nfrom . import child\n")
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(
            tree,
            "other.entry" if wrong_owner == "name" else "pkg.entry",
            runtime_import_custody=custody,
            source_path=(
                None
                if wrong_owner == "missing"
                else tmp_path / "other.py"
                if wrong_owner == "path"
                else owner
            ),
        )


@pytest.mark.parametrize("claim", ["matching", "name", "path", "ast", "missing"])
def test_intrinsic_globals_escape_requires_exact_runtime_source_custody(
    tmp_path: Path, claim: str
) -> None:
    source = (
        "from _intrinsics import require_intrinsic as require\n"
        "require('molt_demo', globals())\n"
        "from . import child\n"
    )
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(
        source.replace("molt_demo", "molt_other") if claim == "ast" else source
    )

    def collect() -> list[str]:
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            runtime_import_custody=None if claim == "missing" else custody,
            source_path=tmp_path / "other.py" if claim == "path" else owner,
        )

    if claim == "matching":
        assert set(collect()) == set(custody.modules) | {
            "_intrinsics",
            "_intrinsics.require_intrinsic",
        }
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            collect()


def test_custodied_scan_does_not_populate_strict_memory_cache(tmp_path: Path) -> None:
    owner, custody = _custody(tmp_path)
    tree = ast.parse("__package__ = choose_package()\nfrom . import child\n")
    cache = _ModuleResolutionCache()
    assert set(
        cache.collect_imports(
            owner,
            tree,
            collector=_collect_imports,
            module_name="pkg.entry",
            runtime_import_custody=custody,
        )
    ) == set(custody.modules)
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        cache.collect_imports(
            owner, tree, collector=_collect_imports, module_name="pkg.entry"
        )


@pytest.mark.parametrize(
    ("body", "expected_candidate"),
    [
        ("from . import child\n", "pkg.child"),
        (
            "__import__('child', globals(), locals(), (), 1)\n",
            "pkg.child",
        ),
        (
            "import importlib\nimportlib.import_module('.child', __package__)\n",
            "pkg.child",
        ),
    ],
)
def test_graph_projection_keeps_dynamic_relative_candidates_out_of_semantics(
    body: str,
    expected_candidate: str,
) -> None:
    tree = ast.parse("__package__ = choose_package()\n" + body)
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(tree, "pkg.entry")

    projection = _collect_imports_for_graph(tree, "pkg.entry")

    assert projection.requires_runtime_package_anchor
    assert expected_candidate in projection.dynamic_relative_import_candidates
    assert expected_candidate not in projection.imports


@pytest.mark.parametrize(
    "body",
    [
        "__import__('child', foreign_globals, None, (), 1)\n",
        "import importlib\nimportlib.import_module('.child', foreign_package)\n",
    ],
)
def test_graph_projection_never_uses_lexical_package_for_foreign_metadata(
    body: str,
) -> None:
    tree = ast.parse("__package__ = choose_package()\n" + body)

    projection = _collect_imports_for_graph(tree, "pkg.entry")

    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "pkg.child" not in projection.imports


@pytest.mark.parametrize(
    "body",
    [
        "__import__('child', {'__package__': 'foreign'}, None, (), 1)\n",
        "import importlib\nimportlib.import_module('.child', 'foreign')\n",
    ],
)
def test_foreign_relative_metadata_remains_semantic_not_lexical(body: str) -> None:
    projection = _collect_imports_for_graph(ast.parse(body), "pkg.entry")

    assert not projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "foreign.child" in projection.imports


def test_graph_projection_does_not_hide_non_anchor_resolution_errors() -> None:
    top = StaticMetadataValue.known("top")
    module_name = StaticMetadataValue.known("pkg.entry")
    contexts = (
        ModuleImportContext(
            "pkg.entry",
            False,
            ModuleImportState(UNKNOWN_VALUE, UNKNOWN_VALUE, module_name, False),
        ),
        ModuleImportContext(
            "pkg.entry",
            False,
            ModuleImportState(top, top, module_name, False),
        ),
    )

    with pytest.raises(UnresolvedStaticImportError, match="beyond_top"):
        _sealed_import_modules(
            StaticImportRequest.statement("", level=2, fromlist=("child",)),
            contexts,
            dynamic_relative_import_discovery=_DynamicRelativeImportDiscovery(),
        )


def test_graph_and_strict_scan_caches_are_distinct_and_ast_digest_keyed(
    tmp_path: Path,
) -> None:
    owner = tmp_path / "entry.py"
    owner.write_text("pass\n", encoding="utf-8")
    cache = _ModuleResolutionCache()
    tree = ast.parse("__package__ = choose_package()\nfrom . import first\n")

    first_projection = cache.collect_graph_imports(
        owner,
        tree,
        collector=_collect_imports_for_graph,
        module_name="pkg.entry",
    )
    imported = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    imported.names[0].name = "second"
    second_projection = cache.collect_graph_imports(
        owner,
        tree,
        collector=_collect_imports_for_graph,
        module_name="pkg.entry",
    )

    assert "pkg.first" in first_projection.dynamic_relative_import_candidates
    assert "pkg.second" in second_projection.dynamic_relative_import_candidates
    assert first_projection != second_projection
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        cache.collect_imports(
            owner,
            tree,
            collector=_collect_imports,
            module_name="pkg.entry",
        )


def test_custody_seed_preserves_alias_names_for_one_shared_source(
    tmp_path: Path,
) -> None:
    from molt.cli.module_graph_discovery import _discover_module_graph_from_paths

    owner = (tmp_path / "shared.py").resolve()
    owner.write_text("__package__ = choose_package()\nfrom . import child\n")
    first_child = (tmp_path / "first_child.py").resolve()
    second_child = (tmp_path / "second_child.py").resolve()
    first_child.write_text("pass\n")
    second_child.write_text("pass\n")
    tree = ast.parse(owner.read_text(encoding="utf-8"))
    custody = _RuntimeImportScanCustody(
        owners=(("first.owner", owner), ("second.owner", owner)),
        catalog=(
            ("first.owner", owner),
            ("first.child", first_child),
            ("second.owner", owner),
            ("second.child", second_child),
        ),
        owner_ast_digests=(
            ("first.owner", python_ast_digest(tree)),
            ("second.owner", python_ast_digest(tree)),
        ),
    )
    authority = _ModuleGraphScanAuthority(
        tuple(
            _ModuleSourceScanAuthority(name, path, "full", False)
            for name, path in custody.catalog
        )
    )

    result = _discover_module_graph_from_paths(
        (owner, owner),
        [tmp_path],
        [tmp_path],
        tmp_path,
        None,
        set(),
        full_scan_roots=True,
        runtime_import_custody=custody,
        enclosing_scan_authority=authority,
    )

    assert result.graph["first.owner"] == owner
    assert result.graph["second.owner"] == owner


@pytest.mark.parametrize(
    "scope,message",
    [
        ("partial", "full-depth owner scans"),
        ("missing", "requires enclosing source scan authority"),
        ("empty", "lacks enclosing source scan authority"),
    ],
)
def test_runtime_owners_require_full_depth_and_enclosing_authority(
    tmp_path: Path, scope: str, message: str
) -> None:
    from molt.cli.module_graph_discovery import _discover_module_graph_from_paths

    owner, custody = _custody(tmp_path)
    with pytest.raises(ValueError, match=message):
        _discover_module_graph_from_paths(
            (owner,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=scope != "partial",
            runtime_import_custody=custody,
            enclosing_scan_authority=(
                _ModuleGraphScanAuthority() if scope == "empty" else None
            ),
        )


def test_runtime_catalog_requires_source_and_survives_graph_unchanged(
    tmp_path: Path,
) -> None:
    owner, custody = _custody(tmp_path)
    custody.validate_graph(dict(custody.catalog))
    with pytest.raises(ValueError, match="lost source authority"):
        custody.validate_graph({"pkg.entry": owner})
    with pytest.raises(ValueError, match="lost source authority"):
        custody.validate_graph(
            {**dict(custody.catalog), "pkg.entry": tmp_path / "wrong.py"}
        )
    with pytest.raises(ValueError, match="requires a source"):
        _RuntimeImportScanCustody(
            owners=(("missing", tmp_path / "missing.py"),),
            catalog=(("missing", tmp_path / "missing.py"),),
            owner_ast_digests=(("missing", "missing"),),
        )
    # Reusing an immutable catalog must not reuse its old filesystem observation.
    dict(custody.catalog)["admitted.target"].unlink()
    with pytest.raises(ValueError, match="requires a source"):
        custody.validate_graph(dict(custody.catalog))
    dict(custody.catalog)["admitted.target"].mkdir()
    with pytest.raises(ValueError, match="requires a source"):
        custody.validate_graph(dict(custody.catalog))


def test_real_importlib_machinery_keeps_strict_finalizer_boundary() -> None:
    stdlib = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    owner = stdlib / "importlib" / "machinery.py"
    catalog = (
        ("importlib", stdlib / "importlib" / "__init__.py"),
        ("importlib.machinery", owner),
        ("importlib.util", stdlib / "importlib" / "util.py"),
    )
    tree = ast.parse(owner.read_text(encoding="utf-8"))
    custody = _RuntimeImportScanCustody(
        owners=(("importlib.machinery", owner),),
        catalog=catalog,
        owner_ast_digests=(("importlib.machinery", python_ast_digest(tree)),),
    )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(tree, "importlib.machinery")
    assert set(custody.modules).issubset(
        _collect_imports(
            tree,
            "importlib.machinery",
            runtime_import_custody=custody,
            source_path=owner,
        )
    )

    projection = _collect_imports_for_graph(tree, "importlib.machinery")
    assert projection.requires_runtime_package_anchor
    assert "importlib.util" in projection.dynamic_relative_import_candidates
    assert "importlib.util" not in projection.imports


def test_real_asyncio_graph_projection_retains_static_and_relative_discovery() -> None:
    stdlib = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    owner = stdlib / "asyncio" / "__init__.py"
    tree = ast.parse(owner.read_text(encoding="utf-8"))

    projection = _collect_imports_for_graph(
        tree,
        "asyncio",
        is_package=True,
        import_scan_mode="module_init",
    )

    assert projection.requires_runtime_package_anchor
    assert "ssl" in projection.imports
    assert "asyncio.exceptions" in projection.dynamic_relative_import_candidates
    assert "asyncio.exceptions" not in projection.imports


def test_real_asyncio_and_importlib_dynamic_sources_join_runtime_custody(
    tmp_path: Path,
) -> None:
    from molt.cli.module_graph import _prepare_entry_module_graph

    entry = tmp_path / "entry.py"
    entry.write_text("import asyncio\nimport importlib.machinery\n", encoding="utf-8")
    stdlib = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    reasons: dict[str, set[str]] = {}

    prepared, error = _prepare_entry_module_graph(
        source_path=entry,
        entry_module="entry",
        module_roots=[tmp_path],
        stdlib_root=stdlib,
        project_root=None,
        entry_tree=ast.parse(entry.read_text(encoding="utf-8")),
        diagnostics_enabled=False,
        module_reasons=reasons,
        json_output=False,
        target="native",
    )

    assert error is None and prepared is not None
    custody = prepared.runtime_import_scan_custody
    assert custody is not None
    assert (
        custody.owners_by_module["asyncio"]
        == (stdlib / "asyncio" / "__init__.py").resolve()
    )
    assert (
        custody.owners_by_module["importlib.machinery"]
        == (stdlib / "importlib" / "machinery.py").resolve()
    )
    assert (
        prepared.scan_authority.mode_for("asyncio", prepared.module_graph["asyncio"])
        == "full"
    )
    assert (
        prepared.scan_authority.mode_for(
            "importlib.machinery", prepared.module_graph["importlib.machinery"]
        )
        == "full"
    )


def test_source_claim_cannot_authorize_substituted_ast_or_cache_hit(
    tmp_path: Path,
) -> None:
    owner, custody = _custody(tmp_path)
    cache = _ModuleResolutionCache()
    cache.collect_imports(
        owner,
        ast.parse(owner.read_text()),
        collector=_collect_imports,
        module_name="pkg.entry",
        runtime_import_custody=custody,
    )
    substituted_tree = ast.parse("__package__ = attacker()\nfrom . import secret\n")
    with pytest.raises(ValueError, match="source AST changed"):
        cache.collect_imports(
            owner,
            substituted_tree,
            collector=_collect_imports,
            module_name="pkg.entry",
            runtime_import_custody=custody,
        )
    with pytest.raises(ValueError, match="source AST changed"):
        _collect_imports(
            substituted_tree,
            "pkg.entry",
            source_path=owner,
            runtime_import_custody=custody,
        )
    admitted_tree = ast.parse(owner.read_text(encoding="utf-8"))
    admission = _PythonAstDigestAdmission(admitted_tree)
    with pytest.raises(ValueError, match="different tree"):
        _collect_imports(
            substituted_tree,
            "pkg.entry",
            source_path=owner,
            runtime_import_custody=custody,
            ast_digest_admission=admission,
        )
    imported = next(
        node for node in ast.walk(admitted_tree) if isinstance(node, ast.ImportFrom)
    )
    imported.names[0].name = "mutated"
    with pytest.raises(ValueError, match="source AST changed"):
        _collect_imports(
            admitted_tree,
            "pkg.entry",
            source_path=owner,
            runtime_import_custody=custody,
        )


def test_full_module_analysis_keeps_custody_out_of_persisted_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import module_cache, module_graph_cache

    owner, custody = _custody(tmp_path)

    def forbidden_persisted_access(*args: object, **kwargs: object) -> None:
        raise AssertionError("custodied owner analysis must not use strict persistence")

    for name in (
        "_read_persisted_module_analysis",
        "_write_persisted_module_analysis",
    ):
        monkeypatch.setattr(module_cache, name, forbidden_persisted_access)
    for name in (
        "_read_persisted_import_scan_record",
        "_write_persisted_import_scan",
    ):
        monkeypatch.setattr(module_graph_cache, name, forbidden_persisted_access)
    result = module_cache._load_module_analysis(
        owner,
        module_name="pkg.entry",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(owner),
        resolution_cache=_ModuleResolutionCache(),
        project_root=tmp_path,
        runtime_import_custody=custody,
    )
    assert set(result[1]) == set(custody.modules)


@_source_tree_fingerprint_transaction()
def test_print_graph_import_plan_and_full_frontend_share_runtime_custody(
    tmp_path: Path,
) -> None:
    # Match build()'s transaction: frontend cache reads/writes share one source
    # fingerprint instead of repeatedly scanning the compiler during this test.
    from molt.cli.frontend_pipeline import _prepare_frontend_analysis
    from molt.cli.module_graph import (
        _materialize_import_plan,
        _prepare_entry_module_graph,
    )

    entry = tmp_path / "demo.py"
    entry.write_text("print(1)\n", encoding="utf-8")
    stdlib = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    reasons: dict[str, set[str]] = {}
    graph, error = _prepare_entry_module_graph(
        source_path=entry,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=stdlib,
        project_root=None,
        entry_tree=ast.parse(entry.read_text()),
        diagnostics_enabled=False,
        module_reasons=reasons,
        json_output=False,
        target="native",
    )
    assert error is None and graph is not None
    plan = _materialize_import_plan(
        prepared_module_graph=graph,
        module_reasons=reasons,
        stdlib_root=stdlib,
        artifacts_root=tmp_path / "tmp" / "acceptance" / "runs" / "custody" / "build",
        entry_module="demo",
        diagnostics_enabled=False,
    )
    assert plan.runtime_import_scan_custody is graph.runtime_import_scan_custody
    custody = plan.runtime_import_scan_custody
    assert custody is not None
    custody.validate_graph(plan.module_graph)
    assert set(custody.modules).issubset(plan.runtime_import_dispatch_roots)
    analysis, error = _prepare_frontend_analysis(
        module_graph=plan.module_graph,
        module_graph_metadata=plan.module_graph_metadata,
        module_resolution_cache=plan.module_resolution_cache,
        roots=plan.roots,
        stdlib_root=stdlib,
        stdlib_allowlist=set(plan.stdlib_allowlist),
        project_root=tmp_path,
        entry_module="demo",
        json_output=False,
        target_python=graph.target_python,
        dependency_known_modules=plan.known_modules,
        runtime_import_scan_custody=custody,
    )
    assert error is None and analysis is not None
    assert "demo" in analysis.module_order
    assert "importlib.machinery" in analysis.module_order
    assert (
        set(custody.modules) - {"importlib.machinery"}
        <= analysis.module_deps["importlib.machinery"]
    )


@pytest.mark.parametrize("mode", ["module_init", "module_init_static_helpers"])
@pytest.mark.parametrize("consumer", ["imports", "stars", "cache", "loader"])
def test_every_owner_scan_entry_rejects_incomplete_depth(
    tmp_path: Path,
    mode: ImportScanMode,
    consumer: str,
) -> None:
    from molt.cli.module_graph_discovery import _load_module_import_scan

    owner, custody = _custody(tmp_path)
    tree = ast.parse(owner.read_text(encoding="utf-8"))
    with pytest.raises(ValueError, match="full-depth owner scans"):
        if consumer == "imports":
            _collect_imports(
                tree,
                "pkg.entry",
                import_scan_mode=mode,
                runtime_import_custody=custody,
                source_path=owner,
            )
        elif consumer == "stars":
            _collect_import_star_modules(
                tree,
                "pkg.entry",
                import_scan_mode=mode,
                runtime_import_custody=custody,
                source_path=owner,
            )
        elif consumer == "cache":
            _ModuleResolutionCache().collect_imports(
                owner,
                tree,
                collector=_collect_imports,
                module_name="pkg.entry",
                import_scan_mode=mode,
                runtime_import_custody=custody,
            )
        else:
            _load_module_import_scan(
                owner,
                module_name="pkg.entry",
                is_package=False,
                import_scan_mode=mode,
                resolution_cache=_ModuleResolutionCache(),
                project_root=None,
                tree=tree,
                runtime_import_custody=custody,
            )
