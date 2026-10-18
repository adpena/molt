from __future__ import annotations

import ast
from collections.abc import Callable
import functools
import hashlib
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.cli.python_import_resolution import LocalPythonModuleResolver

from molt.cli import module_source as _module_source
from molt.cli.python_import_topology import ExactEntryKind, PythonImportTopology
from molt.source_root import compiler_source_root
from molt.cli.models import ImportScanMode, _RuntimeImportScanCustody
from molt.compiler_analysis.python_imports import _ImportDiscoveryProjection
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    _PythonAstDigestAdmission,
    python_ast_digest,
)
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_source_for_target,
)
from molt.cli.project_roots import _is_path_within

_ModuleRootAliases = tuple[tuple[str, Path], ...]
_ResolutionContext = tuple[tuple[Path, ...], Path, frozenset[str]]
_ImportScanCacheKey = tuple[
    Path,
    str | None,
    bool,
    ImportScanMode,
    PythonSourceKey | None,
    tuple[str, ...],
    str,
    str,
    # A runtime-import owner's scan: it defers to the catalog, any generation.
    bool,
    PythonNamespacePremise | None,
]


def _is_ascii_identifier_part(part: str) -> bool:
    if not part:
        return False
    first = part[0]
    if not (first == "_" or "A" <= first <= "Z" or "a" <= first <= "z"):
        return False
    return all(
        ch == "_" or "A" <= ch <= "Z" or "a" <= ch <= "z" or "0" <= ch <= "9"
        for ch in part[1:]
    )


def _is_dotted_module_root_alias(prefix: str) -> bool:
    return all(_is_ascii_identifier_part(part) for part in prefix.split("."))


def _parse_module_root_alias_entry(
    entry: str,
    *,
    import_topology: PythonImportTopology | None = None,
    source_cwd: Path | None = None,
) -> tuple[str, Path] | None:
    prefix, sep, raw_path = entry.partition("=")
    if not sep:
        return None
    prefix = prefix.strip()
    raw_path = raw_path.strip()
    if not raw_path or not _is_dotted_module_root_alias(prefix):
        return None
    path = Path(raw_path).expanduser()
    if source_cwd is not None:
        path = source_cwd / path
    if import_topology is not None:
        selected = import_topology.probe(path)
        return None if selected is None else (prefix, selected.path)
    if not path.exists():
        return None
    return prefix, path.resolve()


def _module_root_aliases_from_env(
    *, import_topology: PythonImportTopology | None = None
) -> _ModuleRootAliases:
    raw_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if not raw_roots:
        return ()
    aliases: list[tuple[str, Path]] = []
    for entry in raw_roots.split(os.pathsep):
        if not entry:
            continue
        parsed = _parse_module_root_alias_entry(entry, import_topology=import_topology)
        if parsed is not None:
            aliases.append(parsed)
    return tuple(dict.fromkeys(aliases))


def _module_root_alias_match(
    module_name: str,
    aliases: _ModuleRootAliases | None = None,
) -> tuple[str, Path, str] | None:
    best: tuple[str, Path, str] | None = None
    best_len = -1
    alias_entries = aliases if aliases is not None else _module_root_aliases_from_env()
    for prefix, root in alias_entries:
        if module_name == prefix:
            suffix = ""
        elif module_name.startswith(prefix + "."):
            suffix = module_name[len(prefix) + 1 :]
        else:
            continue
        if len(prefix) > best_len:
            best = (prefix, root, suffix)
            best_len = len(prefix)
    return best


def _resolve_module_alias_path(
    module_name: str,
    aliases: _ModuleRootAliases | None = None,
    *,
    import_topology: PythonImportTopology | None = None,
) -> Path | None:
    topology = import_topology or PythonImportTopology()
    match = _module_root_alias_match(
        module_name,
        aliases
        if aliases is not None
        else _module_root_aliases_from_env(import_topology=topology),
    )
    if match is None:
        return None
    _prefix, root, suffix = match
    if not suffix:
        package_init = root / "__init__.py"
        if topology.is_file(package_init):
            return package_init
        module_file = root.with_suffix(".py")
        if topology.is_file(module_file):
            return module_file
        return None
    parts = tuple(part for part in suffix.split(".") if part)
    return _resolve_module_path_parts(parts, [root], import_topology=topology)


def _module_name_from_alias_path(
    resolved: Path,
    aliases: _ModuleRootAliases | None = None,
) -> str | None:
    resolved_parts = resolved.parts
    best_name: str | None = None
    best_len = -1
    alias_entries = aliases if aliases is not None else _module_root_aliases_from_env()
    for prefix, root in alias_entries:
        root_resolved = root.resolve()
        rel_parts = _relative_parts_if_within(resolved_parts, root_resolved.parts)
        if rel_parts is None or len(root_resolved.parts) <= best_len:
            continue
        if rel_parts == ("__init__.py",):
            best_name = prefix
            best_len = len(root_resolved.parts)
            continue
        suffix = _module_name_from_relative_parts(
            rel_parts, fallback_parent=resolved.parent.name
        )
        best_name = prefix if suffix is None else f"{prefix}.{suffix}"
        best_len = len(root_resolved.parts)
    return best_name


def _module_name_from_path(
    path: Path,
    roots: list[Path],
    stdlib_root: Path,
    *,
    module_root_aliases: _ModuleRootAliases | None = None,
) -> str:
    resolved = path.resolve()
    resolved_roots = tuple(root.resolve() for root in roots)
    resolved_stdlib_root = stdlib_root.resolve()
    cpython_test_root: Path | None = None
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if cpython_dir:
        cpython_test_root = (Path(cpython_dir) / "Lib" / "test").resolve()
    return _module_name_from_resolved_path(
        resolved,
        resolved_roots=resolved_roots,
        resolved_stdlib_root=resolved_stdlib_root,
        cpython_test_root=cpython_test_root,
        module_root_aliases=(
            _module_root_aliases_from_env()
            if module_root_aliases is None
            else module_root_aliases
        ),
    )


def _entry_module_root_for_path(
    path: Path, *, import_topology: PythonImportTopology | None = None
) -> Path:
    topology = import_topology or PythonImportTopology()
    selected = topology.probe(path)
    resolved = selected.path if selected is not None else path.resolve()
    package_dir = resolved.parent
    topmost_package_parent = package_dir
    while topology.is_file(package_dir / "__init__.py"):
        topmost_package_parent = package_dir.parent
        if topmost_package_parent == package_dir:
            break
        package_dir = topmost_package_parent
    return topmost_package_parent


def _module_name_from_resolved_path(
    resolved: Path,
    *,
    resolved_roots: tuple[Path, ...],
    resolved_stdlib_root: Path,
    cpython_test_root: Path | None,
    module_root_aliases: _ModuleRootAliases | None = None,
) -> str:
    resolved_parts = resolved.parts
    rel_parts = _relative_parts_if_within(resolved_parts, resolved_stdlib_root.parts)
    if rel_parts is not None:
        module_name = _module_name_from_relative_parts(
            rel_parts, fallback_parent=resolved.parent.name
        )
        if module_name is not None:
            return module_name

    alias_name = _module_name_from_alias_path(resolved, module_root_aliases)
    if alias_name is not None:
        return alias_name

    best_rel_parts: tuple[str, ...] | None = None
    best_len = -1
    for root_resolved in resolved_roots:
        if cpython_test_root is not None and root_resolved == cpython_test_root:
            continue
        candidate_parts = _relative_parts_if_within(resolved_parts, root_resolved.parts)
        if candidate_parts is None:
            continue
        root_len = len(root_resolved.parts)
        if root_len > best_len:
            best_len = root_len
            best_rel_parts = candidate_parts
    if best_rel_parts is None:
        # Paths outside known module roots still compile deterministically.
        if resolved.name == "__init__.py":
            return resolved.parent.name or "__init__"
        return resolved.stem
    module_name = _module_name_from_relative_parts(
        best_rel_parts, fallback_parent=resolved.parent.name
    )
    if module_name is not None:
        return module_name
    return resolved.parent.name or resolved.stem


def _stdlib_root_path() -> Path:
    # Compiler and stdlib inputs always come from the same selected source.
    # Missing installation data must never borrow a guest/cwd stdlib tree.
    return (compiler_source_root() / "src/molt/stdlib").resolve(strict=False)


def _resolve_module_path(
    module_name: str,
    roots: list[Path],
    *,
    import_topology: PythonImportTopology | None = None,
    module_root_aliases: _ModuleRootAliases | None = None,
) -> Path | None:
    topology = import_topology or PythonImportTopology()
    alias_path = _resolve_module_alias_path(
        module_name, module_root_aliases, import_topology=topology
    )
    if alias_path is not None:
        return alias_path
    return _resolve_module_path_parts(
        tuple(module_name.split(".")), roots, import_topology=topology
    )


def _case_exact_file_under(
    root_text: str, rel_parts: tuple[str, ...], *, import_topology: PythonImportTopology
) -> bool:
    if not rel_parts:
        return False
    current = Path(root_text)
    last = len(rel_parts) - 1
    for index, part in enumerate(rel_parts):
        kind: ExactEntryKind = "file" if index == last else "directory"
        if not import_topology.contains_exact(current, part, kind):
            return False
        current /= part
    return import_topology.is_file(current)


def _case_exact_file(
    root: Path, path: Path, *, import_topology: PythonImportTopology | None = None
) -> bool:
    """Whether ``path`` is a file whose components below ``root`` match case.

    CPython's FileFinder matches case only inside the directory it searches,
    and uses the search root as given. ``root`` is such a root: its ancestors
    are never listed, so a change above it, such as a new entry in a shared
    temporary directory, cannot change or fail the lookup.
    """
    try:
        parts = path.relative_to(root).parts
    except ValueError:
        return False
    return _case_exact_file_under(
        os.fspath(root),
        parts,
        import_topology=import_topology or PythonImportTopology(),
    )


def _python_module_resolver(
    roots: tuple[Path, ...], import_topology: PythonImportTopology
) -> "LocalPythonModuleResolver":
    from molt.cli.python_import_resolution import LocalPythonModuleResolver

    # Import admission owns external source permission; this resolver selects
    # candidates. Tooling keeps its stricter source-root containment by default.
    return LocalPythonModuleResolver(
        roots,
        import_topology=import_topology,
        case_exact=True,
        restrict_sources=False,
    )


def _resolve_module_path_parts(
    parts: tuple[str, ...],
    roots: list[Path],
    *,
    import_topology: PythonImportTopology | None = None,
) -> Path | None:
    if not parts or not roots:
        return None
    topology = import_topology or PythonImportTopology()
    return _python_module_resolver(tuple(roots), topology).source_for_module(
        ".".join(parts)
    )


@dataclass(frozen=True)
class _ParsedModuleAst:
    """One parsed source generation; discovery and analysis keep it read-only."""

    tree: ast.Module

    @functools.cached_property
    def admission(self) -> _PythonAstDigestAdmission:
        return _PythonAstDigestAdmission(self.tree)


# Source text digest, target tag and native roots -> pruned AST digest. The key
# names the content, so entries stay valid across builds; FIFO bounds it.
_MODULE_AST_DIGESTS: dict[tuple[bytes, str, tuple[str, ...]], str] = {}
_MODULE_AST_DIGEST_LIMIT = 4096


@dataclass
class _ModuleResolutionCache:
    import_topology: PythonImportTopology = field(default_factory=PythonImportTopology)
    module_root_aliases: _ModuleRootAliases | None = None
    python_resolvers: dict[tuple[Path, ...], "LocalPythonModuleResolver"] = field(
        default_factory=dict
    )

    def __post_init__(self) -> None:
        if self.module_root_aliases is None:
            self.module_root_aliases = _module_root_aliases_from_env(
                import_topology=self.import_topology
            )
        else:
            for _prefix, path in self.module_root_aliases:
                self.import_topology.probe(path)

    def _python_resolver(self, roots: list[Path]) -> "LocalPythonModuleResolver":
        key = tuple(roots)
        if key not in self.python_resolvers:
            self.python_resolvers[key] = _python_module_resolver(
                key, self.import_topology
            )
        return self.python_resolvers[key]

    roots_cache: dict[tuple[str, _ResolutionContext], list[Path]] = field(
        default_factory=dict
    )
    resolve_cache: dict[
        tuple[str, _ModuleRootAliases, _ResolutionContext], Path | None
    ] = field(default_factory=dict)
    resolved_path_cache: dict[Path, Path] = field(default_factory=dict)
    resolved_roots_cache: dict[tuple[Path, ...], tuple[Path, ...]] = field(
        default_factory=dict
    )
    source_cache: dict[Path, str] = field(default_factory=dict)
    source_error_cache: dict[Path, Exception] = field(default_factory=dict)
    ast_cache: dict[tuple[Path, str, str, str], _ParsedModuleAst] = field(
        default_factory=dict
    )
    ast_error_cache: dict[tuple[Path, str, str, str], SyntaxError] = field(
        default_factory=dict
    )
    module_name_cache: dict[
        tuple[Path, tuple[Path, ...], Path, Path | None, _ModuleRootAliases], str
    ] = field(default_factory=dict)
    module_name_context_key: (
        tuple[tuple[Path, ...], Path, Path | None, _ModuleRootAliases] | None
    ) = None
    module_name_context_cache: dict[Path, str] = field(default_factory=dict)
    stdlib_path_cache: dict[tuple[Path, Path], bool] = field(default_factory=dict)
    graph_import_scan_cache: dict[_ImportScanCacheKey, _ImportDiscoveryProjection] = (
        field(default_factory=dict)
    )
    path_stat_cache: dict[Path, os.stat_result] = field(default_factory=dict)
    path_stat_error_cache: dict[Path, OSError] = field(default_factory=dict)
    cpython_test_root_cache: Path | None = None
    cpython_test_root_cache_populated: bool = False
    source_parse_count: int = 0
    source_prune_count: int = 0
    native_support_view_cache: dict[
        tuple[Path, str], tuple[str, tuple[str, ...], _ParsedModuleAst]
    ] = field(default_factory=dict)

    def roots_for_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> list[Path]:
        key = (module_name, (tuple(roots), stdlib_root, frozenset(stdlib_allowlist)))
        candidate_roots = self.roots_cache.get(key)
        if candidate_roots is None:
            candidate_roots = _roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            self.roots_cache[key] = candidate_roots
        return candidate_roots

    def resolve_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> Path | None:
        cache_name = module_name
        if module_name.startswith("molt.stdlib."):
            cache_name = f"stdlib:{module_name}"
        module_root_aliases = self.module_root_aliases
        assert module_root_aliases is not None
        cache_key = (
            cache_name,
            module_root_aliases,
            (tuple(roots), stdlib_root, frozenset(stdlib_allowlist)),
        )
        if cache_key not in self.resolve_cache:
            match = _module_root_alias_match(module_name, module_root_aliases)
            if match is not None and match[2]:
                alias_path = self._python_resolver([match[1]]).source_for_module(
                    match[2]
                )
            else:
                alias_path = _resolve_module_alias_path(
                    module_name,
                    module_root_aliases,
                    import_topology=self.import_topology,
                )
            if alias_path is not None:
                self.resolve_cache[cache_key] = alias_path
            elif cache_name.startswith("stdlib:"):
                stdlib_candidate = module_name[len("molt.stdlib.") :]
                self.resolve_cache[cache_key] = self._python_resolver(
                    [stdlib_root]
                ).source_for_module(stdlib_candidate)
            else:
                candidate_roots = self.roots_for_module(
                    module_name, roots, stdlib_root, stdlib_allowlist
                )
                self.resolve_cache[cache_key] = (
                    self._python_resolver(candidate_roots).source_for_module(
                        module_name
                    )
                    if candidate_roots
                    else None
                )
        return self.resolve_cache[cache_key]

    def package_locations(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> tuple[Path, ...]:
        aliases = self.module_root_aliases
        assert aliases is not None
        match = _module_root_alias_match(module_name, aliases)
        if match is not None:
            _prefix, root, suffix = match
            if not suffix:
                if self.import_topology.is_file(root / "__init__.py"):
                    return (root,)
                if self.import_topology.is_file(root.with_suffix(".py")):
                    return ()
                alias_locations = (root,) if self.import_topology.is_dir(root) else ()
            else:
                resolver = self._python_resolver([root])
                alias_locations = resolver.package_locations(suffix)
                if resolver.source_for_module(suffix) is not None:
                    return alias_locations
            # A source-less alias may fall through to an ordinary source, as
            # source lookup does. Retain namespace portions only if no source
            # won the normal lookup below.
            if (
                self.resolve_module(module_name, roots, stdlib_root, stdlib_allowlist)
                is None
            ):
                if alias_locations:
                    return alias_locations
        if module_name.startswith("molt.stdlib."):
            return self._python_resolver([stdlib_root]).package_locations(
                module_name[len("molt.stdlib.") :]
            )
        candidates = self.roots_for_module(
            module_name, roots, stdlib_root, stdlib_allowlist
        )
        return (
            self._python_resolver(candidates).package_locations(module_name)
            if candidates
            else ()
        )

    def has_namespace_dir(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> bool:
        return self.resolve_module(
            module_name, roots, stdlib_root, stdlib_allowlist
        ) is None and bool(
            self.package_locations(module_name, roots, stdlib_root, stdlib_allowlist)
        )

    def resolved_path(self, path: Path) -> Path:
        resolved = self.resolved_path_cache.get(path)
        if resolved is None:
            selected = self.import_topology.probe(path)
            if selected is not None:
                resolved = selected.path
            elif (
                path.is_absolute() and "." not in path.parts and ".." not in path.parts
            ):
                resolved = path
            else:
                resolved = path.resolve()
            self.resolved_path_cache[path] = resolved
        return resolved

    def resolved_roots(self, roots: list[Path]) -> tuple[Path, ...]:
        roots_key = tuple(roots)
        resolved = self.resolved_roots_cache.get(roots_key)
        if resolved is None:
            resolved = tuple(self.resolved_path(root) for root in roots)
            self.resolved_roots_cache[roots_key] = resolved
        return resolved

    def module_name_from_path(
        self, path: Path, roots: list[Path], stdlib_root: Path
    ) -> str:
        resolved = self.resolved_path(path)
        resolved_roots = self.resolved_roots(roots)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cpython_test_root = self.cpython_test_root()
        module_root_aliases = self.module_root_aliases
        assert module_root_aliases is not None
        context_key = (
            resolved_roots,
            resolved_stdlib_root,
            cpython_test_root,
            module_root_aliases,
        )
        if self.module_name_context_key == context_key:
            cached = self.module_name_context_cache.get(resolved)
            if cached is not None:
                return cached
        else:
            self.module_name_context_key = context_key
            self.module_name_context_cache.clear()
        cache_key = (
            resolved,
            resolved_roots,
            resolved_stdlib_root,
            cpython_test_root,
            module_root_aliases,
        )
        cached = self.module_name_cache.get(cache_key)
        if cached is not None:
            self.module_name_context_cache[resolved] = cached
            return cached
        module_name = _module_name_from_resolved_path(
            resolved,
            resolved_roots=resolved_roots,
            resolved_stdlib_root=resolved_stdlib_root,
            cpython_test_root=cpython_test_root,
            module_root_aliases=module_root_aliases,
        )
        self.module_name_cache[cache_key] = module_name
        self.module_name_context_cache[resolved] = module_name
        return module_name

    def cpython_test_root(self) -> Path | None:
        if not self.cpython_test_root_cache_populated:
            cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
            if cpython_dir:
                self.cpython_test_root_cache = self.resolved_path(
                    Path(cpython_dir) / "Lib" / "test"
                )
            self.cpython_test_root_cache_populated = True
        return self.cpython_test_root_cache

    def is_stdlib_path(self, path: Path, stdlib_root: Path) -> bool:
        resolved_path = self.resolved_path(path)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cache_key = (resolved_path, resolved_stdlib_root)
        cached = self.stdlib_path_cache.get(cache_key)
        if cached is None:
            cached = _is_stdlib_resolved_path(resolved_path, resolved_stdlib_root)
            self.stdlib_path_cache[cache_key] = cached
        return cached

    def read_module_source(self, path: Path, *, retain: bool = True) -> str:
        cache_key = self.resolved_path(path)
        if not retain:
            return _module_source._read_module_source(path)
        source = self.source_cache.get(cache_key)
        if source is not None:
            return source
        cached_error = self.source_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            source = _module_source._read_module_source(path)
        except (OSError, SyntaxError, UnicodeDecodeError) as exc:
            self.source_error_cache[cache_key] = exc
            raise
        self.source_cache[cache_key] = source
        return source

    def path_stat(self, path: Path) -> os.stat_result:
        cache_key = self.resolved_path(path)
        cached = self.path_stat_cache.get(cache_key)
        if cached is not None:
            return cached
        cached_error = self.path_stat_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            stat_result = path.stat()
        except OSError as exc:
            self.path_stat_error_cache[cache_key] = exc
            raise
        self.path_stat_cache[cache_key] = stat_result
        return stat_result

    def parse_module_ast(
        self,
        path: Path,
        source: str,
        *,
        filename: str,
        target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
        retain: bool = True,
    ) -> _ParsedModuleAst:
        cache_key = (self.resolved_path(path), filename, target_python.tag, source)
        tree = self.ast_cache.get(cache_key)
        if tree is not None:
            return tree
        # Analysis may borrow discovery's read-only tree without retaining a
        # newly parsed miss. Content, diagnostic filename and target still bind
        # every reuse; mutating lowering owns its separate parsed tree.
        if not retain:
            self.source_parse_count += 1
            return _ParsedModuleAst(
                _parse_source_for_target(
                    source,
                    filename=filename,
                    target_python=target_python,
                )
            )
        cached_error = self.ast_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            self.source_parse_count += 1
            tree = _ParsedModuleAst(
                _parse_source_for_target(
                    source,
                    filename=filename,
                    target_python=target_python,
                )
            )
        except SyntaxError as exc:
            self.ast_error_cache[cache_key] = exc
            raise
        self.ast_cache[cache_key] = tree
        return tree

    def module_ast_digest(
        self,
        path: Path,
        *,
        native_roots: tuple[str, ...],
        target_python: TargetPythonVersion,
    ) -> str:
        """The AST digest of ``path``'s current source, pruned to ``native_roots``.

        The digest is a function of the source text, the target and the roots,
        so it is memoized by those values: finalization asks once per round
        for every runtime-import owner, and a resident process asks again on
        every build.
        """
        source = self.read_module_source(path, retain=False)
        key = (
            hashlib.sha256(source.encode("utf-8", "surrogatepass")).digest(),
            target_python.tag,
            native_roots,
        )
        digest = _MODULE_AST_DIGESTS.get(key)
        if digest is not None:
            return digest
        parsed = self.parse_module_ast(
            path,
            source,
            filename=str(path),
            retain=bool(native_roots),
            target_python=target_python,
        )
        if native_roots:
            parsed, _pruned = self.native_support_view(
                path,
                parsed.tree,
                native_roots,
                ast_digest_admission=parsed.admission,
                target_python=target_python,
            )
        digest = python_ast_digest(parsed.tree)
        if len(_MODULE_AST_DIGESTS) >= _MODULE_AST_DIGEST_LIMIT:
            _MODULE_AST_DIGESTS.pop(next(iter(_MODULE_AST_DIGESTS)))
        _MODULE_AST_DIGESTS[key] = digest
        return digest

    def native_support_view(
        self,
        path: Path,
        tree: ast.Module,
        roots: tuple[str, ...],
        *,
        ast_digest_admission: _PythonAstDigestAdmission,
        target_python: TargetPythonVersion,
    ) -> tuple[_ParsedModuleAst, bool]:
        from molt.compiler_analysis.native_support_slice import (
            prune_native_support_module,
        )

        admission = _PythonAstDigestAdmission.for_tree(tree, ast_digest_admission)
        key = (self.resolved_path(path), target_python.tag)
        cached = self.native_support_view_cache.get(key)
        if cached is not None and cached[:2] == (admission.digest, roots):
            return cached[2], False
        self.source_prune_count += 1
        tree, _reachable, missing = prune_native_support_module(tree, roots)
        if missing:
            raise ValueError(
                f"native support roots lack source custody in {path}: {list(missing)}"
            )
        selected = _ParsedModuleAst(tree)
        # One current view per source/target, never a retained AST per root prefix.
        self.native_support_view_cache[key] = (admission.digest, roots, selected)
        return selected, True

    def collect_graph_imports(
        self,
        path: Path,
        tree: ast.AST,
        *,
        collector: Callable[..., _ImportDiscoveryProjection],
        module_name: str | None = None,
        is_package: bool = False,
        import_scan_mode: ImportScanMode = "full",
        source_owner: PythonSourceKey | None = None,
        source_arguments: tuple[str, ...] = (),
        target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
        runtime_import_custody: _RuntimeImportScanCustody | None = None,
        ast_digest_admission: _PythonAstDigestAdmission | None = None,
        namespace_premise: PythonNamespacePremise | None = None,
    ) -> _ImportDiscoveryProjection:
        """Cache source graph facts, never strict import acceptance or live edges."""
        ast_digest_admission = _PythonAstDigestAdmission.for_tree(
            tree, ast_digest_admission
        )
        runtime_catalog_owner = False
        if runtime_import_custody is not None:
            source_path = path.resolve()
            if runtime_import_custody.owns(module_name, source_path):
                if source_owner is not None:
                    raise ValueError(
                        "runtime import custody requires full-depth owner scans"
                    )
                runtime_import_custody.validate_scan_mode(
                    module_name, source_path, import_scan_mode
                )
                runtime_import_custody.admits_scan(
                    module_name, source_path, ast_digest_admission.digest
                )
                runtime_catalog_owner = True
        # An owner's scan defers to the catalog, so every custody generation of
        # one owner source shares it; completion applies each live catalog.
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            import_scan_mode,
            source_owner,
            source_arguments,
            target_python.tag,
            ast_digest_admission.digest,
            runtime_catalog_owner,
            namespace_premise,
        )
        cached = self.graph_import_scan_cache.get(cache_key)
        if cached is not None:
            return cached
        owner_kwargs = {"runtime_catalog_owner": True} if runtime_catalog_owner else {}
        result = collector(
            tree,
            module_name,
            is_package,
            import_scan_mode=import_scan_mode,
            source_owner=source_owner,
            source_arguments=source_arguments,
            target_python=target_python,
            ast_digest_admission=ast_digest_admission,
            namespace_premise=namespace_premise,
            **owner_kwargs,
        )
        self.graph_import_scan_cache[cache_key] = result
        return result


def _has_namespace_dir(
    module_name: str,
    roots: list[Path],
    *,
    import_topology: PythonImportTopology | None = None,
) -> bool:
    if not roots:
        return False
    resolver = _python_module_resolver(
        tuple(roots), import_topology or PythonImportTopology()
    )
    return resolver.source_for_module(module_name) is None and bool(
        resolver.package_locations(module_name)
    )


def _is_stdlib_resolved_path(resolved: Path, resolved_stdlib_root: Path) -> bool:
    return (
        _relative_parts_if_within(resolved.parts, resolved_stdlib_root.parts)
        is not None
    )


def _relative_parts_if_within(
    candidate_parts: tuple[str, ...], root_parts: tuple[str, ...]
) -> tuple[str, ...] | None:
    if len(candidate_parts) < len(root_parts):
        return None
    if candidate_parts[: len(root_parts)] != root_parts:
        return None
    return candidate_parts[len(root_parts) :]


def _module_name_from_relative_parts(
    rel_parts: tuple[str, ...], *, fallback_parent: str
) -> str | None:
    if not rel_parts:
        return None
    if rel_parts[-1] == "__init__.py":
        package_parts = rel_parts[:-1]
        if package_parts:
            return ".".join(package_parts)
        return fallback_parent or None
    last = rel_parts[-1]
    if last.endswith(".py"):
        rel_parts = (*rel_parts[:-1], last[:-3])
    filtered = tuple(part for part in rel_parts if part)
    if not filtered:
        return fallback_parent or None
    return ".".join(filtered)


def _is_stdlib_module(name: str, stdlib_allowlist: set[str]) -> bool:
    if name.startswith("molt."):
        return False
    if name in stdlib_allowlist:
        return True
    top = name.split(".", 1)[0]
    return top in stdlib_allowlist


def _roots_for_module(
    module_name: str,
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
) -> list[Path]:
    if _is_stdlib_module(module_name, stdlib_allowlist):
        if module_name == "test.tokenizedata" or module_name.startswith(
            "test.tokenizedata."
        ):
            return [stdlib_root] + [root for root in roots if root != stdlib_root]
        if module_name == "test" or module_name.startswith("test."):
            if os.environ.get("MOLT_REGRTEST_CPYTHON_DIR"):
                return roots
        return [stdlib_root]
    return roots


def _runtime_owned_module_roots() -> tuple[Path, ...]:
    stdlib_root = _stdlib_root_path().resolve()
    package_root = stdlib_root.parent
    return (
        stdlib_root,
        package_root / "gpu",
        package_root / "lib",
    )


def _is_runtime_owned_module_path(module_path: Path) -> bool:
    return any(
        _is_path_within(module_path, root) for root in _runtime_owned_module_roots()
    )
