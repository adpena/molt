"""Which code can rebind a module's globals outside its own statements (HF-226).

Molt's dynamic-execution policy (spec 0216) admits scoped mutation of a module
namespace and excludes unrestricted ``exec``/``eval``, arbitrary runtime
monkeypatching and unrestricted reflection. Under that contract a module's
globals change only through:

- the module's own statements, including deferred code that declares a name
  ``global`` and own-namespace item writes with literal keys;
- named writes from other modules: an attribute store, ``setattr`` or
  ``__dict__``/``vars`` item write on a module reference;
- the import system, which binds a package's submodules as its attributes.

This module is the one authority for that syntax. It is conservative by
spelling: it never narrows on a construct it cannot see. A module whose own code
reflects its namespace dynamically (an escaping ``globals()``, a non-literal
key, ``exec``/``eval`` on its own globals, star imports) reports no own-name
bound, and the binding flow then keeps its whole-namespace callback model.
Design: ``docs/design/verified_namespace_premises.md``.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Final, Literal, TypeAlias

from molt.python_private_names import python_import_binding

# Persisted summaries carry this; bump it when the recognized syntax changes.
SUMMARY_SCHEMA: Final = 1

PythonNamespaceWriteConstruct: TypeAlias = Literal[
    # ``module.name = value``, ``del module.name``, ``module.name += value``
    "attribute",
    # ``setattr``/``delattr`` or ``__setattr__``/``__delattr__`` on a module
    "setattr",
    # an item write on ``module.__dict__``, ``vars(module)`` or
    # ``sys.modules[...]``-reached namespaces
    "namespace",
    # a mutation of ``sys.modules``, ``sys.meta_path``, ``sys.path_hooks`` or
    # ``sys.path_importer_cache``: it changes what later imports bind
    "import_hooks",
]

# Calls that only read their argument. A module or namespace passed to any other
# call is not a write: generic writers on module objects are monkeypatching,
# outside the contract.
_READ_METHODS: Final = frozenset(
    {
        "get",
        "keys",
        "values",
        "items",
        "copy",
        "__contains__",
        "__getitem__",
        "__len__",
        "__iter__",
    }
)
_ITEM_WRITE_METHODS: Final = frozenset(
    {"pop", "setdefault", "__setitem__", "__delitem__"}
)
# Builtins that read an argument without keeping it.
_WHOLE_READ_CALLS: Final = frozenset(
    {
        "bool",
        "dict",
        "enumerate",
        "frozenset",
        "getattr",
        "hasattr",
        "hash",
        "id",
        "isinstance",
        "iter",
        "len",
        "list",
        "print",
        "repr",
        "reversed",
        "set",
        "sorted",
        "str",
        "tuple",
        "type",
    }
)
_IMPORT_HOOK_OBJECTS: Final = frozenset(
    {"sys.modules", "sys.meta_path", "sys.path_hooks", "sys.path_importer_cache"}
)
_LIST_READ_METHODS: Final = frozenset({"index", "count", "copy", "__contains__"})


@dataclass(frozen=True, slots=True)
class PythonNamespaceWrite:
    """One named write to another module's namespace.

    ``module`` is the receiver's dotted spelling made absolute: the import
    binding plus its attribute chain. It names a module only when a module of
    that name exists; a consumer resolves it against its own module topology.
    ``None`` means a namespace reached through ``sys.modules`` by a computed
    key. ``name`` is ``None`` when the written name is computed.
    """

    module: str | None
    name: str | None
    construct: PythonNamespaceWriteConstruct
    line: int


@dataclass(frozen=True, slots=True)
class PythonNamespaceSummary:
    """Writes this module's code performs on module namespaces.

    ``own_names`` lists the globals the module's deferred code and
    own-namespace item writes may rebind, so callbacks can rebind them; ``None``
    when the module reflects its own namespace dynamically (first site in
    ``own_reflection_line``). ``foreign_writes`` lists named writes to other
    modules' namespaces in source order, one row per distinct write.
    """

    own_names: frozenset[str] | None
    foreign_writes: tuple[PythonNamespaceWrite, ...]
    own_reflection_line: int = 0


def python_namespace_summary(
    tree: ast.Module, *, package: str | None = None
) -> PythonNamespaceSummary:
    """Summarize ``tree``; ``package`` resolves its relative imports.

    Without ``package`` a relative import binds no module reference. The own
    part of the summary never depends on it.
    """

    collector = _NamespaceWriteCollector(tree, package)
    collector.collect()
    return PythonNamespaceSummary(
        None if collector.own_reflection_line else frozenset(collector.own_names),
        tuple(sorted(collector.writes.values(), key=_source_order)),
        collector.own_reflection_line,
    )


def _source_order(write: PythonNamespaceWrite) -> tuple[int, str, str, str]:
    return (write.line, write.module or "", write.name or "", write.construct)


def python_package_name(module: str, *, is_package: bool) -> str:
    """The ``__package__`` a module of this identity executes with."""

    return module if is_package else module.rpartition(".")[0]


def _absolute_module(name: str, level: int, package: str | None) -> str | None:
    if level == 0:
        return name
    if not package:
        return None
    parts = package.split(".")
    if level - 1 >= len(parts):
        return None
    base = ".".join(parts[: len(parts) - (level - 1)])
    return f"{base}.{name}" if name else base


def _literal_string(node: ast.expr | None) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _literal_module_name(node: ast.expr | None) -> str | None:
    """A ``sys.modules`` key that names a module, else None (any module)."""
    key = _literal_string(node)
    if key is None or not all(part.isidentifier() for part in key.split(".")):
        return None
    return key


_DEFERRED_SCOPES: Final = (
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.Lambda,
    ast.ClassDef,
)
_HOOK_ATTRIBUTES: Final = frozenset(
    spelling.rpartition(".")[2] for spelling in _IMPORT_HOOK_OBJECTS
)
_CANDIDATE_CALLS: Final = frozenset(
    {"setattr", "delattr", "vars", "globals", "locals", "exec", "eval"}
)
# (node, parent, grandparent, in module scope)
_Site: TypeAlias = tuple[ast.AST, ast.AST | None, ast.AST | None, bool]


class _NamespaceWriteCollector:
    """One traversal finds candidate sites; classification runs once imports are known."""

    def __init__(self, tree: ast.Module, package: str | None) -> None:
        self.tree = tree
        self.package = package
        # Names an import binds anywhere in the module, flow-insensitively: a
        # parameter that shadows one is read as a module reference, which can
        # only add writes.
        self.references: dict[str, str] = {}
        self.own_names: set[str] = set()
        self.own_reflection_line = 0
        self.writes: dict[
            tuple[str | None, str | None, PythonNamespaceWriteConstruct],
            PythonNamespaceWrite,
        ] = {}

    def collect(self) -> None:
        sites: list[_Site] = []
        names: list[_Site] = []
        stack: list[_Site] = [(self.tree, None, None, True)]
        while stack:
            site = stack.pop()
            node, parent, _grandparent, module_scope = site
            if isinstance(node, ast.Attribute):
                sites.append(site)
            elif isinstance(node, ast.Call):
                if isinstance(node.func, ast.Name) and node.func.id in _CANDIDATE_CALLS:
                    sites.append(site)
            elif isinstance(node, ast.Name):
                if isinstance(node.ctx, ast.Load):
                    names.append(site)
                continue
            elif isinstance(node, ast.Import):
                for alias in node.names:
                    self.references[python_import_binding(alias)] = (
                        alias.name if alias.asname else alias.name.partition(".")[0]
                    )
                continue
            elif isinstance(node, ast.ImportFrom):
                owner = _absolute_module(node.module or "", node.level, self.package)
                for alias in node.names:
                    if alias.name == "*":
                        self._reflects_own_namespace(node)
                    elif owner is not None:
                        self.references[alias.asname or alias.name] = (
                            f"{owner}.{alias.name}"
                        )
                continue
            elif isinstance(node, ast.Global):
                if not module_scope:
                    self.own_names.update(node.names)
                continue
            # Comprehensions stay in module scope: PEP 709 inlines them, and
            # reading one's ``locals()`` as the module's can only add names.
            child_scope = module_scope and not isinstance(node, _DEFERRED_SCOPES)
            for child in ast.iter_child_nodes(node):
                stack.append((child, node, parent, child_scope))
        for site in sites:
            if isinstance(site[0], ast.Attribute):
                self._visit_attribute(site)
            else:
                self._visit_call(site)
        hook_names = {
            name
            for name, spelling in self.references.items()
            if spelling in _IMPORT_HOOK_OBJECTS
        }
        if hook_names:
            # ``from sys import modules`` binds the hook object itself.
            for site in names:
                node = site[0]
                assert isinstance(node, ast.Name)
                if node.id in hook_names:
                    self._import_hook_use(site, self.references[node.id])

    def _record(
        self,
        module: str | None,
        name: str | None,
        construct: PythonNamespaceWriteConstruct,
        node: ast.AST,
    ) -> None:
        # The traversal is not in source order; keep each write's first site.
        key = (module, name, construct)
        line = getattr(node, "lineno", 0)
        known = self.writes.get(key)
        if known is None or line < known.line:
            self.writes[key] = PythonNamespaceWrite(module, name, construct, line)

    def _reflects_own_namespace(self, node: ast.AST) -> None:
        line = getattr(node, "lineno", 0) or 1
        if not self.own_reflection_line or line < self.own_reflection_line:
            self.own_reflection_line = line

    def _reference(self, node: ast.AST) -> str | None:
        """The dotted spelling of a module reference expression, if any."""

        if isinstance(node, ast.Name):
            if node.id == "__builtins__":
                return "builtins"
            return self.references.get(node.id)
        if isinstance(node, ast.Attribute):
            base = self._reference(node.value)
            return f"{base}.{node.attr}" if base is not None else None
        return None

    def _module_receiver(self, node: ast.AST) -> tuple[bool, str | None]:
        """Whether ``node`` evaluates to a module, and its spelling if known.

        A ``sys.modules`` entry is a module whose name the key computes.
        """

        reference = self._reference(node)
        if reference is not None:
            return True, reference
        if (
            isinstance(node, ast.Subscript)
            and isinstance(node.ctx, ast.Load)
            and self._reference(node.value) == "sys.modules"
        ):
            return True, _literal_module_name(node.slice)
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "get"
            and self._reference(node.func.value) == "sys.modules"
            and node.args
        ):
            return True, _literal_module_name(node.args[0])
        return False, None

    def _visit_attribute(self, site: _Site) -> None:
        node, parent, grandparent, _module_scope = site
        assert isinstance(node, ast.Attribute)
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            is_module, module = self._module_receiver(node.value)
            if is_module:
                # ``__class__`` and ``__dict__`` replacement change every name.
                name = None if node.attr in {"__class__", "__dict__"} else node.attr
                self._record(module, name, "attribute", node)
            return
        if node.attr == "__dict__":
            is_module, module = self._module_receiver(node.value)
            if is_module:
                self._namespace_use(site, module)
        elif node.attr in {"__setattr__", "__delattr__"}:
            if isinstance(parent, ast.Call) and parent.func is node:
                is_module, module = self._module_receiver(node.value)
                if is_module:
                    name = _literal_string(parent.args[0]) if parent.args else None
                    self._record(module, name, "setattr", parent)
        elif node.attr in _HOOK_ATTRIBUTES:
            spelling = self._reference(node)
            if spelling in _IMPORT_HOOK_OBJECTS:
                self._import_hook_use(site, spelling)

    def _visit_call(self, site: _Site) -> None:
        node, _parent, _grandparent, module_scope = site
        assert isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
        func = node.func.id
        if func in {"setattr", "delattr"} and node.args:
            is_module, module = self._module_receiver(node.args[0])
            if is_module:
                name = _literal_string(node.args[1]) if len(node.args) > 1 else None
                self._record(module, name, "setattr", node)
        elif func == "vars" and node.args:
            is_module, module = self._module_receiver(node.args[0])
            if is_module:
                self._namespace_use(site, module)
        elif func in {"globals", "vars", "locals"} and not node.args:
            if func == "globals" or module_scope:
                self._own_namespace_use(site)
        elif func in {"exec", "eval"} and len(node.args) < 2:
            # Code run on this module's own globals may bind any name.
            self._reflects_own_namespace(node)

    @staticmethod
    def _consumer(site: _Site) -> tuple[ast.AST, ast.AST | None, ast.AST | None]:
        """The expression as its consumer sees it, that consumer, and its parent."""

        node, parent, grandparent, _module_scope = site
        if isinstance(parent, ast.Starred):
            # A starred operand's consumer is the call or display around it;
            # neither reads the value as a receiver.
            return parent, grandparent, None
        return node, parent, grandparent

    @staticmethod
    def _reads_whole(value: ast.AST, parent: ast.AST | None) -> bool:
        """Membership, iteration and copying read a mapping or list whole."""

        return isinstance(parent, (ast.Compare, ast.comprehension, ast.For)) or (
            isinstance(parent, ast.Call)
            and isinstance(parent.func, ast.Name)
            and parent.func.id in _WHOLE_READ_CALLS
            and value in parent.args
        )

    def _item_write(self, site: _Site) -> tuple[bool, str | None]:
        """Classify one use of a namespace mapping: (writes, literal name)."""

        value, parent, grandparent = self._consumer(site)
        if isinstance(parent, ast.Subscript) and parent.value is value:
            if isinstance(parent.ctx, ast.Load):
                return False, None
            return True, _literal_string(parent.slice)
        if isinstance(parent, ast.Attribute) and parent.value is value:
            if isinstance(grandparent, ast.Call) and grandparent.func is parent:
                if parent.attr in _READ_METHODS:
                    return False, None
                if parent.attr in _ITEM_WRITE_METHODS:
                    return True, (
                        _literal_string(grandparent.args[0])
                        if grandparent.args
                        else None
                    )
            return True, None
        if self._reads_whole(value, parent):
            return False, None
        # Any other use hands the mapping on: every name may change.
        return True, None

    def _namespace_use(self, site: _Site, module: str | None) -> None:
        writes, name = self._item_write(site)
        if writes:
            self._record(module, name, "namespace", site[0])

    def _own_namespace_use(self, site: _Site) -> None:
        writes, name = self._item_write(site)
        if not writes:
            return
        if name is None:
            self._reflects_own_namespace(site[0])
        else:
            self.own_names.add(name)

    def _import_hook_use(self, site: _Site, spelling: str) -> None:
        value, parent, grandparent = self._consumer(site)
        if isinstance(parent, ast.Subscript) and parent.value is value:
            if isinstance(parent.ctx, ast.Load):
                return
        elif isinstance(parent, ast.Attribute) and parent.value is value:
            if (
                isinstance(grandparent, ast.Call)
                and grandparent.func is parent
                and parent.attr in _READ_METHODS | _LIST_READ_METHODS
            ):
                return
        elif self._reads_whole(value, parent):
            return
        module, _, name = spelling.rpartition(".")
        self._record(module, name, "import_hooks", site[0])
