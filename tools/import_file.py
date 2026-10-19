from __future__ import annotations

import ast
import importlib.machinery
import importlib.util
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType


# Tools import this module only as ``tools.import_file`` (HF-221). A bare
# ``import_file`` import would build a second binder with its own state.
if __name__ != "tools.import_file":
    raise ImportError(
        f"import the repository binder as tools.import_file, not {__name__!r}"
    )


def _load_package_import_custody(path: Path) -> ModuleType:
    """Load one neutral authority; never import it through a foreign parent."""
    name = "_molt_package_import_custody"
    selected = path.resolve(strict=True)
    if name in sys.modules:
        loaded = sys.modules[name]
        spec = getattr(loaded, "__spec__", None)
        loader = getattr(spec, "loader", None)
        if (
            type(loaded) is not ModuleType
            or getattr(loaded, "__name__", None) != name
            or getattr(loaded, "__package__", None) != ""
            or getattr(loaded, "__file__", None) != str(selected)
            or getattr(spec, "name", None) != name
            or getattr(spec, "origin", None) != str(selected)
            or type(loader) is not importlib.machinery.SourceFileLoader
            or loader.name != name
            or loader.path != str(selected)
            or getattr(loaded, "__loader__", None) is not loader
        ):
            raise ImportError(
                f"package import custody already loaded from another authority; "
                f"selected {selected}, loaded {getattr(loaded, '__file__', None)!r}"
            )
        return loaded
    spec = importlib.util.spec_from_file_location(name, selected)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load selected package import custody: {selected}")
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[name] = loaded
    try:
        spec.loader.exec_module(loaded)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    return loaded


_package_import_custody = _load_package_import_custody(
    Path(__file__).resolve().parents[1] / "src/molt/package_import_custody.py"
)


@dataclass(frozen=True, slots=True)
class _MissingModuleBinding:
    pass


_MISSING = _MissingModuleBinding()


_TOOLS_ROOT = Path(__file__).resolve().parent


def tool_module_name(path: str | Path) -> str | None:
    """The one module name a repository tool file executes as, else None.

    A Python file under ``tools/`` is a module of the ``tools`` package and has
    exactly that identity (HF-221); loading it by path under another name
    builds a second copy of its state and types.
    """
    resolved = Path(path).resolve()
    if resolved.suffix != ".py" or not resolved.is_relative_to(_TOOLS_ROOT):
        return None
    parts = resolved.relative_to(_TOOLS_ROOT.parent).with_suffix("").parts
    if parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts) if all(part.isidentifier() for part in parts) else None


_MOLT_ROOT = _TOOLS_ROOT.parent / "src" / "molt"
# Molt's stdlib sources are modules of compiled programs, not of the host
# ``molt`` package; tests load them by path under names of their own.
_MOLT_STDLIB_ROOT = _MOLT_ROOT / "stdlib"
# A Molt host module that may execute under a neutral name declares it with
# this module-level literal (HF-223); the guard keeps no list of them.
_NEUTRAL_AUTHORITY_DECLARATION = "__molt_neutral_authority__"


def molt_module_name(path: str | Path) -> str | None:
    """The one module name a Molt host source file executes as, else None."""
    resolved = Path(path).resolve()
    if (
        resolved.suffix != ".py"
        or not resolved.is_relative_to(_MOLT_ROOT)
        or resolved.is_relative_to(_MOLT_STDLIB_ROOT)
    ):
        return None
    parts = resolved.relative_to(_MOLT_ROOT.parent).with_suffix("").parts
    if parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts) if all(part.isidentifier() for part in parts) else None


def _declares_neutral_authority(path: Path) -> bool:
    """Whether a Molt source declares itself loadable under a neutral name."""
    for node in ast.parse(path.read_bytes(), filename=str(path)).body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == _NEUTRAL_AUTHORITY_DECLARATION
            for target in node.targets
        ):
            return isinstance(node.value, ast.Constant) and node.value.value is True
    return False


def require_repository_identity(module_name: str, path: str | Path) -> None:
    """Refuse to load a repository host module under any name but its own.

    A file under ``tools/`` is ``tools.<path>`` (HF-221) and a Molt host
    source is ``molt.<path>`` (HF-223): loading either by path under another
    name builds a second copy of its state and types. A Molt source that
    declares ``__molt_neutral_authority__ = True`` may run under a neutral
    name, because it is selected by exact file before any ``molt`` import.
    """
    canonical = tool_module_name(path)
    owner = "tool"
    if canonical is None:
        canonical = molt_module_name(path)
        owner = "Molt"
        if (
            canonical is not None
            and module_name != canonical
            and _declares_neutral_authority(Path(path).resolve())
        ):
            return
    if canonical is not None and module_name != canonical:
        raise ImportError(
            f"{path} is the repository {owner} module {canonical}; "
            f"import {canonical} instead of loading it by path as {module_name!r}"
        )


def _repository_root(source_file: str | Path) -> Path:
    source = Path(source_file).resolve()
    for candidate in (source.parent, *source.parents):
        if (candidate / "pyproject.toml").is_file() and (candidate / "tools").is_dir():
            return candidate
    raise RuntimeError(f"cannot locate Molt repository root from {source}")


def _canonical_namespace_prototype(package: str, expected: Path) -> ModuleType:
    search_locations = [str(expected)]

    def find_selected_namespace(
        fullname: str,
        _parent_path: object,
    ) -> importlib.machinery.ModuleSpec:
        selected = importlib.machinery.PathFinder.find_spec(
            fullname,
            [str(expected.parent)],
        )
        if selected is None:
            raise ImportError(f"selected namespace source disappeared: {expected}")
        return selected

    loader = importlib.machinery.NamespaceLoader(
        package,
        search_locations,
        find_selected_namespace,
    )
    spec = importlib.machinery.ModuleSpec(package, loader, is_package=True)
    spec.submodule_search_locations = search_locations
    return importlib.util.module_from_spec(spec)


def _install_namespace_metadata(
    loaded: ModuleType,
    prototype: ModuleType,
) -> None:
    loaded.__package__ = prototype.__package__
    loaded.__loader__ = prototype.__loader__
    loaded.__spec__ = prototype.__spec__
    loaded.__path__ = prototype.__path__


def _publish_namespace(package: str, prototype: ModuleType) -> None:
    sys.modules[package] = prototype
    prefix = f"{package}."
    for name, loaded in tuple(sys.modules.items()):
        child = name.removeprefix(prefix)
        if name.startswith(prefix) and "." not in child and loaded is not None:
            setattr(prototype, child, loaded)


def bind_repository_imports(source_file: str | Path) -> Path:
    """Admit the package family and repair only an unbound search path."""

    root = _repository_root(source_file)
    source_root = root / "src"
    namespace_updates = (
        *_package_import_custody.repository_namespace_updates("tools", root / "tools"),
        *_package_import_custody.repository_namespace_updates(
            "molt", source_root / "molt"
        ),
    )
    # NamespaceLoader observes its parent package when constructed. Validate
    # the whole family first, then restore parents before nested namespaces.
    for name, loaded, expected in sorted(
        namespace_updates, key=lambda update: (update[0].count("."), update[0])
    ):
        prototype = _canonical_namespace_prototype(name, expected)
        if loaded is None:
            _publish_namespace(name, prototype)
        else:
            _install_namespace_metadata(loaded, prototype)
    selected_roots = (root.resolve(), source_root.resolve())
    tools_root = (root / "tools").resolve()
    admitted_prefix = set()
    prefix_open = True
    needs_repair = False
    remaining = []
    for item in sys.path:
        # A by-path launch puts the script's own directory first. Tools
        # import each other only as `tools.<module>`, so no directory inside
        # tools/ belongs on the path; removing it also stops tools/profile.py
        # from shadowing the stdlib `profile` module.
        if isinstance(item, str):
            resolved = Path(item or ".").resolve()
            if resolved in selected_roots:
                if item != str(resolved):
                    needs_repair = True
                    prefix_open = False
                elif prefix_open:
                    admitted_prefix.add(resolved)
                continue
            if resolved.is_relative_to(tools_root):
                needs_repair = True
                prefix_open = False
                continue
        prefix_open = False
        remaining.append(item)
    if needs_repair or admitted_prefix != set(selected_roots):
        # Repeated library binding preserves an admitted root/src order. A
        # repair removes unsafe aliases and publishes both roots together,
        # without a transient window exposing only ambient search directories.
        sys.path[:] = [str(source_root), str(root), *remaining]
    return root


def load_module_from_path(module_name: str, path: Path) -> ModuleType:
    """Execute *path* with normal import transaction semantics.

    ``module_from_spec`` does not register the module. Register it before
    execution because dataclasses, pickling, and recursive imports consult
    ``sys.modules`` while the body runs. A failed body restores the exact
    prior binding.
    """

    require_repository_identity(module_name, path)
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load module {module_name!r} from {path}")
    module = importlib.util.module_from_spec(spec)
    previous = sys.modules.get(module_name, _MISSING)
    sys.modules[module_name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        if isinstance(previous, _MissingModuleBinding):
            sys.modules.pop(module_name, None)
        else:
            sys.modules[module_name] = previous
        raise
    return module


def load_sibling_package_module_from_path(
    module_name: str,
    path: Path,
) -> ModuleType:
    """Load one file inside a transactional synthetic sibling package.

    This gives relative imports normal package semantics without adding an
    ambient directory to ``sys.path``. Parent and descendant bindings are
    restored together when the module body fails.
    """

    package_name, separator, _child_name = module_name.rpartition(".")
    if not separator or not package_name:
        raise ValueError("sibling package module name must include a parent package")
    package_path = path.resolve().parent
    previous_package = sys.modules.get(package_name, _MISSING)
    sibling_prefix = f"{package_name}."
    previous_siblings = {
        name: module
        for name, module in sys.modules.items()
        if name.startswith(sibling_prefix)
    }
    package = ModuleType(package_name)
    package.__package__ = package_name
    setattr(package, "__path__", [str(package_path)])
    package_spec = importlib.machinery.ModuleSpec(
        package_name,
        loader=None,
        is_package=True,
    )
    package_spec.submodule_search_locations = [str(package_path)]
    package.__spec__ = package_spec
    sys.modules[package_name] = package
    try:
        return load_module_from_path(module_name, path)
    except BaseException:
        for name in tuple(sys.modules):
            if name.startswith(sibling_prefix) and name not in previous_siblings:
                sys.modules.pop(name, None)
        sys.modules.update(previous_siblings)
        if isinstance(previous_package, _MissingModuleBinding):
            sys.modules.pop(package_name, None)
        else:
            sys.modules[package_name] = previous_package
        raise
