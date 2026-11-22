"""Release-built namespace syntax and initializer requests for stdlib sources."""

from __future__ import annotations

import ast
from functools import cached_property
import io
import json
from pathlib import Path
import re
import sys
from typing import Literal, TypedDict
from zipfile import ZIP_DEFLATED, BadZipFile, ZipFile, ZipInfo

from molt.cli.cache_fingerprints import (
    _import_scan_tooling_fingerprint,
    _source_tree_fingerprint_transaction,
)
from molt.cli.json_cache import SOURCE_ANALYSIS_CACHE_MAX_BYTES
from molt.cli.module_graph_cache import (
    _decode_namespace_summary,
    _import_scan_payload,
    _encode_namespace_summary,
    _import_scan_cache_key,
)
from molt.cli.module_source import PythonSourceSnapshot
from molt.compiler_analysis.python_import_scan import SourceImportPurpose
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    python_namespace_name_universe,
)
from molt.compiler_analysis.python_namespace_writes import python_namespace_summary
from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission
from molt.compiler_distribution import InstalledCompiler, installed_compiler
from molt.exact_json import canonical_json_bytes, loads_exact
from molt.source_root import compiler_source_root, installed_distribution_root
from molt.target_python import (
    TargetPythonVersion,
    _parse_source_for_target,
    _SUPPORTED_TARGET_PYTHON_VERSIONS,
)

_SCHEMA = "molt.stdlib-namespace-syntax.v1"
NAMESPACE_SYNTAX_MEMBER = "namespace-syntax.json"
_INITIALIZERS = "initializers/"
_INITIALIZER_MEMBER = re.compile(r"initializers/[0-9a-f]{24}")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_TARGET_TAGS = frozenset(target.tag for target in _SUPPORTED_TARGET_PYTHON_VERSIONS)
type _Key = tuple[str, str, str]


class _InitializerContext(TypedDict):
    module_name: str
    is_package: bool
    import_scan_mode: Literal["module_init"]
    purpose: SourceImportPurpose
    target_python: TargetPythonVersion


def _write_initializers(
    archive: ZipFile,
    captured: PythonSourceSnapshot,
    admission: _PythonAstDigestAdmission,
    tree: ast.Module,
    *,
    context: _InitializerContext,
    premise: PythonNamespacePremise,
) -> None:
    from molt.cli.module_graph_discovery import _collect_module_import_scan
    from molt.cli.module_resolution import _ModuleResolutionCache

    # Native for this bundle coordinate, WASI, and source targets without an OS.
    for platform in (sys.platform, "wasi", None):
        try:
            _, requests = _collect_module_import_scan(
                captured.path,
                tree,
                **context,
                source_owner=None,
                source_arguments=(),
                runtime_import_custody=None,
                ast_digest_admission=admission,
                namespace_premise=premise,
                resolution_cache=_ModuleResolutionCache(target_sys_platform=platform),
                source=captured.text,
            )
        except UnresolvedStaticImportError:
            # A refusal is evaluated in the importing program's actual context.
            continue
        payload = json.dumps(
            _import_scan_payload(
                **context,
                scan=requests,
                source_sha256=captured.sha256,
                source_size=len(captured.content),
                target_sys_platform=platform,
            ),
            separators=(",", ":"),
        ).encode()
        if len(payload) > SOURCE_ANALYSIS_CACHE_MAX_BYTES:
            continue
        key = _import_scan_cache_key(
            captured.path,
            **context,
            namespace_premise=premise,
            snapshot=captured,
            target_sys_platform=platform,
        )
        archive.writestr(
            ZipInfo(_INITIALIZERS + key), payload, compress_type=ZIP_DEFLATED
        )


@_source_tree_fingerprint_transaction()
def encode_stdlib_source_summaries(stdlib_root: Path) -> bytes:
    """Produce an archive using the exact source scan record protocol."""
    from molt.cli.module_resolution import _module_name_from_relative_parts
    from molt.cli.python_import_resolution import python_package_child_names
    from molt.cli.python_import_topology import PythonImportTopology

    records: dict[_Key, tuple[str, dict[str, object]]] = {}
    archive_bytes = io.BytesIO()
    topology = PythonImportTopology()
    with ZipFile(archive_bytes, "w") as archive:
        for path in sorted(stdlib_root.rglob("*.py")):
            captured = PythonSourceSnapshot.capture(path)
            relative = path.relative_to(stdlib_root).parts
            package = ".".join(relative[:-1])
            module = _module_name_from_relative_parts(relative, fallback_parent="")
            is_package = path.name == "__init__.py"
            children = (
                tuple(
                    sorted(
                        python_package_child_names(
                            (path.parent,), import_topology=topology
                        )
                    )
                )
                if is_package
                else ()
            )
            premise = PythonNamespacePremise(children, 0, 0, (), ()).scan_view(
                python_namespace_name_universe(captured.text)
            )
            for target in _SUPPORTED_TARGET_PYTHON_VERSIONS:
                try:
                    tree = _parse_source_for_target(
                        captured.content, filename=str(path), target_python=target
                    )
                except SyntaxError:
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
                if module:
                    context = _InitializerContext(
                        module_name=module,
                        is_package=is_package,
                        import_scan_mode="module_init",
                        purpose=SourceImportPurpose.EXECUTION,
                        target_python=target,
                    )
                    _write_initializers(
                        archive,
                        captured,
                        admission,
                        tree,
                        context=context,
                        premise=premise,
                    )
        document = {
            "schema": _SCHEMA,
            "producer": _import_scan_tooling_fingerprint(),
            "rows": [[*key, *record] for key, record in sorted(records.items())],
        }
        archive.writestr(
            ZipInfo(NAMESPACE_SYNTAX_MEMBER),
            canonical_json_bytes(document),
            compress_type=ZIP_DEFLATED,
        )
    return archive_bytes.getvalue()


class InstalledStdlibSourceSummaries:
    """Admit one manifest-bound archive per source-resolution operation."""

    def __init__(self, compiler: InstalledCompiler) -> None:
        raw = compiler.capture_stdlib_source_summaries()
        try:
            self.archive = ZipFile(io.BytesIO(raw))
            names = self.archive.namelist()
            if (
                NAMESPACE_SYNTAX_MEMBER not in names
                or len(set(names)) != len(names)
                or any(
                    name != NAMESPACE_SYNTAX_MEMBER
                    and _INITIALIZER_MEMBER.fullmatch(name) is None
                    for name in names
                )
            ):
                raise ValueError("Invalid installed stdlib summary archive members")
        except BadZipFile as exc:
            raise ValueError("Invalid installed stdlib summary archive") from exc

    @cached_property
    def records(self) -> dict[_Key, tuple[str, object]]:
        document = loads_exact(self._read(NAMESPACE_SYNTAX_MEMBER).decode("utf-8"))
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
        records: dict[_Key, tuple[str, object]] = {}
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
            records[key] = (ast_digest, summary)
        return records

    @classmethod
    def selected(cls) -> InstalledStdlibSourceSummaries | None:
        source_root = compiler_source_root()
        if installed_distribution_root(source_root) is None:
            return None
        compiler = installed_compiler(source_root)
        assert compiler is not None
        return cls(compiler)

    def _read(self, name: str) -> bytes:
        member = self.archive.getinfo(name)
        if not 0 < member.file_size <= SOURCE_ANALYSIS_CACHE_MAX_BYTES:
            raise ValueError("Installed stdlib summary member exceeds its byte policy")
        with self.archive.open(member) as stream:
            payload = stream.read(member.file_size + 1)
        if len(payload) != member.file_size:
            raise ValueError("Installed stdlib summary member has the wrong size")
        return payload

    def initializer(self, key: str) -> bytes | None:
        try:
            return self._read(_INITIALIZERS + key)
        except KeyError:
            return None

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
