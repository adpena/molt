from __future__ import annotations

import ast
from collections.abc import Callable, Collection
import functools
import os
from dataclasses import dataclass, field
from pathlib import Path

from molt.cli import module_source as _module_source
from molt.cli.models import ImportScanMode
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_source_for_target,
)
from molt.cli.toolchain_validation import _is_path_within


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
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
    )


def _entry_module_root_for_path(path: Path) -> Path:
    resolved = path.resolve()
    package_dir = resolved.parent
    topmost_package_parent = package_dir
    while (package_dir / "__init__.py").exists():
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
) -> str:
    resolved_parts = resolved.parts
    rel_parts = _relative_parts_if_within(resolved_parts, resolved_stdlib_root.parts)
    if rel_parts is not None:
        module_name = _module_name_from_relative_parts(
            rel_parts, fallback_parent=resolved.parent.name
        )
        if module_name is not None:
            return module_name

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
    override = os.environ.get("MOLT_PROJECT_ROOT")
    if override:
        root = Path(override).expanduser()
        if not root.is_absolute():
            root = (Path.cwd() / root).absolute()
        candidate = root / "src/molt/stdlib"
        if candidate.exists():
            return candidate.resolve()
    package_root = Path(__file__).resolve().parents[1]
    candidate = package_root / "stdlib"
    if candidate.exists():
        return candidate.resolve()
    candidate = Path(__file__).resolve().parent / "stdlib"
    if candidate.exists():
        return candidate.resolve()
    return Path("src/molt/stdlib").resolve()


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    return _resolve_module_path_parts(tuple(module_name.split(".")), roots)


@functools.lru_cache(maxsize=65536)
def _case_exact_dir_entries_cached(
    dir_text: str, mtime_ns: int, size: int
) -> frozenset[str]:
    del mtime_ns, size
    try:
        return frozenset(entry.name for entry in os.scandir(dir_text))
    except OSError:
        return frozenset()


def _case_exact_dir_entries(dir_text: str) -> frozenset[str]:
    try:
        stat = os.stat(dir_text)
    except OSError:
        return frozenset()
    return _case_exact_dir_entries_cached(dir_text, stat.st_mtime_ns, stat.st_size)


def _case_exact_file_under(root_text: str, rel_parts: tuple[str, ...]) -> bool:
    if not rel_parts:
        return False
    current = root_text
    for part in rel_parts:
        if part not in _case_exact_dir_entries(current):
            return False
        current = os.path.join(current, part)
    return os.path.isfile(current)


def _case_exact_file(path: Path) -> bool:
    if path.is_absolute():
        anchor = path.anchor
        rel_parts = tuple(part for part in path.parts[1:] if part)
        return _case_exact_file_under(anchor, rel_parts)
    return _case_exact_file_under(os.curdir, tuple(path.parts))


def _resolve_module_path_parts(
    parts: tuple[str, ...], roots: list[Path]
) -> Path | None:
    if not parts:
        return None
    module_filename = f"{parts[-1]}.py"
    for root in roots:
        root_text = os.fspath(root)
        pkg_text = os.path.join(root_text, *parts, "__init__.py")
        if _case_exact_file_under(root_text, (*parts, "__init__.py")):
            return Path(pkg_text)
        if len(parts) == 1:
            mod_text = os.path.join(root_text, module_filename)
            mod_parts = (module_filename,)
        else:
            mod_text = os.path.join(root_text, *parts[:-1], module_filename)
            mod_parts = (*parts[:-1], module_filename)
        if _case_exact_file_under(root_text, mod_parts):
            return Path(mod_text)
    return None


@dataclass
class _ModuleResolutionCache:
    roots_cache: dict[str, list[Path]] = field(default_factory=dict)
    resolve_cache: dict[str, Path | None] = field(default_factory=dict)
    namespace_dir_cache: dict[str, bool] = field(default_factory=dict)
    resolved_path_cache: dict[Path, Path] = field(default_factory=dict)
    resolved_roots_cache: dict[tuple[Path, ...], tuple[Path, ...]] = field(
        default_factory=dict
    )
    source_cache: dict[Path, str] = field(default_factory=dict)
    source_error_cache: dict[Path, Exception] = field(default_factory=dict)
    ast_cache: dict[tuple[Path, str, str], ast.AST] = field(default_factory=dict)
    ast_error_cache: dict[tuple[Path, str, str], SyntaxError] = field(
        default_factory=dict
    )
    runtime_import_protocol_cache: dict[
        tuple[Path, str | None, bool, ImportScanMode], bool
    ] = field(default_factory=dict)
    module_name_cache: dict[tuple[Path, tuple[Path, ...], Path, Path | None], str] = (
        field(default_factory=dict)
    )
    module_name_context_key: tuple[tuple[Path, ...], Path, Path | None] | None = None
    module_name_context_cache: dict[Path, str] = field(default_factory=dict)
    stdlib_path_cache: dict[tuple[Path, Path], bool] = field(default_factory=dict)
    import_scan_cache: dict[
        tuple[Path, str | None, bool, ImportScanMode], tuple[str, ...]
    ] = field(default_factory=dict)
    path_stat_cache: dict[Path, os.stat_result] = field(default_factory=dict)
    path_stat_error_cache: dict[Path, OSError] = field(default_factory=dict)
    module_parts_cache: dict[str, tuple[str, ...]] = field(default_factory=dict)
    cpython_test_root_cache: Path | None = None
    cpython_test_root_cache_populated: bool = False

    def roots_for_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> list[Path]:
        candidate_roots = self.roots_cache.get(module_name)
        if candidate_roots is None:
            candidate_roots = _roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            self.roots_cache[module_name] = candidate_roots
        return candidate_roots

    def module_parts(self, module_name: str) -> tuple[str, ...]:
        cached = self.module_parts_cache.get(module_name)
        if cached is None:
            cached = tuple(module_name.split("."))
            self.module_parts_cache[module_name] = cached
        return cached

    def resolve_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> Path | None:
        cache_key = module_name
        if module_name.startswith("molt.stdlib."):
            cache_key = f"stdlib:{module_name}"
        if cache_key not in self.resolve_cache:
            if cache_key.startswith("stdlib:"):
                stdlib_candidate = module_name[len("molt.stdlib.") :]
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(stdlib_candidate), [stdlib_root]
                )
            else:
                candidate_roots = self.roots_for_module(
                    module_name, roots, stdlib_root, stdlib_allowlist
                )
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(module_name), candidate_roots
                )
        return self.resolve_cache[cache_key]

    def has_namespace_dir(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> bool:
        has_namespace_dir = self.namespace_dir_cache.get(module_name)
        if has_namespace_dir is None:
            candidate_roots = self.roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            has_namespace_dir = _has_namespace_dir(module_name, candidate_roots)
            self.namespace_dir_cache[module_name] = has_namespace_dir
        return has_namespace_dir

    def resolved_path(self, path: Path) -> Path:
        resolved = self.resolved_path_cache.get(path)
        if resolved is None:
            if path.is_absolute() and "." not in path.parts and ".." not in path.parts:
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
        context_key = (resolved_roots, resolved_stdlib_root, cpython_test_root)
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
    ) -> ast.AST:
        cache_key = (self.resolved_path(path), filename, target_python.tag)
        if not retain:
            return _parse_source_for_target(
                source,
                filename=filename,
                target_python=target_python,
            )
        tree = self.ast_cache.get(cache_key)
        if tree is not None:
            return tree
        cached_error = self.ast_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            tree = _parse_source_for_target(
                source,
                filename=filename,
                target_python=target_python,
            )
        except SyntaxError as exc:
            self.ast_error_cache[cache_key] = exc
            raise
        self.ast_cache[cache_key] = tree
        return tree

    def collect_imports(
        self,
        path: Path,
        tree: ast.AST,
        *,
        collector: Callable[..., Collection[str]],
        module_name: str | None = None,
        is_package: bool = False,
        import_scan_mode: ImportScanMode = "full",
    ) -> tuple[str, ...]:
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            import_scan_mode,
        )
        cached = self.import_scan_cache.get(cache_key)
        if cached is not None:
            return cached
        imports = collector(
            tree,
            module_name,
            is_package,
            import_scan_mode=import_scan_mode,
        )
        cached_imports = tuple(imports)
        self.import_scan_cache[cache_key] = cached_imports
        return cached_imports

    def uses_runtime_import_protocol(
        self,
        path: Path,
        tree: ast.AST,
        *,
        detector: Callable[..., bool],
        module_name: str | None = None,
        is_package: bool = False,
        import_scan_mode: ImportScanMode = "full",
    ) -> bool:
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            import_scan_mode,
        )
        cached = self.runtime_import_protocol_cache.get(cache_key)
        if cached is not None:
            return cached
        cached = detector(
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
        )
        self.runtime_import_protocol_cache[cache_key] = cached
        return cached


def _has_namespace_dir(module_name: str, roots: list[Path]) -> bool:
    rel = Path(*module_name.split("."))
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            return True
    return False


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
