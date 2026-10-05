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
    _collect_import_scan_requests,
    _collect_import_star_modules,
    _complete_import_scan,
    _collect_imports,
    _collect_imports_for_graph,
    _sealed_import_modules,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from molt.compiler_analysis.python_source_keys import (
    _PythonAstDigestAdmission,
    python_ast_digest,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    ModuleImportState,
    StaticImportRequest,
    StaticMetadataValue,
    UnresolvedStaticImportError,
    UNKNOWN_VALUE,
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


@pytest.mark.parametrize(
    "call,candidate,claim",
    [
        ("__import__('admitted.target', **options)", "admitted.target", "matching"),
        ("load_builtin('admitted.target', **options)", "admitted.target", "matching"),
        (
            "importlib.import_module('admitted.target', **options)",
            "admitted.target",
            "matching",
        ),
        (
            "importlib.util.find_spec('admitted.target', **options)",
            "admitted.target",
            "matching",
        ),
        ("load_builtin(*arguments, **options)", None, "matching"),
        (
            "load_builtin(*arguments, name='admitted.target')",
            "admitted.target",
            "matching",
        ),
        ("importlib.import_module('.target', **options)", None, "matching"),
        ("load_builtin('admitted.target', **options)", "admitted.target", "name"),
        ("load_builtin('admitted.target', **options)", "admitted.target", "path"),
        ("load_builtin('admitted.target', **options)", "admitted.target", "ast"),
        ("load_builtin('admitted.target', **options)", "admitted.target", "missing"),
        (
            "__import__('admitted.target', fromlist=options)",
            "admitted.target",
            "matching",
        ),
        (
            "load_builtin('admitted.target', fromlist=options)",
            "admitted.target",
            "matching",
        ),
        ("__import__('admitted.target', fromlist=[1])", "admitted.target", "matching"),
        (
            "__import__('admitted.target', fromlist=('*',))",
            "admitted.target",
            "matching",
        ),
        ("__import__('admitted.target', level=options)", "admitted.target", "matching"),
        ("__import__('admitted.target', fromlist=options)", "admitted.target", "name"),
        ("__import__('admitted.target', fromlist=options)", "admitted.target", "path"),
        ("__import__('admitted.target', fromlist=options)", "admitted.target", "ast"),
        (
            "__import__('admitted.target', fromlist=options)",
            "admitted.target",
            "missing",
        ),
    ],
)
def test_dynamic_import_operands_require_exact_runtime_source_custody(
    tmp_path: Path, call: str, candidate: str | None, claim: str
) -> None:
    source = (
        "import importlib\nimport importlib.util\n"
        "from builtins import __import__ as load_builtin\n"
        f"def deferred(arguments, options):\n    return {call}\n"
    )
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == (
        (candidate,) if candidate is not None else ()
    )
    assert "admitted.target" not in projection.imports
    if claim == "ast":
        tree = ast.parse(source + "changed = True\n")

    def collect() -> list[str]:
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            runtime_import_custody=None if claim == "missing" else custody,
            source_path=tmp_path / "other.py" if claim == "path" else owner,
        )

    if claim == "matching":
        assert set(custody.modules) <= set(collect())
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            collect()


@pytest.mark.parametrize(
    "namespace_expression",
    [
        "{'__package__': __package__, '__name__': 'wrong.mod'}",
        "{**{'__package__': __package__}, '__name__': 'wrong.mod'}",
        "{'__package__': __package__, '__name__': (__package__ := 'inside')}",
    ],
)
@pytest.mark.parametrize(
    "later_effect",
    [
        "globals().__delitem__('__package__')",
        "globals().__setitem__('__package__', 'later')",
    ],
)
def test_captured_dictionary_metadata_precedes_later_argument_effects(
    namespace_expression, later_effect
) -> None:
    source = f"__import__('child', {namespace_expression}, {later_effect}, [], 1)\n"
    captured = []

    def importing(name, globals, locals, fromlist, level):
        captured.append((name, dict(globals), fromlist, level))

    namespace = {
        "__package__": "pkg",
        "__name__": "pkg.entry",
        "__import__": importing,
    }
    exec(compile(source, "<captured-import-metadata>", "exec"), namespace)
    assert captured[0][1]["__package__"] == "pkg"
    assert namespace.get("__package__") != "pkg"

    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert "pkg.child" in projection.dynamic_relative_import_candidates
    assert not {"wrong.child", "later.child", "inside.child"} & set(
        projection.dynamic_relative_import_candidates
    )
    assert "pkg.child" not in projection.imports
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(ast.parse(source), "pkg.entry")


def test_captured_name_fallback_uses_its_read_point() -> None:
    source = (
        "__import__('child', {'__package__': None, '__spec__': None, "
        "'__name__': __name__}, "
        "globals().__setitem__('__name__', 'wrong.mod'), [], 1)\n"
    )
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert "pkg.child" in projection.dynamic_relative_import_candidates
    assert "wrong.child" not in projection.dynamic_relative_import_candidates
    assert "wrong.child" not in projection.imports


def test_current_globals_mapping_retains_invocation_time_contents() -> None:
    source = (
        "__import__('child', globals(), "
        "globals().__setitem__('__package__', 'later'), [], 1)\n"
    )
    captured = []

    def importing(name, globals, locals, fromlist, level):
        captured.append(globals["__package__"])

    namespace = {"__package__": "pkg", "__import__": importing}
    exec(compile(source, "<live-import-metadata>", "exec"), namespace)
    assert captured == ["later"]
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert "later.child" in (
        *projection.imports,
        *projection.dynamic_relative_import_candidates,
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


def test_graph_cache_keeps_custody_separate_from_strict_acceptance(
    tmp_path: Path,
) -> None:
    owner, custody = _custody(tmp_path)
    tree = ast.parse("__package__ = choose_package()\nfrom . import child\n")
    cache = _ModuleResolutionCache()
    custodied = cache.collect_graph_imports(
        owner,
        tree,
        collector=_collect_imports_for_graph,
        module_name="pkg.entry",
        runtime_import_custody=custody,
    )
    assert set(custodied.imports) == set(custody.modules)
    uncustodied = cache.collect_graph_imports(
        owner, tree, collector=_collect_imports_for_graph, module_name="pkg.entry"
    )
    assert uncustodied == _collect_imports_for_graph(tree, "pkg.entry")
    assert uncustodied != custodied
    assert uncustodied.requires_runtime_package_anchor
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(tree, "pkg.entry", source_path=owner)


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


def test_graph_projection_does_not_hide_nondeferrable_statement_errors() -> None:
    top = StaticMetadataValue("invalid")
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

    with pytest.raises(UnresolvedStaticImportError, match="invalid_package"):
        _sealed_import_modules(
            StaticImportRequest.statement("", level=2, fromlist=("child",)),
            contexts,
            dynamic_relative_import_discovery=_DynamicRelativeImportDiscovery(),
        )


def test_graph_scan_cache_is_ast_digest_keyed_without_certifying_strict_imports(
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
        _collect_imports(tree, "pkg.entry", source_path=owner)


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
    assert "asyncio.events" in prepared.module_graph
    assert "asyncio._debug" not in prepared.module_graph
    assert not (stdlib / "asyncio" / "_debug.py").exists()
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
    cache.collect_graph_imports(
        owner,
        ast.parse(owner.read_text()),
        collector=_collect_imports_for_graph,
        module_name="pkg.entry",
        runtime_import_custody=custody,
    )
    substituted_tree = ast.parse("__package__ = attacker()\nfrom . import secret\n")
    with pytest.raises(ValueError, match="source AST changed"):
        cache.collect_graph_imports(
            owner,
            substituted_tree,
            collector=_collect_imports_for_graph,
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


@pytest.mark.parametrize(
    "source,needs_custody",
    [
        ("print(1)\n", False),
        ("import importlib.machinery\nprint(1)\n", True),
    ],
    ids=["print-only", "runtime-import"],
)
@_source_tree_fingerprint_transaction()
def test_graph_import_plan_and_full_frontend_share_runtime_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, source: str, needs_custody: bool
) -> None:
    # Match build()'s transaction: frontend cache reads/writes share one source
    # fingerprint instead of repeatedly scanning the compiler during this test.
    from molt.cli.frontend_pipeline import _prepare_frontend_analysis
    from molt.cli.module_graph import (
        _materialize_import_plan,
        _prepare_entry_module_graph,
    )

    # A print-only micro program no longer imports runtime protocol owners as a
    # side effect of builtin publication. The positive case owns a real import.
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", "micro")
    entry = tmp_path / "demo.py"
    entry.write_text(source, encoding="utf-8")
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
    assert (custody is not None) is needs_custody
    if custody is not None:
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
    if custody is not None:
        assert "importlib.machinery" in analysis.module_order
        assert (
            set(custody.modules) - {"importlib.machinery"}
            <= analysis.module_deps["importlib.machinery"]
        )
    else:
        assert "importlib.machinery" not in analysis.module_order


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
            _ModuleResolutionCache().collect_graph_imports(
                owner,
                tree,
                collector=_collect_imports_for_graph,
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


@pytest.mark.parametrize("fromlist", ["names", "[1]", "('*',)"])
@pytest.mark.parametrize("prefix", ["", "__package__ = choose_package()\n"])
def test_dynamic_fromlist_keeps_known_relative_level(
    tmp_path: Path, fromlist: str, prefix: str
) -> None:
    source = (
        prefix + "def load(names):\n"
        f"    return __import__('child', globals(), None, {fromlist}, 1)\n"
    )
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")

    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ("pkg.child",)
    assert "child" not in projection.imports
    assert "pkg.child" not in projection.imports
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(tree, "pkg.entry", source_path=owner)
    assert set(custody.modules) <= set(
        _collect_imports(
            tree, "pkg.entry", source_path=owner, runtime_import_custody=custody
        )
    )


@pytest.mark.parametrize(
    "globals_expr,candidates",
    [
        ("{'__package__': 'foreign'}", ("foreign.child",)),
        ("foreign_globals", ()),
    ],
)
def test_dynamic_fromlist_preserves_explicit_globals_authority(
    globals_expr: str, candidates: tuple[str, ...]
) -> None:
    tree = ast.parse(
        "def load(names):\n"
        f"    return __import__('child', {globals_expr}, None, names, 1)\n"
    )
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == candidates
    assert "pkg.child" not in projection.imports
    assert "foreign.child" not in projection.imports


@pytest.mark.parametrize(
    "spelling",
    ["_MOLT_IMPORTLIB_IMPORT_TRANSACTION", "molt_importlib_import_transaction"],
)
def test_import_transaction_spelling_neither_grants_nor_blocks_identity(
    spelling: str,
) -> None:
    body = f"{spelling}('child', globals(), None, ('*',), 1)\n"
    fake = ast.parse(f"{spelling} = object()\n" + body)
    projection = _collect_imports_for_graph(fake, "pkg.entry")
    assert projection.imports == ()
    assert projection.dynamic_relative_import_candidates == ()
    assert not projection.requires_runtime_package_anchor
    assert _collect_import_star_modules(fake, "pkg.entry") == ()

    canonical = ast.parse(f"from builtins import __import__ as {spelling}\n" + body)
    projection = _collect_imports_for_graph(canonical, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ("pkg.child",)
    assert "child" not in projection.imports


@pytest.mark.parametrize(
    "source,mode,expected_base,expected_children",
    [
        (
            "__import__('bundle', fromlist=('*',))\n",
            "full",
            "bundle",
            ("bundle.child",),
        ),
        (
            "import bundle\n__import__('bundle', fromlist=('*',))\n",
            "full",
            "bundle",
            ("bundle.child",),
        ),
        (
            "from builtins import __import__ as load\n"
            "load('bundle', fromlist=('*',))\n",
            "full",
            "bundle",
            ("bundle.child",),
        ),
        (
            "from builtins import __import__ as load\n"
            "def helper(name):\n    return load(name, fromlist=('*',))\n"
            "helper('bundle')\n",
            "module_init",
            "bundle",
            ("bundle.child",),
        ),
        (
            "__package__ = choose_package()\n"
            "__import__('bundle', globals(), None, ('*',), 1)\n",
            "full",
            "pkg.bundle",
            ("pkg.bundle.child",),
        ),
        (
            "__import__('bundle', fromlist=('named', '*'))\n",
            "full",
            "bundle",
            ("bundle.named", "bundle.child"),
        ),
    ],
)
def test_call_star_uses_live_all_expansion_without_promoting_discovery(
    tmp_path: Path,
    source: str,
    mode: ImportScanMode,
    expected_base: str,
    expected_children: tuple[str, ...],
) -> None:
    package = tmp_path.joinpath(*expected_base.split("."))
    package.mkdir(parents=True)
    if expected_base.startswith("pkg."):
        (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    init = package / "__init__.py"
    init.write_text("__all__ = ['child']\n", encoding="utf-8")
    for child in ("child", "named", "later"):
        (package / f"{child}.py").write_text("VALUE = 1\n", encoding="utf-8")
    owner = tmp_path / "entry.py"
    owner.write_text(source, encoding="utf-8")
    tree = ast.parse(source)
    target = _DEFAULT_TARGET_PYTHON_VERSION
    projection = _collect_imports_for_graph(tree, "pkg.entry", import_scan_mode=mode)
    requests = _collect_import_scan_requests(
        projection,
        tree,
        source_path=owner,
        module_name="pkg.entry",
        import_scan_mode=mode,
        target_python=target,
        ast_digest_admission=_PythonAstDigestAdmission(tree),
        source=source,
    )
    assert requests.star_modules == ()
    assert requests.dynamic_star_modules == (expected_base,)
    cache = _ModuleResolutionCache()

    def complete():
        return _complete_import_scan(
            requests,
            source_path=owner,
            roots=[tmp_path],
            stdlib_root=tmp_path,
            stdlib_allowlist=set(),
            resolution_cache=cache,
            target_python=target,
        )

    completed = complete()
    assert expected_base in completed.dynamic_relative_import_candidates
    assert all(
        child in completed.dynamic_relative_import_candidates
        for child in expected_children
    )
    assert all(child not in completed.imports for child in expected_children)
    assert (expected_base in completed.imports) == source.startswith("import bundle\n")
    # Filesystem-derived __all__ expansion remains live, not cached source closure.
    init.write_text("__all__ = ['later']\n", encoding="utf-8")
    changed = complete()
    assert expected_base + ".later" in changed.dynamic_relative_import_candidates
    assert expected_base + ".child" not in changed.dynamic_relative_import_candidates


@pytest.mark.parametrize(
    "call,error",
    [
        ("__import__('child', level=-1)", "negative_level"),
        ("__import__('child', level=1)", "missing_globals"),
        ("importlib.import_module('.child')", "no_parent"),
        ("importlib.import_module('.child', package=1)", "invalid_package"),
    ],
)
@pytest.mark.parametrize("claim", ["matching", "missing", "name", "path", "ast"])
def test_catchable_call_resolution_errors_require_exact_custody(
    tmp_path: Path,
    call: str,
    error: str,
    claim: str,
) -> None:
    source = (
        "import importlib\ndef caught():\n    try:\n"
        f"        return {call}\n"
        "    except (ValueError, TypeError, ImportError):\n        return 'caught'\n"
    )
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "child" not in projection.imports
    if claim == "ast":
        tree = ast.parse(source + "changed = True\n")

    def collect():
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            source_path=tmp_path / "other.py" if claim == "path" else owner,
            runtime_import_custody=None if claim == "missing" else custody,
        )

    if claim == "matching":
        assert set(custody.modules) <= set(collect())
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match=error):
            collect()


@pytest.mark.parametrize(
    "prefix,statement,error",
    [
        ("", "from .. import child", "beyond_top"),
        ("__package__ = ''\n", "from . import child", "no_parent"),
        ("", "from .. import *", "beyond_top"),
    ],
)
@pytest.mark.parametrize("claim", ["matching", "missing", "name", "path", "ast"])
def test_catchable_relative_statement_errors_require_exact_custody(
    tmp_path: Path,
    prefix: str,
    statement: str,
    error: str,
    claim: str,
) -> None:
    source = prefix + "try:\n    " + statement + "\nexcept ImportError:\n    pass\n"
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.imports == ()
    assert projection.dynamic_relative_import_candidates == ()
    if claim == "ast":
        tree = ast.parse(source + "changed = True\n")

    def collect():
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            source_path=tmp_path / "other.py" if claim == "path" else owner,
            runtime_import_custody=None if claim == "missing" else custody,
        )

    if claim == "matching":
        assert set(custody.modules) <= set(collect())
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match=error):
            collect()


def test_relative_statement_invalid_metadata_stays_rejected_under_custody(
    tmp_path: Path,
) -> None:
    source = "__package__ = 1\nfrom . import child\n"
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    with pytest.raises(UnresolvedStaticImportError, match="invalid_package"):
        _collect_imports_for_graph(tree, "pkg.entry")
    with pytest.raises(UnresolvedStaticImportError, match="invalid_package"):
        _collect_imports(
            tree, "pkg.entry", source_path=owner, runtime_import_custody=custody
        )


@pytest.mark.parametrize("damage", [None, "missing", "invalid", "old_schema"])
def test_persisted_scan_retains_explicit_dynamic_star_provenance(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    damage: str | None,
) -> None:
    import json
    from molt.cli import module_graph_cache, module_source
    from molt.cli.models import _ImportScanRequests

    owner = tmp_path / "entry.py"
    owner.write_text(
        "import bundle\n__import__('bundle', fromlist=('*',))\n", encoding="utf-8"
    )
    monkeypatch.setattr(
        module_graph_cache,
        "_frontend_semantic_tooling_fingerprint",
        lambda: "star-provenance",
    )
    requests = _ImportScanRequests(
        imports=("bundle",),
        source_executions=(),
        dynamic_relative_import_candidates=("bundle",),
        requires_runtime_package_anchor=True,
        dynamic_star_modules=("bundle",),
    )
    module_graph_cache._write_persisted_import_scan(
        tmp_path,
        owner,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        scan=requests,
        snapshot=module_source.PythonSourceSnapshot.capture(owner),
    )
    if damage is not None:
        path = module_graph_cache._import_scan_cache_path(
            tmp_path,
            owner,
            module_name="entry",
            is_package=False,
            import_scan_mode="full",
        )
        payload = json.loads(path.read_text(encoding="utf-8"))
        if damage == "missing":
            del payload["dynamic_star_modules"]
        elif damage == "invalid":
            payload["dynamic_star_modules"] = [1]
        else:
            payload["version"] -= 1
        path.write_text(json.dumps(payload), encoding="utf-8")
    restored = module_graph_cache._read_persisted_import_scan_record(
        tmp_path,
        owner,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
    )
    if damage is None:
        assert restored == requests
        assert restored.star_modules == ()
        assert restored.dynamic_star_modules == ("bundle",)
    else:
        assert restored is None


@pytest.mark.parametrize(
    "setup,call,error",
    [
        ("", "__import__('child', level=~0)", "negative_level"),
        ("level = -1\n", "__import__('child', level=level)", "negative_level"),
        ("level = +1\n", "__import__('child', level=-level)", "negative_level"),
        ("", "__import__('child', level=1.5)", "invalid_level"),
        ("level = None\n", "__import__('child', level=level)", "invalid_level"),
        (
            "bad = -1\n",
            "__import__('child', {'__package__': bad}, level=1)",
            "invalid_package",
        ),
        (
            "bad = +1.5\n",
            "__import__('child', {'__name__': bad}, level=1)",
            "invalid_name",
        ),
        ("bad = -1\n", "importlib.import_module('.child', bad)", "invalid_package"),
    ],
)
@pytest.mark.parametrize("claim", ["matching", "missing", "name", "path", "ast"])
def test_known_scalar_import_errors_keep_no_candidate_and_exact_custody(
    tmp_path: Path,
    setup: str,
    call: str,
    error: str,
    claim: str,
) -> None:
    # Locals keep source-owned scalar facts independently of imported-module
    # initialization and the function's replaceable global namespace.
    source = (
        "import importlib\ndef caught():\n"
        + "".join("    " + line + "\n" for line in setup.splitlines())
        + "    try:\n        return "
        + call
        + "\n    except (ValueError, TypeError, ImportError):\n        return 'caught'\n"
    )
    owner, custody = _custody(tmp_path, source)
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "child" not in projection.imports
    if claim == "ast":
        tree = ast.parse(source + "changed = True\n")

    def collect():
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            source_path=tmp_path / "other.py" if claim == "path" else owner,
            runtime_import_custody=None if claim == "missing" else custody,
        )

    if claim == "matching":
        assert set(custody.modules) <= set(collect())
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match=error):
            collect()


@pytest.mark.parametrize("level", ["+1", "-(-1)", "~~1", "True", "+True"])
def test_numeric_unary_and_bool_import_levels_preserve_foreign_package(
    level: str,
) -> None:
    tree = ast.parse(
        f"__import__('child', {{'__package__': 'foreign'}}, level={level})\n"
    )
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert "foreign.child" in projection.imports
    assert "pkg.child" not in projection.imports
    assert projection.dynamic_relative_import_candidates == ()
    assert not projection.requires_runtime_package_anchor


@pytest.mark.parametrize("expression", ["None", "False", "0", "''", "()", "[]"])
def test_proven_falsy_fromlist_is_not_an_unknown_import_operand(
    expression: str,
) -> None:
    tree = ast.parse(f"__import__('pkg.child', fromlist={expression})\n")
    projection = _collect_imports_for_graph(tree, "entry")
    assert "pkg.child" in projection.imports
    assert not projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "pkg.child" in _collect_imports(tree, "entry")


def test_bound_none_fromlist_retains_the_null_fact() -> None:
    tree = ast.parse("children = None\n__import__('pkg.child', fromlist=children)\n")
    projection = _collect_imports_for_graph(tree, "entry")
    assert "pkg.child" in projection.imports
    assert not projection.requires_runtime_package_anchor


def test_local_package_parameter_never_borrows_module_metadata() -> None:
    tree = ast.parse(
        "import importlib\ndef load(__package__):\n"
        "    return importlib.import_module('.child', __package__)\n"
    )
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "pkg.child" not in projection.imports


def test_star_requests_share_one_cached_custodied_graph_pass(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import module_import_scanner as scanner
    from molt.cli.module_graph_discovery import _load_module_import_scan

    source = "__import__('bundle', fromlist=('*',))\n"
    owner = (tmp_path / "entry.py").resolve()
    owner.write_text(source, encoding="utf-8")
    package = tmp_path / "bundle"
    package.mkdir()
    init = (package / "__init__.py").resolve()
    init.write_text("__all__ = ['child']\n", encoding="utf-8")
    (package / "child.py").write_text("VALUE = 1\n", encoding="utf-8")
    tree = ast.parse(source)
    custody = _RuntimeImportScanCustody(
        owners=(("pkg.entry", owner),),
        catalog=(("pkg.entry", owner), ("bundle", init)),
        owner_ast_digests=(("pkg.entry", python_ast_digest(tree)),),
    )
    via = tmp_path / "via"
    via.mkdir()
    raw_path = via / ".." / owner.name
    cache = _ModuleResolutionCache()
    original = scanner._collect_imports
    calls = 0

    def collect(*args, **kwargs):
        nonlocal calls
        calls += 1
        return original(*args, **kwargs)

    monkeypatch.setattr(scanner, "_collect_imports", collect)
    for path in (raw_path, owner):
        loaded = _load_module_import_scan(
            path,
            module_name="pkg.entry",
            is_package=False,
            import_scan_mode="full",
            resolution_cache=cache,
            project_root=None,
            tree=tree,
            source=source,
            roots=[tmp_path],
            stdlib_root=tmp_path,
            stdlib_allowlist=set(),
            runtime_import_custody=custody,
        )
        assert "bundle.child" in loaded.scan.imports
        assert "bundle.child" not in loaded.scan.dynamic_relative_import_candidates
    assert calls == 1
    projection = next(iter(cache.graph_import_scan_cache.values()))
    assert "bundle" in projection.star_modules
    assert projection.dynamic_star_modules == ()


@pytest.mark.parametrize("operator", ["+", "-", "~"])
def test_unary_index_callbacks_keep_dynamic_level_custody(operator: str) -> None:
    source = "level = unknown\n__import__('child', level=" + operator + "level)\n"
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ("child",)
    assert "child" not in projection.imports


def test_shadowed_globals_call_never_acquires_current_package_authority() -> None:
    source = "def globals():\n    return foreign_mapping\n__import__('child', globals(), level=1)\n"
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "pkg.child" not in projection.imports


def test_current_globals_alias_preserves_its_canonical_package_identity() -> None:
    source = "namespace = globals\n__import__('child', namespace(), level=1)\n"
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert "pkg.child" in projection.imports
    assert not projection.requires_runtime_package_anchor


@pytest.mark.parametrize(
    "source",
    [
        "def read():\n    return globals()\n",
        "def read():\n    def anchor(): pass\n    return anchor.__globals__\n",
        "def read():\n    import inspect\n    return inspect.currentframe().f_globals\n",
        "def read():\n    from builtins import globals as current_globals\n"
        "    return current_globals()\n",
    ],
)
def test_possible_globals_provenance_is_only_a_discovery_alternative(source) -> None:
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_imports import (
        dunder_globals_state_from_expression,
    )

    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    expression = tree.body[0].body[-1].value
    context = ModuleImportContext("pkg.entry", False)
    assert (
        dunder_globals_state_from_expression(
            expression, context, expression_fact=index.expression_fact
        )
        is None
    )
    discovery = dunder_globals_state_from_expression(
        expression,
        context,
        expression_fact=index.expression_fact,
        allow_possible_current_globals=True,
    )
    assert discovery is not None
    assert discovery.package == StaticMetadataValue.known("pkg")


@pytest.mark.parametrize(
    "name,expected", [("__package__", "pkg"), ("__name__", "pkg.entry")]
)
def test_explicit_metadata_global_load_requires_stable_activation(
    name, expected
) -> None:
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_imports import metadata_value_from_expression

    context = ModuleImportContext("pkg.entry", False)
    source = f"def read():\n    return {name}\n"
    tree = ast.parse(source)
    expression = tree.body[0].body[0].value
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(expression)
    assert fact is not None and not fact.module_metadata.activation_namespace_stable
    assert (
        metadata_value_from_expression(
            expression, context, expression_fact=index.expression_fact
        )
        == UNKNOWN_VALUE
    )
    assert metadata_value_from_expression(
        expression,
        context,
        expression_fact=index.expression_fact,
        allow_activation_metadata_for_discovery=True,
    ) == StaticMetadataValue.known(expected)


def test_deferred_import_module_package_load_keeps_candidate_without_authority() -> (
    None
):
    source = (
        "import importlib\ndef load():\n"
        "    return importlib.import_module('.child', __package__)\n"
    )
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert "pkg.child" in projection.dynamic_relative_import_candidates
    assert "pkg.child" not in projection.imports
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _collect_imports(ast.parse(source), module_name="pkg.entry")


def test_proven_local_metadata_remains_authoritative_in_deferred_activation() -> None:
    source = (
        "import importlib\ndef load():\n"
        "    loader = importlib.import_module\n"
        "    __package__ = 'explicit.pkg'\n"
        "    return loader('.child', __package__)\n"
    )
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert "explicit.pkg.child" in projection.imports
    assert not projection.requires_runtime_package_anchor


def test_definitely_shadowed_globals_never_acquires_discovery_namespace() -> None:
    source = (
        "def load(globals):\n"
        "    return __import__('child', globals(), None, ('*',), 1)\n"
    )
    projection = _collect_imports_for_graph(ast.parse(source), "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert projection.dynamic_relative_import_candidates == ()
    assert "pkg.child" not in projection.imports


@pytest.mark.parametrize(
    "source",
    [
        "items.attribute\nfrom . import child\n",
        "items.attribute\n__import__('child', globals(), level=1)\n",
        "from . import sibling\nfrom . import child\n",
        "import importlib\nfrom . import sibling\nimportlib.import_module('.child', __package__)\n",
        "def load():\n    from . import child\n",
        "__import__('child', {'__package__': __package__, '__name__': 'other.mod'}, globals().__delitem__('__package__'), [], 1)\n",
    ],
)
@pytest.mark.parametrize("claim", ["matching", "name", "path", "ast", "missing"])
def test_statement_and_expression_metadata_share_exact_catalog_custody(
    tmp_path, source, claim
):
    owner, original = _custody(tmp_path, source)
    child = tmp_path / "child.py"
    child.write_text("", encoding="utf-8")
    custody = _RuntimeImportScanCustody(
        owners=original.owners,
        catalog=(*original.catalog, ("pkg.child", child.resolve())),
        owner_ast_digests=original.owner_ast_digests,
    )
    tree = ast.parse(source)
    projection = _collect_imports_for_graph(tree, "pkg.entry")
    assert projection.requires_runtime_package_anchor
    assert "pkg.child" in projection.dynamic_relative_import_candidates
    assert "pkg.child" not in projection.imports
    assert "other.child" not in projection.imports
    assert "other.child" not in projection.dynamic_relative_import_candidates
    if claim == "ast":
        tree = ast.parse(source + "changed = True\n")

    def collect():
        return _collect_imports(
            tree,
            "other.entry" if claim == "name" else "pkg.entry",
            runtime_import_custody=None if claim == "missing" else custody,
            source_path=tmp_path / "other.py" if claim == "path" else owner,
        )

    if claim == "matching":
        assert set(custody.modules) <= set(collect())
    elif claim == "ast":
        with pytest.raises(ValueError, match="source AST changed"):
            collect()
    else:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            collect()
