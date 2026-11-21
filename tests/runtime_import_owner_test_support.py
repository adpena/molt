"""Scan a runtime-import owner the way the compiler does (source_scan_authority.md).

Routing and AST admission happen in the resolution cache, the scan records its
catalog deferral, and the custody completes it with the live catalog.
"""

from __future__ import annotations

import ast
from pathlib import Path

from molt.compiler_analysis.python_imports import ImportScanMode
from molt.cli.models import _RuntimeImportScanCustody
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.compiler_analysis.python_import_scan import (
    _collect_import_scan_requests,
    _collect_imports_for_graph,
)
from molt.compiler_analysis.python_imports import _ImportScanRequests
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION


def scan_as_runtime_owner(
    tree: ast.AST,
    module_name: str,
    *,
    source_path: Path,
    custody: _RuntimeImportScanCustody | None,
    import_scan_mode: ImportScanMode = "full",
    namespace_premise: PythonNamespacePremise | None = None,
) -> _ImportScanRequests:
    """The completed scan: a non-owner's is strict; an owner's has the catalog."""
    projection = _ModuleResolutionCache().collect_graph_imports(
        source_path,
        tree,
        collector=_collect_imports_for_graph,
        module_name=module_name,
        import_scan_mode=import_scan_mode,
        runtime_import_custody=custody,
        namespace_premise=namespace_premise,
    )
    requests = _collect_import_scan_requests(
        projection,
        tree,
        is_package=source_path.name == "__init__.py",
        module_name=module_name,
        import_scan_mode=import_scan_mode,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        ast_digest_admission=_PythonAstDigestAdmission(tree),
        namespace_premise=namespace_premise,
    )
    if requests.runtime_catalog is None:
        return requests
    assert custody is not None
    return custody.complete_owner_scan(module_name, source_path.resolve(), requests)
