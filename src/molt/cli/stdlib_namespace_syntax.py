"""One release-built namespace syntax summary for each admitted stdlib source."""

from __future__ import annotations

import re
from pathlib import Path

from molt.cli.cache_fingerprints import _import_scan_tooling_fingerprint
from molt.cli.module_graph_cache import (
    _decode_namespace_summary,
    _encode_namespace_summary,
)
from molt.cli.module_source import PythonSourceSnapshot
from molt.compiler_analysis.python_namespace_writes import python_namespace_summary
from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission
from molt.compiler_distribution import InstalledCompiler, installed_compiler
from molt.source_root import compiler_source_root, installed_distribution_root
from molt.target_python import (
    TargetPythonVersion,
    _parse_source_for_target,
    _SUPPORTED_TARGET_PYTHON_VERSIONS,
)

_SCHEMA = "molt.stdlib-namespace-syntax.v1"
_SHA256 = re.compile(r"[0-9a-f]{64}")
_TARGET_TAGS = frozenset(target.tag for target in _SUPPORTED_TARGET_PYTHON_VERSIONS)
type _Key = tuple[str, str, str]


def stdlib_namespace_syntax_document(stdlib_root: Path) -> dict[str, object]:
    """Produce source-byte and target-bound namespace syntax from the staged tree."""
    records: dict[_Key, tuple[str, dict[str, object]]] = {}
    for path in sorted(stdlib_root.rglob("*.py")):
        captured = PythonSourceSnapshot.capture(path)
        package = ".".join(path.relative_to(stdlib_root).parts[:-1])
        for target in _SUPPORTED_TARGET_PYTHON_VERSIONS:
            try:
                tree = _parse_source_for_target(
                    captured.content, filename=str(path), target_python=target
                )
            except SyntaxError:
                # A target that refuses this source has no syntax summary.
                continue
            admission = _PythonAstDigestAdmission(tree)
            summary = python_namespace_summary(
                tree,
                package=package,
                target_python=target.feature_version,
                ast_digest_admission=admission,
            )
            records[target.tag, captured.sha256, package] = (
                admission.digest,
                _encode_namespace_summary(summary),
            )
    return {
        "schema": _SCHEMA,
        "producer": _import_scan_tooling_fingerprint(),
        "rows": [[*key, *record] for key, record in sorted(records.items())],
    }


class InstalledStdlibNamespaceSyntax:
    """Admit one signed syntax artifact per source-resolution operation."""

    def __init__(self, compiler: InstalledCompiler) -> None:
        document = compiler.capture_stdlib_namespace_syntax()
        if (
            not isinstance(document, dict)
            or set(document) != {"schema", "producer", "rows"}
            or document["schema"] != _SCHEMA
            or document["producer"] != _import_scan_tooling_fingerprint()
            or not isinstance(document["rows"], list)
        ):
            raise ValueError(
                "Invalid installed stdlib namespace syntax schema or producer"
            )
        self.records: dict[_Key, tuple[str, object]] = {}
        previous: _Key | None = None
        for row in document["rows"]:
            if not isinstance(row, list) or len(row) != 5:
                raise ValueError("Invalid installed stdlib namespace syntax row")
            target, source, package, ast_digest, summary = row
            if (
                not isinstance(target, str)
                or target not in _TARGET_TAGS
                or not isinstance(source, str)
                or _SHA256.fullmatch(source) is None
                or not isinstance(package, str)
                or not isinstance(ast_digest, str)
                or _SHA256.fullmatch(ast_digest) is None
                or not isinstance(summary, dict)
            ):
                raise ValueError("Invalid installed stdlib namespace syntax identity")
            key = (target, source, package)
            if previous is not None and key <= previous:
                raise ValueError("Installed stdlib namespace syntax rows are unordered")
            previous = key
            self.records[key] = (ast_digest, summary)

    @classmethod
    def selected(cls) -> InstalledStdlibNamespaceSyntax | None:
        source_root = compiler_source_root()
        if installed_distribution_root(source_root) is None:
            return None
        compiler = installed_compiler(source_root)
        assert compiler is not None
        return cls(compiler)

    def seed(
        self,
        snapshot: PythonSourceSnapshot,
        *,
        package: str,
        target: TargetPythonVersion,
        admission: _PythonAstDigestAdmission,
    ) -> None:
        context = (target.feature_version, package)
        if context in admission.namespace_summaries:
            return
        row = self.records.get((target.tag, snapshot.sha256, package))
        if row is not None and row[0] == admission.digest:
            admission.namespace_summaries[context] = _decode_namespace_summary(row[1])
