"""Closed-world bindings of module attribute calls (call_anatomy.md 4.5).

Molt compiles every module of a program together, and spec 0216 puts every
writer of a module namespace that the namespace-write authority does not see
outside the contract (``python_namespace_writes``, HF-226). So whether code can
rebind ``M.f`` after ``M`` initializes is a whole-program fact. A call
``m.f(args)`` in module ``U`` becomes a direct call of the intrinsic ``X`` that
``M`` publishes as ``f`` when all of these hold:

F1. ``M.f`` is stable: ``f`` is neither in ``M``'s own deferred writes
    (``PythonNamespaceSummary.own_names``) nor in the names other modules
    write into ``M`` (``PythonNamespacePremise.foreign_writes``); an unknown
    set proves nothing.
F2. ``M``'s body binds ``f`` exactly once, unconditionally, to ``builtin_func
    X`` (the generated publication), and never deletes it.
F3. The receiver is ``M``: it is a read of ``U``'s global ``b``, every store of
    ``b`` in ``U``'s body is the result of ``import M``, ``b`` is stable in
    ``U`` as ``f`` is in ``M``, ``U``'s import of ``M`` is canonical, and ``U``
    is not reachable from ``M`` (no read sees a partly initialized ``M``).
F4. The call is ``call_func [callee, args]`` whose callee is that attribute
    read and has no other use, with exactly ``X``'s positional arity, and ``X``
    is in the generated direct-call table.

The rewrite deletes the attribute read and turns the call into ``call X``;
the receiver's global read stays, so a read before the import still raises
``NameError``. Every other site keeps its generic path, and each qualifying
site that falls back gets one diagnostic naming what blocked it.
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

from molt._public_objects_generated import PUBLIC_DIRECT_CALL_BINDINGS
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.frontend._types import _MOLT_MODULE_CHUNK_PARAM, _intrinsic_signature

# The frontend's guess of the module an attribute read's receiver names
# (``m.f`` with ``m`` an import binding). Consumed here; never on the wire.
MODULE_ATTRIBUTE_FIELD = "module_attribute"

_IMPORT_TRANSACTION = "molt_importlib_import_transaction"
# Graph-wide writes that can change what ``import M`` binds.
_IMPORT_REDIRECTS = (
    PythonMember.SYS_MODULES | PythonMember.IMPORT_HOOKS | PythonMember.BUILTINS_IMPORT
)
_OPENERS = frozenset({"if", "loop_start", "try_start"})
_CLOSERS = frozenset({"end_if", "loop_end", "try_end"})
# Unstructured control flow: no op after one is known to run unconditionally.
_UNSTRUCTURED = frozenset({"label", "jump", "br_if", "state_switch", "state_label"})
_GLOBAL_STORES = frozenset({"module_set_attr", "dict_set"})
_GLOBAL_DELETES = frozenset({"module_del_global", "module_del_global_if_present"})
_ATTRIBUTE_WRITES = (
    "set_attr",
    "store_attr",
    "del_attr",
)


@dataclass(frozen=True)
class ModuleNamespaceFacts:
    """One module's namespace-write facts from the HF-226 authority."""

    own_names: frozenset[str] | None
    premise: PythonNamespacePremise | None
    # The first line whose code reaches the module's own namespace mapping by
    # identity; 0 for none, None when unknown (static module cells).
    own_namespace_line: int | None = None

    def stable_reason(self, name: str) -> str | None:
        """Why code may rebind ``name`` after initialization, or None."""
        if self.own_names is None:
            return "its module reflects its own namespace dynamically"
        if name in self.own_names:
            return "deferred code in its module writes it (global declaration)"
        if self.premise is None or self.premise.foreign_writes is None:
            return (
                "the program writes some module namespace by a computed name "
                "or mapping, so no module's foreign writers are known"
            )
        if name in self.premise.foreign_writes:
            return "another module writes it by name"
        return None


@dataclass(frozen=True)
class _BodyStore:
    unconditional: bool
    value: str | None
    function: str
    line: int | None


def _module_init_symbol(module: str) -> str:
    from molt.frontend.lowering.symbol_naming import SymbolNamingMixin

    return SymbolNamingMixin.module_init_symbol(module)


def _module_body_functions(
    module: str, funcs: Sequence[Mapping[str, Any]]
) -> list[Mapping[str, Any]]:
    init = _module_init_symbol(module)
    return [
        func
        for func in funcs
        if func.get("name") == init
        or _MOLT_MODULE_CHUNK_PARAM in (func.get("params") or ())
    ]


def _body_analysis(
    module: str, funcs: Sequence[Mapping[str, Any]]
) -> tuple[dict[str, list[_BodyStore]], set[str], bool, dict[tuple[str, str], str]]:
    """Every own-namespace store and delete of ``module``'s body, whether
    the body star-imports, and each ``builtin_func`` result's intrinsic."""
    stores: dict[str, list[_BodyStore]] = {}
    deletes: set[str] = set()
    star = False
    builtins: dict[tuple[str, str], str] = {}
    body = _module_body_functions(module, funcs)
    # Every call site of each body function: whether it runs unconditionally.
    chunk_calls: dict[str, list[bool]] = {}
    init = _module_init_symbol(module)
    for func in body:
        constants: dict[str, str] = {}
        depth = 0
        unstructured = False
        own_modules = {
            param
            for param in func.get("params") or ()
            if param == _MOLT_MODULE_CHUNK_PARAM
        }
        globals_dicts: set[str] = set()
        for op in func.get("ops", ()):
            kind = op.get("kind")
            out = op.get("out")
            if kind in _UNSTRUCTURED:
                unstructured = True
            if kind in _CLOSERS:
                depth -= 1
            unconditional = depth == 0 and not unstructured
            if kind in _OPENERS:
                depth += 1
            if kind == "const_str" and isinstance(out, str):
                constants[out] = op.get("s_value")
            elif kind == "module_new" and isinstance(out, str):
                own_modules.add(out)
            elif kind == "call" and op.get("s_value") == "molt_globals_builtin":
                if isinstance(out, str):
                    globals_dicts.add(out)
            elif kind == "builtin_func" and isinstance(out, str):
                builtins[(func.get("name", ""), out)] = op.get("s_value")
            elif kind == "call_internal":
                callee = op.get("s_value")
                if isinstance(callee, str):
                    chunk_calls.setdefault(callee, []).append(unconditional)
            elif kind is not None and "import_star" in kind:
                star = True
            args = op.get("args") or ()
            if len(args) < 2 or args[1] not in constants:
                continue
            name = constants[args[1]]
            own = args[0] in own_modules or (
                kind == "dict_set" and args[0] in globals_dicts
            )
            if kind in _GLOBAL_STORES and own and len(args) == 3:
                stores.setdefault(name, []).append(
                    _BodyStore(
                        unconditional,
                        args[2],
                        func.get("name", ""),
                        op.get("source_line"),
                    )
                )
            elif kind in _GLOBAL_DELETES:
                deletes.add(name)
    # A chunk's store runs once and unconditionally only if the chunk has
    # exactly one call and that call is unconditional. A chunk whose call the
    # scan did not see, or one called twice, proves nothing.
    callers: dict[str, list[str]] = {}
    for func in body:
        for op in func.get("ops", ()):
            if op.get("kind") == "call_internal" and isinstance(op.get("s_value"), str):
                callers.setdefault(op["s_value"], []).append(func.get("name", ""))

    def runs_once(function: str, seen: frozenset[str] = frozenset()) -> bool:
        if function == init:
            return True
        calls = chunk_calls.get(function, [])
        sites = callers.get(function, [])
        return (
            len(calls) == 1
            and calls[0]
            and len(sites) == 1
            and sites[0] not in seen
            and runs_once(sites[0], seen | {function})
        )

    for name, rows in stores.items():
        stores[name] = [
            _BodyStore(
                row.unconditional and runs_once(row.function),
                row.value,
                row.function,
                row.line,
            )
            for row in rows
        ]
    return stores, deletes, star, builtins


@dataclass
class _ModuleBody:
    stores: dict[str, list[_BodyStore]]
    deletes: set[str]
    star: bool
    # Keyed by (function, value): value names are scoped to one function.
    builtins: dict[tuple[str, str], str]
    import_results: dict[tuple[str, str], str]


class ClosedWorldBinder:
    def __init__(
        self,
        module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
        facts: Mapping[str, ModuleNamespaceFacts],
        dep_closures: Mapping[str, Collection[str]],
    ) -> None:
        self.module_functions = module_functions
        self.facts = facts
        self.dep_closures = dep_closures
        self._bodies: dict[str, _ModuleBody] = {}
        self.diagnostics: list[str] = []

    def _body(self, module: str) -> _ModuleBody:
        body = self._bodies.get(module)
        if body is None:
            funcs = self.module_functions.get(module, ())
            stores, deletes, star, builtins = _body_analysis(module, funcs)
            imports: dict[tuple[str, str], str] = {}
            for func in _module_body_functions(module, funcs):
                constants = {
                    op.get("out"): op.get("s_value")
                    for op in func.get("ops", ())
                    if op.get("kind") == "const_str"
                }
                ints = {
                    op.get("out"): op.get("value")
                    for op in func.get("ops", ())
                    if op.get("kind") == "const"
                }
                for op in func.get("ops", ()):
                    args = op.get("args") or ()
                    if (
                        op.get("kind") == "call"
                        and op.get("s_value") == _IMPORT_TRANSACTION
                        and len(args) == 5
                        and isinstance(constants.get(args[0]), str)
                        and ints.get(args[4]) == 0
                        and isinstance(op.get("out"), str)
                    ):
                        imports[(func.get("name", ""), op["out"])] = constants[args[0]]
            body = _ModuleBody(stores, deletes, star, builtins, imports)
            self._bodies[module] = body
        return body

    def _attribute_writers(self, name: str) -> list[str]:
        """Where the program stores or deletes an attribute ``name``, as
        ``module:line``: the location the diagnostic names (the decision
        is the namespace premise's)."""
        found: list[str] = []
        for module, funcs in sorted(self.module_functions.items()):
            for func in funcs:
                for op in func.get("ops", ()):
                    kind = op.get("kind") or ""
                    if kind.startswith(_ATTRIBUTE_WRITES) and op.get("s_value") == name:
                        found.append(f"{module}:{op.get('source_line')}")
                        if len(found) == 3:
                            return found
        return found

    def _facts(self, module: str) -> ModuleNamespaceFacts:
        return self.facts.get(module, ModuleNamespaceFacts(None, None))

    def _publication(self, module: str, name: str, intrinsic: str) -> str | None:
        """F1 and F2: why ``module.name`` is not ``intrinsic``'s published
        object after initialization, or None."""
        reason = self._facts(module).stable_reason(name)
        if reason is not None:
            writers = self._attribute_writers(name)
            located = f" (attribute writes: {', '.join(writers)})" if writers else ""
            return f"{module}.{name} may be rebound: {reason}{located}"
        body = self._body(module)
        if body.star:
            return f"{module}'s body star-imports, which may rebind {name}"
        if name in body.deletes:
            return f"{module}'s body deletes {name}"
        rows = body.stores.get(name, [])
        if len(rows) != 1:
            return f"{module}'s body binds {name} {len(rows)} times, not once"
        (row,) = rows
        if not row.unconditional:
            return f"{module}'s binding of {name} (line {row.line}) is conditional"
        if body.builtins.get((row.function, row.value or "")) != intrinsic:
            return f"{module}'s binding of {name} is not the publication of {intrinsic}"
        return None

    def _receiver(self, user: str, binding: str, module: str) -> str | None:
        """F3: why the global ``binding`` of ``user`` may not be ``module``."""
        reason = self._facts(user).stable_reason(binding)
        if reason is not None:
            return f"{user}.{binding} may be rebound: {reason}"
        if "." in module:
            return f"{module} is a dotted import"
        body = self._body(user)
        if binding in body.deletes:
            return f"{user} deletes {binding}"
        rows = body.stores.get(binding, [])
        if not rows or any(
            body.import_results.get((row.function, row.value or "")) != module
            for row in rows
        ):
            return f"{user}.{binding} is bound by something other than import {module}"
        premise = self._facts(user).premise
        if premise is None:
            return f"{user} has no namespace premise"
        if premise.code_rebinding:
            return (
                f"some code reads a code object or loads a native extension, so "
                f"FunctionType may run {user}'s functions over another globals "
                f"mapping where {binding} is not {module}"
            )
        if module in premise.noncanonical_imports:
            return f"import {module} does not resolve to its canonical provider"
        if premise.member_writes & _IMPORT_REDIRECTS:
            return "the program writes sys.modules, import hooks or __import__"
        if user in self.dep_closures.get(module, (user,)):
            return (
                f"{module} imports {user}: a read may see {module} partly initialized"
            )
        return None

    def bind(self, user: str, func: dict[str, Any]) -> dict[str, Any] | None:
        """The function with its proven sites rewritten, or None if unchanged.
        Every op loses the frontend's module guess."""
        ops = func.get("ops", ())
        if not any(MODULE_ATTRIBUTE_FIELD in op for op in ops):
            return None
        uses = Counter(arg for op in ops for arg in op.get("args") or ())
        defs = {op.get("out"): op for op in ops if isinstance(op.get("out"), str)}
        own_modules = {
            param
            for param in func.get("params") or ()
            if param == _MOLT_MODULE_CHUNK_PARAM
        } | {
            op["out"]
            for op in ops
            if op.get("kind") == "module_new" and isinstance(op.get("out"), str)
        }
        callers = {
            op["args"][0]: op
            for op in ops
            if op.get("kind") == "call_func" and op.get("args")
        }
        replaced: dict[int, dict[str, Any] | None] = {}
        for op in ops:
            module = op.get(MODULE_ATTRIBUTE_FIELD)
            if module is None:
                continue
            stripped = {k: v for k, v in op.items() if k != MODULE_ATTRIBUTE_FIELD}
            replaced[id(op)] = stripped
            name = op.get("s_value")
            intrinsic = PUBLIC_DIRECT_CALL_BINDINGS.get(f"{module}.{name}")
            call = callers.get(op.get("out"))
            if intrinsic is None or call is None:
                continue
            reason = self._site_reason(
                user, op, call, module, name, intrinsic, uses, defs, own_modules
            )
            line = call.get("source_line", op.get("source_line"))
            if reason is not None:
                self.diagnostics.append(
                    f"{user}:{line} {module}.{name}: generic call: {reason}"
                )
                continue
            replaced[id(op)] = None
            direct = {
                key: value
                for key, value in call.items()
                if key not in {"kind", "args", "argument_custody"}
            }
            direct.update(
                {"kind": "call", "s_value": intrinsic, "args": list(call["args"][1:])}
            )
            replaced[id(call)] = direct
        rewritten = [
            replaced.get(id(op), op) if id(op) in replaced else op for op in ops
        ]
        return {**func, "ops": [op for op in rewritten if op is not None]}

    def _site_reason(
        self,
        user: str,
        attribute: Mapping[str, Any],
        call: Mapping[str, Any],
        module: str,
        name: str,
        intrinsic: str,
        uses: Counter[str],
        defs: Mapping[str, Mapping[str, Any]],
        own_modules: set[str],
    ) -> str | None:
        signature = _intrinsic_signature(intrinsic)
        if signature is None or signature[1] != len(call["args"]) - 1:
            return f"the call does not pass {intrinsic}'s exact positional arity"
        out = attribute.get("out")
        if not isinstance(out, str) or uses[out] != 1:
            return "the loaded attribute has another use"
        receiver = defs.get((attribute.get("args") or [None])[0])
        binding = None
        if receiver is not None:
            args = receiver.get("args") or ()
            names = receiver_name = None
            if receiver.get("kind") == "module_get_global" or (
                receiver.get("kind") == "module_get_attr"
                and args
                and args[0] in own_modules
            ):
                names = defs.get(args[1]) if len(args) > 1 else None
            if names is not None and names.get("kind") == "const_str":
                receiver_name = names.get("s_value")
            binding = receiver_name
        reason = self._publication(module, name, intrinsic)
        if reason is not None:
            return reason
        if not isinstance(binding, str):
            return "the receiver is not a read of a module global"
        return self._receiver(user, binding, module)


def bind_closed_world_calls(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
    facts: Mapping[str, ModuleNamespaceFacts],
    dep_closures: Mapping[str, Collection[str]],
) -> list[str]:
    """Rewrite every proven site in ``functions`` (copy-on-write, as the
    global slot stamps are) and drop every module guess; return the
    diagnostics of qualifying sites that keep the generic call."""
    binder = ClosedWorldBinder(module_functions, facts, dep_closures)
    position = {id(function): index for index, function in enumerate(functions)}
    rebound: dict[str, dict[int, dict[str, Any]]] = {}
    for module, funcs in module_functions.items():
        for index, func in enumerate(funcs):
            bound = binder.bind(module, func)
            if bound is not None and id(func) in position:
                functions[position[id(func)]] = bound
                rebound.setdefault(module, {})[index] = bound
    # Later integration steps (the global slot stamps) find each module's
    # bodies by identity in `functions`: they see the rewritten ones.
    for module, rows in rebound.items():
        funcs = module_functions[module]
        for index, bound in rows.items():
            funcs[index] = bound
    return binder.diagnostics
