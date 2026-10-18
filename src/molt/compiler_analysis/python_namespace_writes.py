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
key, ``exec``/``eval`` on its own globals) reports no own-name
bound, and the binding flow then keeps its whole-namespace callback model.
Design: ``docs/design/verified_namespace_premises.md``.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Final, Literal, NamedTuple, TypeAlias, cast, get_args

from molt.python_private_names import (
    python_import_binding,
    resolve_python_private_names,
)
from molt.compiler_analysis.python_builtin_shapes import builtin_method_writes_receiver
from molt.compiler_analysis.python_lexical_scope import (
    LexicalDefinitionNode,
    PythonDependencyAuthority,
    definition_evaluation_regions,
    definition_lexical_regions,
    module_annotation_execution,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    _PythonAstDigestAdmission,
    decode_python_source_key,
    python_node_source_key,
)
from molt.compiler_analysis.python_value_identity import (
    canonical_import_binding_candidates,
)
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

# Persisted summaries carry this; bump it when the recognized syntax changes.
SUMMARY_SCHEMA: Final = 4

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
    # A sys.modules entry changes canonical acquisition, not the mapping object.
    "import_binding",
    # A computed cache key proven disjoint from canonical-provider names.
    "import_binding_disjoint",
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
# The standard import-hook objects and their native container kinds.
_IMPORT_HOOK_OBJECTS: Final[dict[str, Literal["list", "dict"]]] = {
    "sys.modules": "dict",
    "sys.meta_path": "list",
    "sys.path_hooks": "list",
    "sys.path_importer_cache": "dict",
}
# The method an augmented assignment calls on its target first.
_INPLACE_METHODS: Final[dict[type[ast.operator], str]] = {
    ast.Add: "__iadd__",
    ast.Sub: "__isub__",
    ast.Mult: "__imul__",
    ast.MatMult: "__imatmul__",
    ast.Div: "__itruediv__",
    ast.FloorDiv: "__ifloordiv__",
    ast.Mod: "__imod__",
    ast.Pow: "__ipow__",
    ast.LShift: "__ilshift__",
    ast.RShift: "__irshift__",
    ast.BitAnd: "__iand__",
    ast.BitOr: "__ior__",
    ast.BitXor: "__ixor__",
}


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
    source_owner: PythonSourceKey | None = None


def python_namespace_write_payload(write: PythonNamespaceWrite) -> list[object]:
    return [
        write.module,
        write.name,
        write.construct,
        write.line,
        None if write.source_owner is None else list(write.source_owner),
    ]


def decode_python_namespace_write(raw: object) -> PythonNamespaceWrite:
    if not isinstance(raw, list) or len(raw) != 5:
        raise ValueError("invalid foreign namespace write")
    module, name, construct, line, owner = raw
    if (
        module is not None
        and type(module) is not str
        or name is not None
        and type(name) is not str
        or construct not in get_args(PythonNamespaceWriteConstruct)
        or type(line) is not int
        or line < 1
    ):
        raise ValueError("invalid foreign namespace write")
    source_owner = None if owner is None else decode_python_source_key(owner)
    if construct in {"import_binding", "import_binding_disjoint"} and (
        module != "sys.modules"
        or construct == "import_binding_disjoint"
        and name is not None
    ):
        raise ValueError("invalid import binding write")
    if source_owner is not None and source_owner[-1] not in {
        "FunctionDef",
        "AsyncFunctionDef",
        "Lambda",
        "GeneratorExp",
        "TypeAlias",
    }:
        raise ValueError("invalid namespace write source owner")
    return PythonNamespaceWrite(module, name, construct, line, source_owner)


@dataclass(frozen=True, slots=True)
class PythonNamespaceSummary:
    """Writes this module's code performs on module namespaces.

    ``own_names`` lists the globals the module's deferred code may rebind,
    including deferred own-namespace item writes; ``None``
    when the module reflects its own namespace dynamically (first site in
    ``own_reflection_line``). ``foreign_writes`` lists named writes to other
    modules' namespaces in source order, one row per distinct write.
    """

    own_names: frozenset[str] | None
    foreign_writes: tuple[PythonNamespaceWrite, ...]
    own_reflection_line: int = 0


def python_namespace_summary(
    tree: ast.Module,
    *,
    package: str | None = None,
    target_python: tuple[int, int] = _DEFAULT_TARGET_PYTHON_VERSION.feature_version,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> PythonNamespaceSummary:
    """Summarize ``tree``; ``package`` resolves its relative imports.

    Without ``package`` a relative import binds no module reference. The own
    part of the summary never depends on it. An existing read-only AST admission
    owns syntax facts across binding policies and graph scans. Mutable callers
    without that admission recompute; no tree or digest is captured for them.
    """
    if ast_digest_admission is not None:
        ast_digest_admission.resolved_private_tree(tree)
    else:
        resolve_python_private_names(tree)
    return _resolved_python_namespace_summary(
        tree,
        package=package,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    )


def _resolved_python_namespace_summary(
    tree: ast.Module,
    *,
    package: str | None = None,
    target_python: tuple[int, int] = _DEFAULT_TARGET_PYTHON_VERSION.feature_version,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> PythonNamespaceSummary:
    """The syntax projection for an already resolved analysis tree.

    Binding analysis and captured host-source snapshots own private-name
    resolution before reaching here. Do not normalize them a second time or
    require a new AST digest just to query syntax. The public entry above
    normalizes arbitrary caller trees through that same canonical authority.
    """
    context = (target_python, package)
    if ast_digest_admission is not None:
        cached = ast_digest_admission.namespace_summaries.get(context)
        if cached is not None:
            return cached
    collector = _NamespaceWriteCollector(tree, package, target_python)
    collector.collect()
    summary = PythonNamespaceSummary(
        None if collector.own_reflection_line else frozenset(collector.own_names),
        tuple(sorted(collector.writes.values(), key=_source_order)),
        collector.own_reflection_line,
    )
    if ast_digest_admission is not None:
        ast_digest_admission.namespace_summaries[context] = summary
    return summary


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


_LEXICAL_DEFINITIONS: Final = get_args(LexicalDefinitionNode)
_HOOK_ATTRIBUTES: Final = frozenset(
    spelling.rpartition(".")[2] for spelling in _IMPORT_HOOK_OBJECTS
)
_CANDIDATE_CALLS: Final = frozenset(
    {"setattr", "delattr", "vars", "globals", "locals", "exec", "eval"}
)


class _Site(NamedTuple):
    node: ast.AST
    parent: ast.AST | None
    grandparent: ast.AST | None
    module_scope: bool
    source_owner: PythonSourceKey | None
    executing: bool
    callback_deferred: bool


class _StarLookupScope(NamedTuple):
    nodes: tuple[ast.AST, ...]
    bindings: dict[str, bool]
    class_lookup: bool


class _StarLookupRestore(NamedTuple):
    bindings: tuple[tuple[str, bool | None], ...]
    class_lookup: bool


def _star_local_names(
    tree: ast.Module, *, eager_annotations: bool, future_annotations: bool
) -> set[ast.Name]:
    """Find lexical names that module IMPORT_STAR cannot initialize.

    This proves lookup custody, never a receiver's type or value. Explicit
    import references keep their separate conservative classification. Class
    mappings can shadow even a closure cell, so their lookups stay unknown.
    Canonical declarations are visited once per scope; enter/restore events
    give constant-time name lookup without copying enclosing environments.
    """
    authority = PythonDependencyAuthority(
        eager_annotations=eager_annotations,
        future_annotations=future_annotations,
    )
    bindings: dict[str, bool] = {}
    class_lookup = False
    local_names: set[ast.Name] = set()
    pending: list[ast.AST | _StarLookupScope | _StarLookupRestore] = [tree]
    while pending:
        node = pending.pop()
        if isinstance(node, _StarLookupRestore):
            for name, previous in node.bindings:
                if previous is None:
                    del bindings[name]
                else:
                    bindings[name] = previous
            class_lookup = node.class_lookup
            continue
        if isinstance(node, _StarLookupScope):
            pending.append(
                _StarLookupRestore(
                    tuple((name, bindings.get(name)) for name in node.bindings),
                    class_lookup,
                )
            )
            bindings.update(node.bindings)
            class_lookup = node.class_lookup
            pending.extend(reversed(node.nodes))
            continue
        if isinstance(node, _LEXICAL_DEFINITIONS):
            definition = cast(LexicalDefinitionNode, node)
            regions = authority.regions(definition)
            body_bindings = dict.fromkeys(regions.type_parameters, False)
            if regions.kind != "class":
                declarations = authority.declarations(definition)
                body_bindings.update(dict.fromkeys(declarations.bound, False))
                body_bindings.update(dict.fromkeys(declarations.globals, True))
            if regions.body:
                pending.append(
                    _StarLookupScope(
                        regions.body, body_bindings, regions.kind == "class"
                    )
                )
            if regions.annotations:
                pending.append(
                    _StarLookupScope(
                        regions.annotations,
                        dict.fromkeys(regions.type_parameters, False),
                        class_lookup,
                    )
                )
            pending.extend(reversed(regions.enclosing))
            continue
        if isinstance(node, ast.Name):
            if not class_lookup and bindings.get(node.id) is False:
                local_names.add(node)
            continue
        if isinstance(node, ast.Constant):
            continue
        pending.extend(reversed(tuple(ast.iter_child_nodes(node))))
    return local_names


class _NamespaceWriteCollector:
    """One traversal finds candidate sites; classification runs once imports are known."""

    def __init__(
        self, tree: ast.Module, package: str | None, target_python: tuple[int, int]
    ) -> None:
        self.tree = tree
        self.package = package
        self.future, self.eager_annotations = module_annotation_execution(
            tree, target_python=target_python
        )
        # Names an import binds anywhere in the module, flow-insensitively: a
        # parameter that shadows one is read as a module reference, which can
        # only add writes.
        self.references: dict[str, str] = {}
        self.has_star_import = False
        self.star_local_names: set[ast.Name] = set()
        self.own_names: set[str] = set()
        self.own_reflection_line = 0
        self.writes: dict[
            tuple[
                str | None,
                str | None,
                PythonNamespaceWriteConstruct,
                PythonSourceKey | None,
            ],
            PythonNamespaceWrite,
        ] = {}

    def collect(self) -> None:
        sites: list[_Site] = []
        names: list[_Site] = []
        stack = [_Site(self.tree, None, None, True, None, True, False)]
        roots: dict[ast.AST, tuple[PythonSourceKey | None, bool, bool]] = {}
        while stack:
            site = stack.pop()
            node = site.node
            if node in roots:
                owner, scope, callback_deferred = roots.pop(node)
                site = site._replace(
                    source_owner=owner,
                    module_scope=scope,
                    executing=True,
                    callback_deferred=callback_deferred,
                )
            parent, module_scope = site.parent, site.module_scope
            if isinstance(node, ast.Attribute):
                sites.append(site)
            elif isinstance(node, ast.Subscript) and isinstance(
                node.ctx, (ast.Store, ast.Del)
            ):
                sites.append(site)
            elif isinstance(node, ast.Call):
                if isinstance(node.func, ast.Name) and node.func.id in _CANDIDATE_CALLS:
                    sites.append(site)
            elif isinstance(node, ast.Name):
                if isinstance(node.ctx, ast.Load) or (
                    isinstance(parent, ast.AugAssign) and parent.target is node
                ):
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
                        # IMPORT_STAR is a module-initialization store, not a
                        # future callback writer. Binding flow owns its direct
                        # shadowing effects. Its unknown imported references
                        # still need conservative foreign-write classification.
                        self.has_star_import = True
                    elif owner is not None:
                        self.references[alias.asname or alias.name] = (
                            f"{owner}.{alias.name}"
                        )
                continue
            elif isinstance(node, ast.Global):
                if site.callback_deferred:
                    self.own_names.update(node.names)
                continue
            definition = isinstance(node, _LEXICAL_DEFINITIONS)
            if definition and site.executing:
                definition_node = cast(LexicalDefinitionNode, node)
                immediate, deferred = definition_evaluation_regions(
                    definition_node,
                    eager_annotations=self.eager_annotations,
                    future_annotations=self.future,
                )
                lexical = definition_lexical_regions(
                    definition_node,
                    eager_annotations=self.eager_annotations,
                    future_annotations=self.future,
                )
                body = set(lexical.body)
                for child in immediate:
                    # Class statements execute now, in their class namespace.
                    roots[child] = (
                        site.source_owner,
                        False
                        if lexical.kind == "class" and child in body
                        else module_scope,
                        site.callback_deferred,
                    )
                for child in deferred:
                    executable_body = child in body or (
                        isinstance(node, ast.TypeAlias) and child is node.value
                    )
                    # Lazy annotation/bound expression keys are not source-body
                    # jobs. Keep their writes with the enclosing executable
                    # owner until that protocol has real scheduling authority.
                    roots[child] = (
                        python_node_source_key(node)
                        if executable_body
                        else site.source_owner,
                        False,
                        True,
                    )
            for child in ast.iter_child_nodes(node):
                stack.append(
                    _Site(
                        child,
                        node,
                        parent,
                        False if definition else module_scope,
                        site.source_owner,
                        site.executing and not definition,
                        site.callback_deferred,
                    )
                )
        if self.has_star_import:
            self.star_local_names = _star_local_names(
                self.tree,
                eager_annotations=self.eager_annotations,
                future_annotations=self.future,
            )
        for site in sites:
            if isinstance(site[0], ast.Attribute):
                self._visit_attribute(site)
            elif isinstance(site[0], ast.Call):
                self._visit_call(site)
            self._visit_star_hook_mutation(site)
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
        if self.has_star_import:
            for site in names:
                node, parent = site.node, site.parent
                if isinstance(parent, ast.AugAssign) and parent.target is node:
                    self._star_hook_use(site, _INPLACE_METHODS[type(parent.op)])

    def _star_receiver_root(self, node: ast.AST) -> ast.Name | None:
        """Unwrap a possible star receiver, including carried lexical values."""
        if not self.has_star_import:
            return None
        while isinstance(node, (ast.Attribute, ast.Subscript)):
            node = node.value
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "get"
        ):
            return self._star_receiver_root(node.func.value)
        return node if isinstance(node, ast.Name) else None

    def _visit_star_hook_mutation(self, site: _Site) -> None:
        node, parent = site.node, site.parent
        if isinstance(node, ast.Subscript):
            if isinstance(node.ctx, ast.Store):
                method = "__setitem__"
            elif isinstance(node.ctx, ast.Del):
                method = "__delitem__"
            else:
                return
        elif (
            isinstance(node, ast.Attribute)
            and isinstance(parent, ast.Call)
            and parent.func is node
        ):
            method = node.attr
        else:
            return
        if self._star_receiver_root(node.value) is not None:
            self._star_hook_use(
                _Site(
                    node.value,
                    node,
                    parent,
                    site.module_scope,
                    site.source_owner,
                    site.executing,
                    site.callback_deferred,
                ),
                method,
            )

    def _star_hook_use(self, site: _Site, method: str) -> None:
        """A receiver a star import may supply calls ``method`` natively.

        The imported value could be any standard import-hook object, so each
        one whose container kind writes through ``method`` is used as its
        explicit reference would be. The builtin capability authority decides:
        ``append`` cannot change ``sys.modules`` and ``|=`` cannot change
        ``sys.meta_path``. Custom callbacks retain their own source execution;
        calling an unrelated method is not a native hook-container write.
        """
        for spelling, kind in _IMPORT_HOOK_OBJECTS.items():
            if builtin_method_writes_receiver(kind, method):
                self._import_hook_use(site, spelling)

    def _record(
        self,
        module: str | None,
        name: str | None,
        construct: PythonNamespaceWriteConstruct,
        site: _Site,
        *,
        node: ast.AST | None = None,
    ) -> None:
        if not site.executing:
            return
        # The traversal is not in source order; keep each write's first site.
        key = (module, name, construct, site.source_owner)
        line = getattr(site.node if node is None else node, "lineno", 0)
        known = self.writes.get(key)
        if known is None or line < known.line:
            self.writes[key] = PythonNamespaceWrite(
                module, name, construct, line, site.source_owner
            )

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

        star_root = self._star_receiver_root(node)
        if star_root is not None and star_root not in self.star_local_names:
            # Star exports can supply module aliases or shadow explicit
            # imports. The receiver's module is unproved, but its written
            # member remains named when the syntax names it.
            return True, None
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
        node, parent = site.node, site.parent
        assert isinstance(node, ast.Attribute)
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            is_module, module = self._module_receiver(node.value)
            if is_module:
                # ``__class__`` and ``__dict__`` replacement change every name.
                name = None if node.attr in {"__class__", "__dict__"} else node.attr
                self._record(module, name, "attribute", site)
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
                    self._record(module, name, "setattr", site, node=parent)
        elif node.attr in _HOOK_ATTRIBUTES:
            spelling = self._reference(node)
            if spelling in _IMPORT_HOOK_OBJECTS:
                self._import_hook_use(site, spelling)

    def _visit_call(self, site: _Site) -> None:
        if not site.executing:
            return
        node, module_scope = site.node, site.module_scope
        assert isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
        func = node.func.id
        if func in {"setattr", "delattr"} and node.args:
            is_module, module = self._module_receiver(node.args[0])
            if is_module:
                name = _literal_string(node.args[1]) if len(node.args) > 1 else None
                self._record(module, name, "setattr", site)
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

        node, parent, grandparent = site.node, site.parent, site.grandparent
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
            self._record(module, name, "namespace", site)

    def _own_namespace_use(self, site: _Site) -> None:
        writes, name = self._item_write(site)
        if not writes:
            return
        if not site.callback_deferred:
            value, parent, grandparent = self._consumer(site)
            if (
                isinstance(parent, ast.Subscript)
                and parent.value is value
                and isinstance(parent.ctx, (ast.Store, ast.Del))
            ) or (
                isinstance(parent, ast.Attribute)
                and parent.value is value
                and isinstance(grandparent, ast.Call)
                and grandparent.func is parent
                and builtin_method_writes_receiver("dict", parent.attr)
            ):
                # An immediate native mapping operation changes the current
                # binding state; it does not grant future callbacks the same
                # write. Binding flow owns that operation. Mapping escapes
                # still expose every name, including in module initialization.
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
            if spelling == "sys.modules":
                self._record_import_binding(site, parent.slice)
                return
        elif isinstance(parent, ast.Attribute) and parent.value is value:
            if isinstance(grandparent, ast.Call) and grandparent.func is parent:
                if not (
                    builtin_method_writes_receiver("list", parent.attr)
                    or builtin_method_writes_receiver("dict", parent.attr)
                ):
                    return
                if spelling == "sys.modules":
                    if parent.attr in _ITEM_WRITE_METHODS:
                        self._record_import_binding(
                            site,
                            grandparent.args[0] if grandparent.args else None,
                        )
                    elif parent.attr in {"update", "__ior__", "__init__"}:
                        for argument in grandparent.args:
                            self._record_import_binding_mapping(site, argument)
                        for keyword in grandparent.keywords:
                            if keyword.arg is None:
                                self._record_import_binding_mapping(site, keyword.value)
                            else:
                                self._record_import_binding(site, keyword.arg)
                    else:
                        self._record_import_binding(site, None)
                    return
        elif self._reads_whole(value, parent):
            return
        if spelling == "sys.modules":
            self._record_import_binding(site, None)
            return
        module, _, name = spelling.rpartition(".")
        self._record(module, name, "import_hooks", site)

    def _record_import_binding_mapping(self, site: _Site, mapping: ast.expr) -> None:
        if isinstance(mapping, ast.Dict):
            for key in mapping.keys:
                self._record_import_binding(site, key)
        else:
            self._record_import_binding(site, None)

    def _record_import_binding(self, site: _Site, key: ast.expr | str | None) -> None:
        name: str | None
        if isinstance(key, str):
            name = key
        elif isinstance(key, ast.Constant) and isinstance(key.value, str):
            name = key.value
        elif isinstance(key, ast.JoinedStr) and not any(
            isinstance(part, ast.FormattedValue) for part in key.values
        ):
            candidates = canonical_import_binding_candidates(key)
            name = candidates[0] if candidates else None
        else:
            if not canonical_import_binding_candidates(key):
                self._record("sys.modules", None, "import_binding_disjoint", site)
                return
            name = None
        self._record("sys.modules", name, "import_binding", site)
