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
from collections.abc import Callable
from dataclasses import dataclass, replace
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
    function_parameter_names,
    module_annotation_execution,
    python_scope_declarations,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    _PythonAstDigestAdmission,
    decode_python_source_key,
    python_node_source_key,
)
from molt.compiler_analysis.python_import_redirects import (
    PythonImportKeyBound,
    decode_python_import_key_bound,
    python_import_key,
)
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

# Persisted summaries carry this; bump it when the recognized syntax changes.
SUMMARY_SCHEMA: Final = 16

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
    # A write or delete of one ``sys.modules`` entry (``module`` is
    # ``"sys.modules"``): ``name`` is the exact key, else ``key_bound`` bounds
    # it, else the key is unbounded. It redirects that row and its dotted
    # descendants (``python_import_redirects``).
    "import_binding",
    # An import under a handler that can swallow its failure (an ``except``
    # clause or a ``with`` body around it): ``module`` is the imported name,
    # else ``key_bound`` bounds it, else it is unbounded. A swallowed body
    # failure re-runs the body on the next import (static cells, C4).
    "guarded_import",
    # A read of a namespace mapping by identity, not a write: ``M.__dict__``,
    # ``vars(M)``, a ``sys.modules`` entry's ``__dict__``, a function's
    # ``__globals__`` or a frame's ``f_globals`` (``module`` None: any
    # module's), used other than by a literal dunder key. The mapping then
    # must hold every global, so the target keeps its dictionary namespace
    # (static module cells, call_anatomy.md 4.5a). ``name`` is always None.
    "reflection",
    # A read of a code object (``__code__``, ``f_code``, ``gi_code``,
    # ``cr_code``, ``ag_code``) used other than for ``co_*`` metadata.
    # ``FunctionType`` can run that code over another globals mapping, so
    # no function's global reads are its module's (call_anatomy.md 4.5a).
    # ``module`` and ``name`` are always None.
    "code",
    # A named attribute store or delete (``x.name = value``, a literal
    # ``setattr``/``delattr``) on a receiver that is not syntactically a module
    # reference. ``module`` is None. It writes a module only through a module
    # value that escaped (``module_escape``); the graph joins the two.
    "member_store",
    # The same with a computed name (``setattr(x, name, value)``,
    # ``x.__setattr__(name, value)``) on a receiver that is not a module
    # reference. ``module`` and ``name`` are None. It may write any
    # attribute, ``__class__`` included, so no instance's class is a
    # program constant while one is activated (call_anatomy.md 4.5,
    # "Implementation (field slots)").
    "computed_member_store",
    # A module value that escapes: a ``sys.modules[...]``/``sys.modules.get``,
    # ``importlib.import_module`` or import-name value that is stored,
    # returned, aliased past this function's reads, or passed to a call that
    # may keep it. ``module`` is its spelling, None for a computed key.
    "module_escape",
    # The same, for this module's own value (``sys.modules[__name__]``).
    # ``module`` and ``name`` are None; the graph owner names the source.
    "own_module_escape",
]

# Calls that only read their argument. A module value passed to any other call
# escapes, and a named attribute store anywhere in the program may then write
# that module (``module_escape``, ``member_store``): a writer the analysis
# accounts for (call_anatomy.md 4.5a), not monkeypatching outside the contract.
# Only a computed attribute name on an unknown receiver stays reflection.
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
    source_site: PythonSourceKey | None = None
    # The bound on a computed ``import_binding`` key or ``guarded_import``
    # name; never dropped (bedrock §13, B2a).
    key_bound: PythonImportKeyBound | None = None


def python_namespace_write_payload(write: PythonNamespaceWrite) -> list[object]:
    return [
        write.module,
        write.name,
        write.construct,
        write.line,
        None if write.source_owner is None else list(write.source_owner),
        None if write.source_site is None else list(write.source_site),
        None if write.key_bound is None else write.key_bound.payload(),
    ]


def decode_python_namespace_write(raw: object) -> PythonNamespaceWrite:
    if not isinstance(raw, list) or len(raw) != 7:
        raise ValueError("invalid foreign namespace write")
    module, name, construct, line, owner, site, bound = raw
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
    key_bound = None if bound is None else decode_python_import_key_bound(bound)
    if construct == "import_binding" and (
        module != "sys.modules" or key_bound is not None and name is not None
    ):
        raise ValueError("invalid import binding write")
    if construct == "guarded_import" and (
        name is not None or key_bound is not None and module is not None
    ):
        raise ValueError("invalid guarded import")
    if key_bound is not None and construct not in {"import_binding", "guarded_import"}:
        raise ValueError("invalid import key bound")
    if source_owner is not None and source_owner[-1] not in {
        "FunctionDef",
        "AsyncFunctionDef",
        "Lambda",
        "GeneratorExp",
        "TypeAlias",
    }:
        raise ValueError("invalid namespace write source owner")
    source_site = None if site is None else decode_python_source_key(site)
    if source_site is not None and construct != "import_binding":
        raise ValueError("invalid namespace cache write site")
    if construct == "reflection" and name is not None:
        raise ValueError("invalid namespace reflection")
    if construct == "code" and (module is not None or name is not None):
        raise ValueError("invalid code reflection")
    if construct == "member_store" and (module is not None or name is None):
        raise ValueError("invalid member store")
    if construct == "computed_member_store" and (
        module is not None or name is not None
    ):
        raise ValueError("invalid computed member store")
    if construct == "module_escape" and name is not None:
        raise ValueError("invalid module escape")
    if construct == "own_module_escape" and (module is not None or name is not None):
        raise ValueError("invalid own module escape")
    return PythonNamespaceWrite(
        module, name, construct, line, source_owner, source_site, key_bound
    )


@dataclass(frozen=True, slots=True)
class PythonNamespaceSummary:
    """Writes this module's code performs on module namespaces.

    ``own_names`` lists the globals the module's deferred code may rebind,
    including deferred own-namespace item writes; ``None``
    when the module reflects its own namespace dynamically (first site in
    ``own_reflection_line``). ``foreign_writes`` lists named writes to other
    modules' namespaces in source order, one row per distinct write, and the
    ``reflection`` rows that reach a namespace mapping by identity.
    ``own_namespace_line`` is the first line whose code reaches this module's
    own mapping (``globals()``, a module-scope ``vars()``/``locals()``, or
    ``exec``/``eval`` on its globals) by any use, read or write; 0 for none.
    """

    own_names: frozenset[str] | None
    foreign_writes: tuple[PythonNamespaceWrite, ...]
    own_reflection_line: int = 0
    own_namespace_line: int = 0


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
        collector.own_namespace_line,
    )
    if ast_digest_admission is not None:
        ast_digest_admission.namespace_summaries[context] = summary
    return summary


def _source_order(
    write: PythonNamespaceWrite,
) -> tuple[int, str, str, str, PythonSourceKey, PythonSourceKey, tuple[str, str, int]]:
    absent: PythonSourceKey = (-1, -1, -1, -1, "")
    bound = write.key_bound
    return (
        write.line,
        write.module or "",
        write.name or "",
        write.construct,
        write.source_owner or absent,
        write.source_site or absent,
        ("", "", -1) if bound is None else (bound.prefix, bound.suffix, bound.minimum),
    )


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


def _dunder_setattr_name(method: str, call: ast.Call) -> str | None:
    """The literal attribute name of a ``__setattr__``/``__delattr__`` call.

    A bound call (``x.__setattr__(name, value)``) names it first; an unbound
    one (``object.__setattr__(x, name, value)``) second. A setter call that
    passes three arguments, or a deleter two, is unbound.
    """

    unbound = len(call.args) >= (3 if method == "__setattr__" else 2)
    index = 1 if unbound else 0
    return _literal_string(call.args[index]) if len(call.args) > index else None


def _literal_string(node: ast.expr | None) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _is_dunder(name: str) -> bool:
    return len(name) > 4 and name.startswith("__") and name.endswith("__")


def own_namespace_literal_delete(statement: ast.stmt) -> str | None:
    """The global a ``globals().pop("name", <constant>)`` statement deletes.

    With its value discarded, the statement equals ``del name`` that tolerates
    an unbound name (HF-844). The frontend lowers it to that module global
    delete at module scope, where the builtin ``globals`` is proven, so the
    statement reaches no namespace mapping. One predicate serves the frontend
    and this authority, so the two never disagree on the spelling.
    """
    if not isinstance(statement, ast.Expr):
        return None
    call = statement.value
    if not (
        isinstance(call, ast.Call)
        and isinstance(call.func, ast.Attribute)
        and call.func.attr == "pop"
        and not call.keywords
        and len(call.args) == 2
        and isinstance(call.args[1], ast.Constant)
    ):
        return None
    receiver = call.func.value
    if not (
        isinstance(receiver, ast.Call)
        and isinstance(receiver.func, ast.Name)
        and receiver.func.id == "globals"
        and not receiver.args
        and not receiver.keywords
    ):
        return None
    return _literal_string(call.args[0])


def module_binds_name(tree: ast.Module, name: str) -> bool:
    """Whether any scope of ``tree`` may bind ``name``: a store, ``del``,
    definition, parameter, handler, pattern, import alias or star import."""
    for node in ast.walk(tree):
        if isinstance(node, ast.Name):
            bound = node.id if not isinstance(node.ctx, ast.Load) else None
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            bound = node.name
        elif isinstance(node, ast.arg):
            bound = node.arg
        elif isinstance(node, (ast.ExceptHandler, ast.MatchAs, ast.MatchStar)):
            bound = node.name
        elif isinstance(node, ast.MatchMapping):
            bound = node.rest
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            if any(
                alias.name == "*" or python_import_binding(alias) == name
                for alias in node.names
            ):
                return True
            continue
        else:
            continue
        if bound == name:
            return True
    return False


def _literal_module_name(node: ast.expr | None) -> str | None:
    """A ``sys.modules`` key that names a module, else None (any module)."""
    key = _literal_string(node)
    if key is None or not all(part.isidentifier() for part in key.split(".")):
        return None
    return key


_LEXICAL_DEFINITIONS: Final = get_args(LexicalDefinitionNode)
# Builtins that read a module argument without keeping it.
_MODULE_READ_CALLS: Final = frozenset(
    {
        "callable",
        "dir",
        "getattr",
        "hasattr",
        "hash",
        "id",
        "isinstance",
        "print",
        "repr",
        "str",
        "type",
    }
)
# Alias and conditional-expression hops followed before a value counts as kept.
_ESCAPE_ALIAS_DEPTH: Final = 4
_HOOK_ATTRIBUTES: Final = frozenset(
    spelling.rpartition(".")[2] for spelling in _IMPORT_HOOK_OBJECTS
)
_CANDIDATE_CALLS: Final = frozenset(
    {
        "setattr",
        "delattr",
        "vars",
        "globals",
        "locals",
        "exec",
        "eval",
        "getattr",
        "__import__",
    }
)
# Attributes whose value is some module's namespace mapping, whichever module
# defined the function or runs the frame.
_GLOBALS_ATTRIBUTES: Final = frozenset({"__globals__", "f_globals"})
# Builtins whose zero-argument call returns the calling frame's namespace. As a
# value (``g = globals``, ``builtins.globals``, ``getattr(builtins, name)``) one
# returns the namespace of whichever module calls it, so it reaches every
# module's mapping (HF-872).
_NAMESPACE_BUILTINS: Final = frozenset({"globals", "vars", "locals"})
# Attributes whose value is a code object, which ``FunctionType`` can run over
# another globals mapping.
_CODE_ATTRIBUTES: Final = frozenset(
    {"__code__", "f_code", "gi_code", "cr_code", "ag_code"}
)
# A code object's attributes that can yield a code object: nested code in its
# constants and copies.
_CODE_YIELDING_ATTRIBUTES: Final = frozenset({"co_consts", "replace", "__replace__"})


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


class _MappingFlow:
    """Which names a function writes through: one module's parameter and
    local write summaries, to a fixed point over the module's own functions.

    A name is written when it is item-written, has a non-read method called on
    it, is passed to a parameter that is written, is declared ``global`` or
    ``nonlocal``, is captured by a nested function, lambda or class body that
    does not rebind it, or escapes any other way (assigned elsewhere, returned,
    stored). A callee resolves only to an undecorated top-level ``def`` that the
    module binds exactly once; any other callee counts as a write. A call of the
    builtin ``eval``/``exec`` (a name the module never binds) does not write,
    because spec 0216 excludes source evaluation. Comprehensions share their
    function's scope. Unproven means written: the premise falls back, never
    binds wrongly.
    """

    _SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)

    def __init__(self, tree: ast.Module) -> None:
        self.parents: dict[ast.AST, ast.AST] = {}
        for node in ast.walk(tree):
            for child in ast.iter_child_nodes(node):
                self.parents[child] = node
        bindings: dict[str, int] = {}
        declared_global: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Global):
                declared_global.update(node.names)
            scope = self._scope(node)
            if scope is not None:
                continue
            for name in self._bound_names(node):
                bindings[name] = bindings.get(name, 0) + 1
        self.functions: dict[str, ast.FunctionDef | ast.AsyncFunctionDef] = {}
        for node in tree.body:
            if (
                isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                and not node.decorator_list
                and bindings.get(node.name) == 1
                and node.name not in declared_global
            ):
                self.functions[node.name] = node
        # Module-level names that shadow the evaluation builtins.
        self.rebound_builtins = {
            name
            for name in ("eval", "exec")
            if bindings.get(name) or name in declared_global
        }
        # (function node, name) pairs proven written, for every function of
        # the module, methods and nested ones included; grown to a fixed point.
        self.all_functions = [
            node
            for node in ast.walk(tree)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        ]
        self.written: set[tuple[ast.AST, str]] = set()
        self._solve()

    @staticmethod
    def _bound_names(node: ast.AST) -> list[str]:
        """Names one node binds in its own scope."""
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            return [node.name]
        if isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
            return [node.id]
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            return [python_import_binding(alias) for alias in node.names]
        if isinstance(node, ast.ExceptHandler) and node.name:
            return [node.name]
        if isinstance(node, (ast.MatchAs, ast.MatchStar)) and node.name:
            return [node.name]
        if isinstance(node, ast.MatchMapping) and node.rest:
            return [node.rest]
        return []

    def _scope(self, node: ast.AST) -> ast.AST | None:
        """The nearest enclosing function, lambda or class of ``node``."""
        current = self.parents.get(node)
        while current is not None:
            if isinstance(current, self._SCOPES):
                return current
            current = self.parents.get(current)
        return None

    def _enclosing(
        self, node: ast.AST
    ) -> ast.FunctionDef | ast.AsyncFunctionDef | None:
        scope = self._scope(node)
        return (
            scope
            if isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef))
            else None
        )

    @staticmethod
    def _params(
        function: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
    ) -> list[str]:
        arguments = function.args
        return [
            arg.arg
            for arg in (
                *arguments.posonlyargs,
                *arguments.args,
                *arguments.kwonlyargs,
                *filter(None, (arguments.vararg, arguments.kwarg)),
            )
        ]

    def _scope_binds(self, scope: ast.AST, name: str) -> bool:
        """Whether a nested scope binds ``name`` itself (a different variable)."""
        if isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            if name in self._params(scope):
                return True
        for node in ast.walk(scope):
            if node is scope or self._scope(node) is not scope:
                continue
            if isinstance(node, (ast.Global, ast.Nonlocal)) and name in node.names:
                return False
            if name in self._bound_names(node):
                return True
        return False

    def _call_writes(self, call: ast.Call, value: ast.AST) -> bool:
        """Whether passing ``value`` to ``call`` may write through it."""
        func = call.func
        if isinstance(func, ast.Name) and func.id in {"eval", "exec"}:
            caller = self._scope(call)
            shadowed = func.id in self.rebound_builtins or (
                caller is not None and self._scope_binds(caller, func.id)
            )
            if not shadowed:
                return False
        if not (isinstance(func, ast.Name) and func.id in self.functions):
            return True
        caller = self._scope(call)
        if caller is not None and self._scope_binds(caller, func.id):
            return True
        callee = self.functions[func.id]
        params = [arg.arg for arg in (*callee.args.posonlyargs, *callee.args.args)]
        names: list[str] = []
        for index, argument in enumerate(call.args):
            if isinstance(argument, ast.Starred):
                return True
            if argument is value:
                if index >= len(params):
                    return True
                names.append(params[index])
        for keyword in call.keywords:
            if keyword.value is value:
                if keyword.arg is None or keyword.arg not in params:
                    return True
                names.append(keyword.arg)
        return any((callee, name) in self.written for name in names)

    def _use_writes(self, value: ast.AST, parent: ast.AST | None) -> bool:
        """Whether one use of a mapping-valued expression may write it."""
        if isinstance(parent, ast.Expr):
            return False
        if isinstance(parent, ast.Subscript) and parent.value is value:
            return not isinstance(parent.ctx, ast.Load)
        if isinstance(parent, ast.Attribute) and parent.value is value:
            grandparent = self.parents.get(parent)
            return not (
                isinstance(grandparent, ast.Call)
                and grandparent.func is parent
                and parent.attr in _READ_METHODS
            )
        if isinstance(parent, (ast.Compare, ast.comprehension, ast.For)):
            return False
        if isinstance(parent, ast.Call) and parent.func is not value:
            if (
                isinstance(parent.func, ast.Name)
                and parent.func.id in _WHOLE_READ_CALLS
            ):
                return False
            return self._call_writes(parent, value)
        if isinstance(parent, ast.Assert):
            return False
        if (
            isinstance(parent, ast.Assign)
            and parent.value is value
            and len(parent.targets) == 1
            and isinstance(parent.targets[0], ast.Name)
        ):
            function = self._enclosing(parent)
            return (
                function is None
                or self._scope(parent) is not function
                or (function, parent.targets[0].id) in self.written
            )
        return True

    def _name_writes(
        self, function: ast.FunctionDef | ast.AsyncFunctionDef, name: str
    ) -> bool:
        for node in ast.walk(function):
            if isinstance(node, (ast.Global, ast.Nonlocal)) and name in node.names:
                if self._scope(node) is function:
                    return True
            if not (
                isinstance(node, ast.Name)
                and node.id == name
                and isinstance(node.ctx, ast.Load)
            ):
                continue
            scope = self._scope(node)
            if scope is not function:
                # A nested scope that captures the name may write it later.
                inner = scope
                while inner is not None and inner is not function:
                    if self._scope_binds(inner, name):
                        break
                    inner = self._scope(inner)
                else:
                    return True
                continue
            if self._use_writes(node, self.parents.get(node)):
                return True
        return False

    def _solve(self) -> None:
        candidates = [
            (function, name)
            for function in self.all_functions
            for name in {
                *self._params(function),
                *(
                    node.id
                    for node in ast.walk(function)
                    if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)
                ),
            }
        ]
        changed = True
        while changed:
            changed = False
            for function, name in candidates:
                key = (function, name)
                if key not in self.written and self._name_writes(function, name):
                    self.written.add(key)
                    changed = True

    def escape_writes(self, value: ast.AST, parent: ast.AST | None) -> bool:
        return self._use_writes(value, parent)

    def escape_keys(
        self, value: ast.AST, parent: ast.AST | None
    ) -> frozenset[str] | None:
        """The keys one escape of a mapping may write: empty when no use
        writes, the literal keys when the mapping only reaches a function
        local whose every write names a literal key, else ``None``."""
        if not self._use_writes(value, parent):
            return frozenset()
        if not (
            isinstance(parent, ast.Assign)
            and parent.value is value
            and len(parent.targets) == 1
            and isinstance(parent.targets[0], ast.Name)
        ):
            return None
        function = self._enclosing(parent)
        if function is None or self._scope(parent) is not function:
            return None
        return self._alias_keys(function, parent.targets[0].id)

    def _alias_keys(
        self, function: ast.FunctionDef | ast.AsyncFunctionDef, name: str
    ) -> frozenset[str] | None:
        """Literal keys written through local ``name``, or ``None`` when a
        use is unbounded: a rebinding, a capture, a declaration, a computed
        key or any escape (spec: namespace premise, own-key bound)."""
        keys: set[str] = set()
        assignments = 0
        for node in ast.walk(function):
            if isinstance(node, (ast.Global, ast.Nonlocal)) and name in node.names:
                return None
            if not (isinstance(node, ast.Name) and node.id == name):
                continue
            if self._scope(node) is not function:
                return None
            if not isinstance(node.ctx, ast.Load):
                assignments += 1
                if assignments > 1 or not isinstance(node.ctx, ast.Store):
                    return None
                continue
            parent = self.parents.get(node)
            if isinstance(parent, ast.Subscript) and parent.value is node:
                if isinstance(parent.ctx, ast.Load):
                    continue
                key = _literal_string(parent.slice)
                if key is None:
                    return None
                keys.add(key)
                continue
            if isinstance(parent, ast.Attribute) and parent.value is node:
                call = self.parents.get(parent)
                if isinstance(call, ast.Call) and call.func is parent:
                    if parent.attr in _READ_METHODS:
                        continue
                    if parent.attr in _ITEM_WRITE_METHODS and call.args:
                        key = _literal_string(call.args[0])
                        if key is not None:
                            keys.add(key)
                            continue
                return None
            if self._use_writes(node, parent):
                return None
        return frozenset(keys)


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
        self.own_namespace_line = 0
        self.mapping_flow: _MappingFlow | None = None
        self.parents: dict[ast.AST, ast.AST] | None = None
        self.globals_rebound: bool | None = None
        self.writes: dict[
            tuple[
                str | None,
                str | None,
                PythonNamespaceWriteConstruct,
                PythonSourceKey | None,
                PythonSourceKey | None,
                PythonImportKeyBound | None,
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
                    self._guarded_import(site, alias.name)
                continue
            elif isinstance(node, ast.ImportFrom):
                owner = _absolute_module(node.module or "", node.level, self.package)
                if owner is None:
                    self._guarded_import(site, None)
                for alias in node.names:
                    if owner is not None:
                        # ``from P import x`` may import the submodule ``P.x``.
                        self._guarded_import(
                            site,
                            owner if alias.name == "*" else f"{owner}.{alias.name}",
                        )
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
        for site in names:
            node = site[0]
            assert isinstance(node, ast.Name)
            direct_call = isinstance(site.parent, ast.Call) and site.parent.func is node
            if (
                node.id in _NAMESPACE_BUILTINS
                and node.id not in self.references
                and not direct_call
                and self._resolves_to_builtin(node)
            ):
                # A direct call is classified by ``_visit_call``.
                self._namespace_builtin_use(node, site, node.id)
            spelling = self.references.get(node.id)
            if spelling is None or spelling in _IMPORT_HOOK_OBJECTS:
                continue
            module_name, _, member = spelling.rpartition(".")
            if module_name == "builtins" and member in _NAMESPACE_BUILTINS:
                # ``from builtins import globals [as g]``.
                if not (direct_call and node.id in _CANDIDATE_CALLS):
                    self._namespace_builtin_use(node, site, member)
                continue
            if spelling == "builtins":
                owner = self._tree_parents().get(node)
                if (
                    isinstance(owner, ast.Attribute)
                    and owner.value is node
                    and isinstance(owner.ctx, ast.Load)
                    and owner.attr in _NAMESPACE_BUILTINS
                ):
                    self._namespace_builtin_use(owner, site, owner.attr)
                    continue
            if spelling == "importlib.import_module":
                if isinstance(site.parent, ast.Call) and site.parent.func is node:
                    call = site.parent
                    self._module_value_use(
                        call, site, key=call.args[0] if call.args else None
                    )
                    self._guarded_import(
                        site, call.args[0] if call.args else None, node=call
                    )
                continue
            # A submodule reached by attribute (``os.path``) is a module
            # value too; follow the load chain to the value its consumer sees.
            value: ast.AST = node
            parents = self._tree_parents()
            while (
                isinstance(owner := parents.get(value), ast.Attribute)
                and owner.value is value
                and isinstance(owner.ctx, ast.Load)
                and not (
                    isinstance(call := parents.get(owner), ast.Call)
                    and call.func is owner
                )
            ):
                spelling = f"{spelling}.{owner.attr}"
                value = owner
            if spelling in _IMPORT_HOOK_OBJECTS:
                # The import-hook containers are classified as such below.
                continue
            self._module_value_use(value, site, module=spelling)
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
        source_site: PythonSourceKey | None = None,
        key_bound: PythonImportKeyBound | None = None,
    ) -> None:
        if not site.executing:
            return
        # The traversal is not in source order; keep each write's first site.
        key = (module, name, construct, site.source_owner, source_site, key_bound)
        line = getattr(site.node if node is None else node, "lineno", 0)
        known = self.writes.get(key)
        if known is None or line < known.line:
            self.writes[key] = PythonNamespaceWrite(
                module, name, construct, line, site.source_owner, source_site, key_bound
            )

    def _member_store(self, name: str | None, site: _Site) -> None:
        """An attribute write on a receiver that is not a module reference."""
        if name is None:
            self._record(None, None, "computed_member_store", site)
        else:
            self._record(None, name, "member_store", site)

    def _reflects_own_namespace(self, node: ast.AST) -> None:
        line = getattr(node, "lineno", 0) or 1
        if not self.own_reflection_line or line < self.own_reflection_line:
            self.own_reflection_line = line
        self._reaches_own_namespace(node)

    def _reaches_own_namespace(self, node: ast.AST) -> None:
        line = getattr(node, "lineno", 0) or 1
        if not self.own_namespace_line or line < self.own_namespace_line:
            self.own_namespace_line = line

    def _reflection_use(self, site: _Site, module: str | None) -> None:
        """A namespace mapping reached by identity at ``site``: a reflection
        row unless every use reads only literal dunder keys, which a module's
        dictionary always holds (static module cells, call_anatomy.md 4.5a)."""
        if not self._dunder_only_use(*self._consumer(site)):
            self._record(module, None, "reflection", site)

    def _code_use(self, site: _Site) -> None:
        """A code object read at ``site``: a code row unless every use reads
        only its ``co_*`` metadata, which cannot run it."""
        if not self._code_metadata_use(*self._consumer(site)):
            self._record(None, None, "code", site)

    def _code_metadata_use(
        self,
        value: ast.AST,
        parent: ast.AST | None,
        grandparent: ast.AST | None,
        *,
        alias: bool = True,
    ) -> bool:
        """Whether this use of a code object cannot hand it to ``FunctionType``:
        a metadata attribute read (any ``co_*`` but ``co_consts``, or
        ``__class__``), an identity test, a discarded value, a return from an
        accessor named for a code attribute (its caller's read of that name is
        classified at the caller), or (once) a function-local alias every use
        of which is one of those."""
        if isinstance(parent, ast.Expr):
            return True
        if isinstance(parent, ast.Attribute) and parent.value is value:
            return isinstance(parent.ctx, ast.Load) and (
                parent.attr == "__class__"
                or parent.attr.startswith("co_")
                and parent.attr not in _CODE_YIELDING_ATTRIBUTES
            )
        if isinstance(parent, ast.Return):
            scope = self._tree_parents().get(parent)
            while scope is not None and not isinstance(
                scope, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)
            ):
                scope = self._tree_parents().get(scope)
            return (
                isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef))
                and scope.name in _CODE_ATTRIBUTES
            )
        if isinstance(parent, ast.Compare):
            return all(isinstance(op, (ast.Is, ast.IsNot)) for op in parent.ops)
        if isinstance(parent, (ast.If, ast.While, ast.IfExp)) and parent.test is value:
            return True
        if isinstance(parent, ast.UnaryOp) and isinstance(parent.op, ast.Not):
            return True
        if (
            isinstance(parent, ast.Call)
            and isinstance(parent.func, ast.Name)
            and parent.func.id == "getattr"
            and len(parent.args) >= 2
            and parent.args[0] is value
        ):
            attribute = _literal_string(parent.args[1])
            return (
                attribute is not None
                and attribute.startswith("co_")
                and attribute not in _CODE_YIELDING_ATTRIBUTES
            )
        if (
            alias
            and isinstance(parent, ast.Assign)
            and parent.value is value
            and len(parent.targets) == 1
            and isinstance(parent.targets[0], ast.Name)
        ):
            # Only the alias's reads can pass the object on, so a local that
            # several code reads store is as safe as one stored once.
            return self._alias_only_used_as(
                parent,
                parent.targets[0].id,
                self._code_metadata_use,
                single_store=False,
            )
        return False

    def _resolves_to_builtin(self, node: ast.Name) -> bool:
        """Whether the load ``node`` may read the builtin of its name.

        A function scope that binds the name, or declares it ``nonlocal``,
        shadows the builtin; so does a module-level binding. Class bodies and
        comprehensions count as binding nothing, which can only report a
        builtin where a local is read: a lost cell, never a wrong result."""
        name = node.id
        parents = self._tree_parents()
        child: ast.AST = node
        scope = parents.get(node)
        while scope is not None:
            if isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef)) and any(
                child is statement for statement in scope.body
            ):
                declarations = python_scope_declarations(
                    scope.body,
                    function_parameter_names(scope.args),
                    eager_annotations=self.eager_annotations,
                )
                if name in declarations.globals:
                    break
                if name in declarations.bound or name in declarations.nonlocals:
                    return False
            elif isinstance(scope, ast.Lambda) and child is scope.body:
                if name in function_parameter_names(scope.args):
                    return False
            child, scope = scope, parents.get(scope)
        module = python_scope_declarations(
            self.tree.body, eager_annotations=self.eager_annotations
        )
        return name not in module.bound

    def _namespace_builtin_use(self, value: ast.AST, site: _Site, builtin: str) -> None:
        """``value`` evaluates to the ``builtin`` namespace builtin. Called with
        no arguments, it reaches its own module's mapping as a direct call
        does; any other use reaches every module's mapping (HF-872)."""
        parents = self._tree_parents()
        call = parents.get(value)
        if isinstance(call, ast.Call) and call.func is value:
            if call.args or call.keywords:
                if builtin == "vars":
                    # ``vars(obj)`` returns ``obj``'s mapping, a module's too.
                    self._record(None, None, "reflection", site)
                return
            if builtin == "globals" or site.module_scope:
                call_parent = parents.get(call)
                call_site = site._replace(
                    node=call,
                    parent=call_parent,
                    grandparent=None
                    if call_parent is None
                    else parents.get(call_parent),
                )
                self._reaches_own_namespace(call)
                self._own_namespace_use(call_site)
            return
        self._record(None, None, "reflection", site)

    def _tree_parents(self) -> dict[ast.AST, ast.AST]:
        if self.parents is None:
            self.parents = {
                child: node
                for node in ast.walk(self.tree)
                for child in ast.iter_child_nodes(node)
            }
        return self.parents

    def _module_value_use(
        self,
        value: ast.AST,
        site: _Site,
        *,
        key: ast.expr | None = None,
        module: str | None = None,
    ) -> None:
        """Record a module value that escapes at ``value`` (``module_escape``).

        ``key`` is a ``sys.modules``/``import_module`` key; ``module`` an import
        binding's spelling. ``__name__`` names this module's own value.
        """
        if not self._value_escapes(value, 0):
            return
        if module is not None:
            self._record(module, None, "module_escape", site)
        elif isinstance(key, ast.Name) and key.id == "__name__":
            self._record(None, None, "own_module_escape", site)
        else:
            self._record(_literal_module_name(key), None, "module_escape", site)

    def _value_escapes(self, value: ast.AST, depth: int) -> bool:
        """Whether a module value may reach code that keeps or writes it.

        An attribute access on it (a direct receiver's named store is recorded
        where it occurs), a comparison, a truth test, a discarded value and a
        read-only builtin call keep it here; a store or ``setattr`` through an
        alias, and ``vars``, do not. A local alias keeps it when every load of the alias
        in its function does; any other use escapes.
        """
        parents = self._tree_parents()
        parent = parents.get(value)
        if isinstance(parent, ast.Attribute) and parent.value is value:
            # A store through an alias writes the module where no module
            # reference names it; a direct receiver's store is recorded.
            return depth > 0 and isinstance(parent.ctx, (ast.Store, ast.Del))
        if isinstance(parent, (ast.Expr, ast.Compare, ast.Assert)):
            return False
        if isinstance(parent, ast.Call) and parent.func is value:
            return False
        if isinstance(parent, (ast.If, ast.While)) and parent.test is value:
            return False
        if isinstance(parent, ast.IfExp):
            return parent.test is not value and (
                depth >= _ESCAPE_ALIAS_DEPTH or self._value_escapes(parent, depth + 1)
            )
        if isinstance(parent, ast.BoolOp):
            return depth >= _ESCAPE_ALIAS_DEPTH or self._value_escapes(
                parent, depth + 1
            )
        if isinstance(parent, ast.UnaryOp) and isinstance(parent.op, ast.Not):
            return False
        if (
            isinstance(parent, ast.Call)
            and parent.func is not value
            and value in parent.args
            and isinstance(parent.func, ast.Name)
            and parent.func.id in _MODULE_READ_CALLS
        ):
            return False
        if (
            isinstance(parent, ast.Assign)
            and parent.value is value
            and len(parent.targets) == 1
            and isinstance(parent.targets[0], ast.Name)
            and depth < _ESCAPE_ALIAS_DEPTH
        ):
            return self._alias_escapes(parent.targets[0].id, parent, depth + 1)
        return True

    def _alias_escapes(self, name: str, binding: ast.AST, depth: int) -> bool:
        """Whether any load of local alias ``name`` lets its module escape.

        The alias must be a function local: a module-scope alias is a global
        any code may read, and a captured alias outlives the function.
        """
        parents = self._tree_parents()
        scope = parents.get(binding)
        while scope is not None and not isinstance(
            scope, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.Module)
        ):
            if isinstance(scope, ast.ClassDef):
                return True
            scope = parents.get(scope)
        if not isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return True
        for node in ast.walk(scope):
            if isinstance(node, (ast.Global, ast.Nonlocal)) and name in node.names:
                return True
            if not (isinstance(node, ast.Name) and node.id == name):
                continue
            owner = parents.get(node)
            while owner is not None and not isinstance(
                owner, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)
            ):
                owner = parents.get(owner)
            if owner is not scope:
                # A nested scope captures or shadows it: either may keep it.
                return True
            if isinstance(node.ctx, ast.Load) and self._value_escapes(node, depth):
                return True
        return False

    def _dunder_only_use(
        self,
        value: ast.AST,
        parent: ast.AST | None,
        grandparent: ast.AST | None,
        *,
        alias: bool = True,
    ) -> bool:
        """Whether this use of a mapping reads nothing but literal dunder
        keys: a subscript load, ``get``/``__contains__``/``__getitem__`` or
        ``in`` test with a literal dunder key, an identity test, an
        ``isinstance`` check, a discarded value, or (once) a function-local
        alias every use of which is one of those."""

        def dunder(node: ast.AST | None) -> bool:
            key = _literal_string(node) if isinstance(node, ast.expr) else None
            return key is not None and _is_dunder(key)

        if isinstance(parent, ast.Expr):
            return True
        if isinstance(parent, ast.Subscript) and parent.value is value:
            return isinstance(parent.ctx, ast.Load) and dunder(parent.slice)
        if isinstance(parent, ast.Attribute) and parent.value is value:
            return (
                isinstance(grandparent, ast.Call)
                and grandparent.func is parent
                and parent.attr in {"get", "__contains__", "__getitem__"}
                and bool(grandparent.args)
                and dunder(grandparent.args[0])
            )
        if isinstance(parent, ast.Compare):
            if all(isinstance(op, (ast.Is, ast.IsNot)) for op in parent.ops):
                return True
            return (
                len(parent.ops) == 1
                and isinstance(parent.ops[0], (ast.In, ast.NotIn))
                and parent.comparators[0] is value
                and dunder(parent.left)
            )
        if (
            isinstance(parent, ast.Call)
            and isinstance(parent.func, ast.Name)
            and parent.func.id == "isinstance"
            and parent.args
            and parent.args[0] is value
        ):
            return True
        if (
            alias
            and isinstance(parent, ast.Assign)
            and parent.value is value
            and len(parent.targets) == 1
            and isinstance(parent.targets[0], ast.Name)
        ):
            return self._alias_only_used_as(
                parent, parent.targets[0].id, self._dunder_only_use
            )
        return False

    def _alias_only_used_as(
        self,
        assign: ast.Assign,
        name: str,
        use: Callable[..., bool],
        *,
        single_store: bool = True,
    ) -> bool:
        """Whether a local bound by ``assign`` (once, when ``single_store``) is
        read only by ``use``."""
        parents = self._tree_parents()
        scope = parents.get(assign)
        while scope is not None and not isinstance(
            scope, (ast.FunctionDef, ast.AsyncFunctionDef)
        ):
            if isinstance(scope, (ast.ClassDef, ast.Lambda, ast.Module)):
                return False
            scope = parents.get(scope)
        if scope is None:
            return False
        stores = 0
        for node in ast.walk(scope):
            if isinstance(node, (ast.Global, ast.Nonlocal)) and name in node.names:
                return False
            if not isinstance(node, ast.Name) or node.id != name:
                continue
            if not isinstance(node.ctx, ast.Load):
                stores += 1
                continue
            parent = parents.get(node)
            if not use(
                node, parent, parents.get(parent) if parent else None, alias=False
            ):
                return False
        return stores == 1 or not single_store and stores > 0

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
            else:
                self._record(None, node.attr, "member_store", site)
            return
        if (
            node.attr == "import_module"
            and self._reference(node.value) == "importlib"
            and isinstance(parent, ast.Call)
            and parent.func is node
        ):
            self._module_value_use(
                parent, site, key=parent.args[0] if parent.args else None
            )
            self._guarded_import(
                site, parent.args[0] if parent.args else None, node=parent
            )
        if node.attr == "__dict__":
            is_module, module = self._module_receiver(node.value)
            if is_module:
                self._namespace_use(site, module)
        elif node.attr in _GLOBALS_ATTRIBUTES:
            self._reflection_use(site, None)
        elif node.attr in _CODE_ATTRIBUTES:
            self._code_use(site)
        elif node.attr in {"__setattr__", "__delattr__"}:
            if isinstance(parent, ast.Call) and parent.func is node:
                is_module, module = self._module_receiver(node.value)
                if is_module:
                    name = _literal_string(parent.args[0]) if parent.args else None
                    self._record(module, name, "setattr", site, node=parent)
                else:
                    self._member_store(_dunder_setattr_name(node.attr, parent), site)
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
        if func == "getattr" and len(node.args) >= 2:
            attribute = _literal_string(node.args[1])
            if attribute in _GLOBALS_ATTRIBUTES:
                self._reflection_use(site, None)
            elif attribute in _CODE_ATTRIBUTES:
                self._code_use(site)
            elif attribute == "__dict__":
                is_module, module = self._module_receiver(node.args[0])
                if is_module:
                    self._reflection_use(site, module)
            elif attribute is None or attribute in _NAMESPACE_BUILTINS:
                is_module, module = self._module_receiver(node.args[0])
                if is_module and module == "builtins":
                    # A namespace builtin read by name (HF-872).
                    self._record(None, None, "reflection", site)
        elif func in {"setattr", "delattr"} and node.args:
            is_module, module = self._module_receiver(node.args[0])
            name = _literal_string(node.args[1]) if len(node.args) > 1 else None
            if is_module:
                self._record(module, name, "setattr", site)
            else:
                self._member_store(name, site)
        elif func == "vars" and node.args:
            is_module, module = self._module_receiver(node.args[0])
            if is_module:
                self._namespace_use(site, module)
        elif func in {"globals", "vars", "locals"} and not node.args:
            if func == "globals" and self._literal_own_delete(site):
                # Lowered as a module global delete: no mapping is reached.
                return
            if func == "globals" or module_scope:
                self._reaches_own_namespace(node)
                self._own_namespace_use(site)
        elif func == "__import__" and self._resolves_to_builtin(node.func):
            self._guarded_import(site, node.args[0] if node.args else None)
        elif func in {"exec", "eval"} and len(node.args) < 2:
            # Code run on this module's own globals may bind any name.
            self._reflects_own_namespace(node)

    def _literal_own_delete(self, site: _Site) -> bool:
        """Whether this ``globals()`` call is the receiver of a module-scope
        ``globals().pop("name", <constant>)`` statement
        (``own_namespace_literal_delete``) and no scope rebinds ``globals``."""
        if not site.module_scope or site.callback_deferred:
            return False
        method = site.parent
        if not (
            isinstance(method, ast.Attribute)
            and method.value is site.node
            and isinstance(site.grandparent, ast.Call)
            and site.grandparent.func is method
        ):
            return False
        statement = self._tree_parents().get(site.grandparent)
        if not (
            isinstance(statement, ast.stmt)
            and own_namespace_literal_delete(statement) is not None
        ):
            return False
        if self.globals_rebound is None:
            self.globals_rebound = module_binds_name(self.tree, "globals")
        return not self.globals_rebound

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
        if isinstance(parent, ast.Expr):
            # Discarding the mapping neither publishes it nor writes a name.
            return False, None
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
        self._reflection_use(site, module)
        writes, name = self._item_write(site)
        if writes and name is None and not self._escape_writes(site):
            # The mapping escapes into a local or a call argument that no
            # resolved callee writes through (`_MappingFlow`).
            return
        if writes:
            self._record(module, name, "namespace", site)

    def _escape_writes(self, site: _Site) -> bool:
        """Whether a foreign namespace mapping that escapes at ``site`` may be
        written: a local alias or call argument is a write unless the
        parameter summaries prove no resolved callee writes through it."""
        if self.mapping_flow is None:
            self.mapping_flow = _MappingFlow(self.tree)
        value, parent, _grandparent = self._consumer(site)
        return self.mapping_flow.escape_writes(value, parent)

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
        if name is not None:
            self.own_names.add(name)
            return
        keys = self._escape_keys(site)
        if keys is None:
            self._reflects_own_namespace(site[0])
        else:
            # A function local that only reads the mapping or writes literal
            # keys bounds the reflection to those names.
            self.own_names.update(keys)

    def _escape_keys(self, site: _Site) -> frozenset[str] | None:
        if self.mapping_flow is None:
            self.mapping_flow = _MappingFlow(self.tree)
        value, parent, _grandparent = self._consumer(site)
        return self.mapping_flow.escape_keys(value, parent)

    def _import_hook_use(self, site: _Site, spelling: str) -> None:
        value, parent, grandparent = self._consumer(site)
        if isinstance(parent, ast.Subscript) and parent.value is value:
            if isinstance(parent.ctx, ast.Load):
                if spelling == "sys.modules":
                    self._module_value_use(parent, site, key=parent.slice)
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
                    if spelling == "sys.modules" and parent.attr == "get":
                        self._module_value_use(
                            grandparent,
                            site,
                            key=grandparent.args[0] if grandparent.args else None,
                        )
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
        value, parent, grandparent = self._consumer(site)
        operation = (
            parent
            if isinstance(parent, ast.Subscript) and parent.value is value
            else grandparent
            if isinstance(grandparent, ast.Call) and grandparent.func is parent
            else site.node
        )
        source_site = python_node_source_key(operation)
        spelled = python_import_key(key)
        self._record(
            "sys.modules",
            spelled if isinstance(spelled, str) else None,
            "import_binding",
            site,
            source_site=source_site,
            key_bound=spelled if isinstance(spelled, PythonImportKeyBound) else None,
        )

    def _guarded(self, node: ast.AST) -> bool:
        """Whether a handler in ``node``'s own scope can swallow its failure:
        ``node`` is in the body of a ``try`` with an ``except`` clause, or in
        the body of a ``with`` (a context manager's ``__exit__`` may suppress).
        A caller's handler is outside this scope; the caller's own import
        site, if any, is classified where it stands."""
        parents = self._tree_parents()
        child = node
        current = parents.get(node)
        while current is not None and not isinstance(
            current, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.Module)
        ):
            if isinstance(current, (ast.Try, ast.TryStar)):
                if current.handlers and child in current.body:
                    return True
            elif isinstance(current, (ast.With, ast.AsyncWith)):
                if child in current.body:
                    return True
            child = current
            current = parents.get(current)
        return False

    def _guarded_import(
        self, site: _Site, key: ast.expr | str | None, *, node: ast.AST | None = None
    ) -> None:
        """Record an import whose failure a handler can swallow (M11)."""
        if not site.executing or not self._guarded(site.node if node is None else node):
            return
        spelled = python_import_key(key)
        self._record(
            spelled if isinstance(spelled, str) else None,
            None,
            "guarded_import",
            site,
            node=node,
            key_bound=spelled if isinstance(spelled, PythonImportKeyBound) else None,
        )


def refine_namespace_cache_writes(
    summary: PythonNamespaceSummary, cache_writes
) -> PythonNamespaceSummary:
    """Project exact typed cache keys onto their captured operation sites.

    Raw syntax remains policy-independent. A missing, uncertain or differently
    captured operation cannot narrow its write; same-line operations never
    share a source-site identity. All alternatives of one operation join.
    """
    keys_by_site = {}
    for site, keys, *_origins in cache_writes:
        if site not in keys_by_site:
            keys_by_site[site] = keys
        else:
            previous = keys_by_site[site]
            keys_by_site[site] = (
                None
                if previous is None or keys is None
                else tuple(sorted(set(previous) | set(keys)))
            )
    rows = []
    for write in summary.foreign_writes:
        keys = keys_by_site.get(write.source_site)
        if write.construct == "import_binding" and keys is not None:
            rows.extend(replace(write, name=key, key_bound=None) for key in keys)
        else:
            rows.append(write)
    return replace(summary, foreign_writes=tuple(sorted(set(rows), key=_source_order)))
