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
from molt import compiled_symbols

from collections import Counter
from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass
from functools import cached_property
from typing import Any

from molt._intrinsic_symbols import INTRINSIC_SIGNATURES
from molt._public_objects_generated import PUBLIC_DIRECT_CALL_BINDINGS
from molt._wasm_abi_generated import WASM_NON_RUNTIME_CALLABLE_INTRINSICS
from molt.compiler_analysis.python_import_redirects import ImportRedirects
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    python_namespace_stability_reason,
)
from molt.frontend._types import _MOLT_MODULE_CHUNK_PARAM, _intrinsic_signature
from molt.frontend.lowering.op_kinds_generated import FrontendOpKind

# The frontend's guess of the module an attribute read's receiver names
# (``m.f`` with ``m`` an import binding). Consumed here; never on the wire.
MODULE_ATTRIBUTE_FIELD = "module_attribute"
# A read ``m.f`` whose receiver is proven to be module ``M`` (F3) and whose
# name ``M`` never rebinds after initialization (F1): the module it reads.
# ``stamp_global_slots`` turns it into a read of ``M``'s static cell when
# ``M`` keeps one for ``f`` (HF-845), and consumes it; never on the wire.
FOREIGN_GLOBAL_FIELD = "foreign_global"

_INTRINSIC_RESOLVERS = frozenset(
    compiled_symbols.module_function_symbol(module, "require_intrinsic")
    for module in ("_intrinsics", "molt.stdlib._intrinsics")
)


def imported_module_of(op: Mapping[str, Any]) -> str | None:
    """The module an ``import_name`` op's result is, when the op names it
    statically (bedrock §13 M3): the bound root of an absolute ``import
    a.b.c``, the named module of a from-import, ``import_module`` or a
    compiler root. None for any other op and for a relative import, whose
    target depends on the executing frame."""
    if op.get("kind") != "import_name":
        return None
    name = op.get("s_value")
    if not isinstance(name, str) or not name:
        return None
    if op.get("import_form") != "dunder":
        return name
    if op.get("value") != 0:
        return None
    return name if op.get("fromlist") else name.split(".", 1)[0]


_GLOBAL_STORES = frozenset({"module_set_attr", "dict_set"})
_GLOBAL_DELETES = frozenset({"module_del_global", "module_del_global_if_present"})


@dataclass(frozen=True)
class ModuleNamespaceFacts:
    """One module's namespace-write facts from the HF-226 authority."""

    own_names: frozenset[str] | None
    premise: PythonNamespacePremise | None
    # The first line whose code reaches the module's own namespace mapping by
    # identity; 0 for none, None when unknown (static module cells).
    own_namespace_line: int | None = None
    initializer_bindings: frozenset[str] = frozenset()
    isolate_spawn_line: int | None = None

    def stable_reason(self, name: str) -> str | None:
        return python_namespace_stability_reason(self.own_names, self.premise, name)


@dataclass(frozen=True, slots=True)
class _ClassStamp:
    definition: str
    maker: int
    stamp: int


@dataclass(frozen=True, slots=True)
class _BodyStore:
    value: str | None
    function: str
    line: int | None
    index: int
    class_stamps: tuple[_ClassStamp, ...] = ()


def _module_body_functions(
    module: str, funcs: Sequence[Mapping[str, Any]]
) -> list[Mapping[str, Any]]:
    init = compiled_symbols.module_init_symbol(module)
    return [
        func
        for func in funcs
        if func.get("name") == init
        or _MOLT_MODULE_CHUNK_PARAM in (func.get("params") or ())
    ]


# The control kinds the init chain's CFG models, spelled as the frontend CFG
# reads them; every other kind is straight-line code to it. A body function
# with an unmodeled control kind (``br_if``, generator states) orders nothing.
_CHAIN_CONTROL: Mapping[str, FrontendOpKind] = {
    "if": "IF",
    "else": "ELSE",
    "end_if": "END_IF",
    "loop_start": "LOOP_START",
    "loop_end": "LOOP_END",
    "loop_break": "LOOP_BREAK",
    "loop_break_if_true": "LOOP_BREAK_IF_TRUE",
    "loop_break_if_false": "LOOP_BREAK_IF_FALSE",
    "loop_continue": "LOOP_CONTINUE",
    "try_start": "TRY_START",
    "try_end": "TRY_END",
    "jump": "JUMP",
    "label": "LABEL",
    "check_exception": "CHECK_EXCEPTION",
    "ret": "ret",
    "ret_void": "ret_void",
    "raise": "RAISE",
}
_UNMODELED_CONTROL = frozenset(
    {"br_if", "state_switch", "state_label", "state_yield", "state_transition"}
)
_LABELLED = frozenset({"label", "jump", "check_exception", "try_start"})


class _ChainOp:
    """A SimpleIR op as the frontend CFG reads it: its control kind, and the
    label a control transfer names."""

    __slots__ = ("kind", "args")

    def __init__(self, op: Mapping[str, Any]) -> None:
        kind = str(op.get("kind"))
        self.kind: FrontendOpKind = _CHAIN_CONTROL.get(kind, "NOP")
        value = op.get("value")
        self.args: list[Any] = (
            [value] if kind in _LABELLED and value is not None else []
        )


@dataclass(frozen=True)
class _ChainFunction:
    ops: Sequence[Mapping[str, Any]]
    cfg: Any
    # Blocks on a cycle: an op there may run more than once.
    cyclic: frozenset[int]
    # Blocks that end a normal return: an exit block (a ``ret``, or the end
    # of the ops) that no exception landing (``label`` with
    # ``exception_source: unwind``) dominates.
    normal_returns: tuple[int, ...]


def _cyclic_blocks(successors: Mapping[int, Sequence[int]]) -> frozenset[int]:
    """The blocks of every strongly connected component with a cycle."""
    index_of: dict[int, int] = {}
    low: dict[int, int] = {}
    stack: list[int] = []
    on_stack: set[int] = set()
    cyclic: set[int] = set()
    counter = 0
    for root in successors:
        if root in index_of:
            continue
        work: list[tuple[int, int]] = [(root, 0)]
        while work:
            node, child = work.pop()
            if child == 0:
                index_of[node] = low[node] = counter
                counter += 1
                stack.append(node)
                on_stack.add(node)
            targets = successors.get(node, ())
            if child < len(targets):
                work.append((node, child + 1))
                target = targets[child]
                if target not in index_of:
                    work.append((target, 0))
                elif target in on_stack:
                    low[node] = min(low[node], index_of[target])
                continue
            for target in targets:
                if target in on_stack:
                    low[node] = min(low[node], low[target])
            if low[node] == index_of[node]:
                component: list[int] = []
                while True:
                    member = stack.pop()
                    on_stack.discard(member)
                    component.append(member)
                    if member == node:
                        break
                if len(component) > 1 or node in targets:
                    cyclic.update(component)
    return frozenset(cyclic)


class InitChain:
    """The run-once chain of a module's init: the init function and each body
    chunk it calls from exactly one call site that runs at most once, as one
    control-flow graph per function (the frontend CFG, ``cfg_analysis``).

    A point is a (function, op index) pair. ``dominates(a, b)`` holds when
    every execution of ``b`` follows an execution of ``a``: in one function,
    ``a``'s block dominates ``b``'s; across a chunk call, the caller's part of
    the path is checked at the call site, and a point inside a chunk counts
    as done after the call only when it dominates every normal return of the
    chunk. An exception that leaves a chunk takes its caller's exception edge,
    so code after the call's normal continuation never runs on that path."""

    def __init__(self, module: str, initializers: Sequence[Mapping[str, Any]]) -> None:
        from molt.frontend.cfg_analysis import build_cfg

        body = {func.get("name", ""): func for func in initializers}
        self.init = compiled_symbols.module_init_symbol(module)
        sites: dict[str, list[tuple[str, int]]] = {}
        for name, func in body.items():
            for index, op in enumerate(func.get("ops", ())):
                callee = op.get("s_value")
                if op.get("kind") == "call_internal" and callee in body:
                    sites.setdefault(callee, []).append((name, index))
        self.functions: dict[str, _ChainFunction] = {}
        self.parent: dict[str, tuple[str, int]] = {}
        pending = [self.init] if self.init in body else []
        while pending:
            name = pending.pop()
            ops = body[name].get("ops", ())
            if any(op.get("kind") in _UNMODELED_CONTROL for op in ops):
                continue
            cfg = build_cfg([_ChainOp(op) for op in ops])
            landings = [
                block.id
                for block in cfg.blocks
                if block.end > block.start
                and ops[block.start].get("kind") == "label"
                and ops[block.start].get("exception_source") == "unwind"
            ]
            normal = [
                block.id
                for block in cfg.blocks
                if block.end > block.start
                and block.id in cfg.reachable
                and not cfg.successors.get(block.id)
                and not any(
                    cfg.dominance.dominates(landing, block.id) for landing in landings
                )
            ]
            chain = _ChainFunction(
                ops, cfg, _cyclic_blocks(cfg.successors), tuple(normal)
            )
            self.functions[name] = chain
            for callee, callee_sites in sites.items():
                if len(callee_sites) != 1 or callee in self.functions:
                    continue
                caller, index = callee_sites[0]
                block = cfg.block_for_op(index)
                if (
                    caller == name
                    and block is not None
                    and block in cfg.reachable
                    and block not in chain.cyclic
                ):
                    self.parent[callee] = (caller, index)
                    pending.append(callee)

    def runs_once(self, point: tuple[str, int]) -> bool:
        """Whether ``point``'s op runs at most once per process."""
        chain = self.functions.get(point[0])
        if chain is None:
            return False
        block = chain.cfg.block_for_op(point[1])
        return (
            block is not None
            and block in chain.cfg.reachable
            and block not in chain.cyclic
        )

    def _local(self, function: str, a: int, b: int) -> bool:
        """Whether op ``a`` runs before every execution of op ``b`` (or is
        ``b``) in one function."""
        cfg = self.functions[function].cfg
        block_a, block_b = cfg.block_for_op(a), cfg.block_for_op(b)
        if block_a is None or block_b is None:
            return False
        if block_a == block_b:
            return a <= b and block_a in cfg.reachable
        return cfg.dominance.dominates(block_a, block_b)

    def _done_after_return(self, point: tuple[str, int]) -> bool:
        """Whether ``point`` runs on every normal return of its function."""
        function, index = point
        chain = self.functions[function]
        if index >= len(chain.ops):
            # The continuation of a call that ends the function's ops.
            return True
        last = {
            block: chain.cfg.blocks[block].end - 1 for block in chain.normal_returns
        }
        return all(self._local(function, index, last_op) for last_op in last.values())

    def _continuation(self, function: str) -> tuple[str, int]:
        """The first point of ``function``'s caller after its call returns
        normally: past the call's ``check_exception``, if it has one."""
        caller, index = self.parent[function]
        ops = self.functions[caller].ops
        after = index + 1
        if after < len(ops) and ops[after].get("kind") == "check_exception":
            after += 1
        return caller, after

    def _ancestors(self, function: str) -> list[str]:
        found = [function]
        while found[-1] in self.parent:
            found.append(self.parent[found[-1]][0])
        return found

    def dominates(self, a: tuple[str, int], b: tuple[str, int]) -> bool:
        """Whether every execution of ``b`` follows an execution of ``a``, or
        ``b`` is ``a`` (reflexive)."""
        if a[0] not in self.functions or b[0] not in self.functions:
            return False
        a_line = self._ancestors(a[0])
        b_line = self._ancestors(b[0])
        common = next((name for name in b_line if name in a_line), None)
        if common is None:
            return False
        # Lift ``a`` to the common function: it must be done on every normal
        # return of each chunk it leaves.
        while a[0] != common:
            if not self._done_after_return(a):
                return False
            a = self._continuation(a[0])
        # Lift ``b`` to its call site in the common function.
        while b[0] != common:
            b = self.parent[b[0]]
        return self._local(common, a[1], b[1])

    def unconditional(self, point: tuple[str, int]) -> bool:
        """Whether ``point`` runs once, on every normal completion of the
        init: every op after the init returns observes its effect."""
        if not self.runs_once(point):
            return False
        while point[0] != self.init:
            if point[0] not in self.parent or not self._done_after_return(point):
                return False
            point = self._continuation(point[0])
        return self._done_after_return(point)


# The ops that create a function's object. A closure's object holds its
# environment, which a proven call reads from it (design 78, 5.10).
FUNCTION_MAKERS = frozenset({"func_new", "func_new_closure"})


@dataclass(frozen=True)
class CallablePublication:
    point: tuple[str, int]
    value: str


@dataclass(frozen=True)
class ModuleInitializerFacts:
    stores: dict[str, list[_BodyStore]]
    deletes: set[str]
    star: bool
    # Value identities are scoped to their initializer function.
    builtins: dict[tuple[str, str], str]
    import_results: dict[tuple[str, str], str]
    producers: dict[tuple[str, str], Mapping[str, Any]]
    callable_publications: dict[str, list[CallablePublication]]
    module: str
    body: tuple[Mapping[str, object], ...]

    @cached_property
    def chain(self) -> InitChain:
        return InitChain(self.module, self.body)

    def unconditional(self, store: _BodyStore) -> bool:
        return self.chain.unconditional((store.function, store.index))

    def class_definition(self, store: _BodyStore) -> str | None:
        # A later stamp supersedes an earlier one only if its maker and
        # the stamp itself are proven to precede the store.
        for candidate in reversed(store.class_stamps):
            maker = (store.function, candidate.maker)
            stamp = (store.function, candidate.stamp)
            if self.chain.runs_once(maker) and self.chain.dominates(maker, stamp):
                if self.chain.dominates(stamp, (store.function, store.index)):
                    return candidate.definition
                break
        return None


_EMPTY_INITIALIZER = ModuleInitializerFacts({}, set(), False, {}, {}, {}, {}, "", ())


def _intrinsic_result_candidates(
    initializer: ModuleInitializerFacts,
) -> dict[str, tuple[str, int]]:
    """Identify useful guesses from the initializer's stored values.

    A resolver call and its literal suggest a runtime target. The resolver
    still runs normally and can return a replacement. A candidate does not
    prove initialization, stability, or even the type of the value read.
    """
    candidates: dict[str, tuple[str, int]] = {}
    for name, stores in initializer.stores.items():
        if len(stores) != 1:
            continue
        (store,) = stores
        call = initializer.producers.get((store.function, store.value or ""), {})
        args = call.get("args") or ()
        if (
            call.get("kind") != "call_guarded"
            or call.get("s_value") not in _INTRINSIC_RESOLVERS
            or len(args) != 2
        ):
            continue
        literal = initializer.producers.get((store.function, args[1]), {})
        if literal.get("kind") != "const_str":
            continue
        value = literal.get("s_value")
        signature = INTRINSIC_SIGNATURES.get(value) if isinstance(value, str) else None
        if (
            signature is not None
            and signature[0] not in WASM_NON_RUNTIME_CALLABLE_INTRINSICS
        ):
            candidates[name] = signature
    return candidates


def summarize_module_initializer(
    module: str, funcs: Sequence[Mapping[str, Any]]
) -> ModuleInitializerFacts:
    """Analyze the always-materialized initializer and its run-once chunks."""
    stores: dict[str, list[_BodyStore]] = {}
    deletes: set[str] = set()
    star = False
    builtins: dict[tuple[str, str], str] = {}
    imports: dict[tuple[str, str], str] = {}
    producers: dict[tuple[str, str], Mapping[str, Any]] = {}
    callable_publications: dict[str, list[CallablePublication]] = {}
    body = tuple(_module_body_functions(module, funcs))
    for func in body:
        constants: dict[str, str] = {}
        class_makers: dict[str, int] = {}
        class_stamps: dict[str, list[_ClassStamp]] = {}
        own_modules = {
            param
            for param in func.get("params") or ()
            if param == _MOLT_MODULE_CHUNK_PARAM
        }
        globals_dicts: set[str] = set()
        name_of_function = func.get("name", "")
        for index, op in enumerate(func.get("ops", ())):
            kind = op.get("kind")
            out = op.get("out")
            args = op.get("args") or ()
            if isinstance(out, str):
                producers[(name_of_function, out)] = op
                if kind == "class_def":
                    class_makers[out] = index
                if imported := imported_module_of(op):
                    imports[(name_of_function, out)] = imported
                symbol = op.get("s_value")
                if kind in FUNCTION_MAKERS and isinstance(symbol, str):
                    point = (name_of_function, index)
                    callable_publications.setdefault(symbol, []).append(
                        CallablePublication(point, out)
                    )
            if kind == "const_str" and isinstance(out, str):
                constants[out] = op.get("s_value")
            elif kind == "module_new" and isinstance(out, str):
                own_modules.add(out)
            elif kind == "call" and op.get("s_value") == "molt_globals_builtin":
                if isinstance(out, str):
                    globals_dicts.add(out)
            elif kind == "builtin_func" and isinstance(out, str):
                builtins[(name_of_function, out)] = op.get("s_value")
            elif kind == "class_set_definition" and len(args) == 1:
                definition = op.get("class_definition")
                maker = class_makers.get(args[0])
                if isinstance(definition, str) and definition and maker is not None:
                    class_stamps.setdefault(args[0], []).append(
                        _ClassStamp(definition, maker, index)
                    )
            elif kind is not None and "import_star" in kind:
                star = True
            if len(args) < 2 or args[1] not in constants:
                continue
            name = constants[args[1]]
            own = args[0] in own_modules or (
                kind == "dict_set" and args[0] in globals_dicts
            )
            if kind in _GLOBAL_STORES and own and len(args) == 3:
                stores.setdefault(name, []).append(
                    _BodyStore(
                        args[2],
                        name_of_function,
                        op.get("source_line"),
                        index,
                        tuple(class_stamps.get(args[2], ())),
                    )
                )
            elif kind in _GLOBAL_DELETES:
                deletes.add(name)
    return ModuleInitializerFacts(
        stores,
        deletes,
        star,
        builtins,
        imports,
        producers,
        callable_publications,
        module,
        body,
    )


class ClosedWorldBinder:
    def __init__(
        self,
        initializers: Mapping[str, ModuleInitializerFacts],
        facts: Mapping[str, ModuleNamespaceFacts],
        dep_closures: Mapping[str, Collection[str]],
        import_redirects: ImportRedirects,
    ) -> None:
        self.initializers = initializers
        self.facts = facts
        self.dep_closures = dep_closures
        self.import_redirects = import_redirects
        self.diagnostics: list[str] = []
        self._intrinsic_candidates: dict[str, dict[str, tuple[str, int]]] = {}

    def _body(self, module: str) -> ModuleInitializerFacts:
        return self.initializers.get(module, _EMPTY_INITIALIZER)

    def _facts(self, module: str) -> ModuleNamespaceFacts:
        return self.facts.get(module, ModuleNamespaceFacts(None, None))

    def _publication(self, module: str, name: str, intrinsic: str) -> str | None:
        """F1 and F2: why ``module.name`` is not ``intrinsic``'s published
        object after initialization, or None."""
        reason = self._facts(module).stable_reason(name)
        if reason is not None:
            return f"{module}.{name} may be rebound: {reason}"
        body = self._body(module)
        if body.star:
            return f"{module}'s body star-imports, which may rebind {name}"
        if name in body.deletes:
            return f"{module}'s body deletes {name}"
        rows = body.stores.get(name, [])
        if len(rows) != 1:
            return f"{module}'s body binds {name} {len(rows)} times, not once"
        (row,) = rows
        if not body.unconditional(row):
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
        if writers := self.import_redirects.redirect(module):
            return (
                f"import {module} may be redirected: {', '.join(map(str, writers[:3]))}"
            )
        if user in self.dep_closures.get(module, (user,)):
            return (
                f"{module} imports {user}: a read may see {module} partly initialized"
            )
        return None

    def bind(self, user: str, func: dict[str, Any]) -> dict[str, Any] | None:
        """The function with its proven sites rewritten, or None if unchanged.
        Every op loses the frontend's module guess."""
        ops = func.get("ops", ())
        replaced = self._runtime_call_candidates(user, func)
        if not any(MODULE_ATTRIBUTE_FIELD in op for op in ops):
            return (
                {**func, "ops": [replaced.get(id(op), op) for op in ops]}
                if replaced
                else None
            )
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
        for op in ops:
            module = op.get(MODULE_ATTRIBUTE_FIELD)
            if module is None:
                continue
            stripped = {k: v for k, v in op.items() if k != MODULE_ATTRIBUTE_FIELD}
            name = op.get("s_value")
            foreign = (
                self._foreign_reason(user, op, module, name, defs, own_modules)
                if isinstance(name, str)
                else "the attribute name is not a constant"
            )
            if foreign is None:
                stripped[FOREIGN_GLOBAL_FIELD] = module
            replaced[id(op)] = stripped
            intrinsic = PUBLIC_DIRECT_CALL_BINDINGS.get(f"{module}.{name}")
            call = callers.get(op.get("out"))
            if intrinsic is None or call is None:
                # A refused read stays a by-name runtime lookup; say why. An
                # intrinsic call site names its refusal below instead.
                if foreign is not None:
                    self.diagnostics.append(
                        f"{user}:{op.get('source_line')} {module}.{name}: "
                        f"by-name read: {foreign}"
                    )
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

    def _runtime_call_candidates(
        self, user: str, func: Mapping[str, Any]
    ) -> dict[int, dict[str, Any] | None]:
        """Attach a runtime candidate to an ordinary call of its live load."""
        if user not in self._intrinsic_candidates:
            self._intrinsic_candidates[user] = _intrinsic_result_candidates(
                self._body(user)
            )
        candidates = self._intrinsic_candidates[user]
        if not candidates:
            return {}
        from molt.cli.frontend_integration import _global_name_sites

        reads = {
            op["out"]: candidates[name]
            for _, op, name in _global_name_sites(func)
            if op.get("kind") == "module_get_global"
            and isinstance(op.get("out"), str)
            and name in candidates
        }
        rewritten: dict[int, dict[str, Any] | None] = {}
        for op in func.get("ops", ()):
            args = op.get("args") or ()
            if op.get("kind") != "call_func" or not args or args[0] not in reads:
                continue
            symbol, arity = reads[args[0]]
            if len(args) == arity + 1 and op.get("argument_custody") == [
                "transferred"
            ] * len(args):
                rewritten[id(op)] = {**op, "runtime_candidate": symbol}
        return rewritten

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
        reason = self._publication(module, name, intrinsic)
        if reason is not None:
            return reason
        binding = _receiver_binding(attribute, defs, own_modules)
        if binding is None:
            return "the receiver is not a read of a module global"
        return self._receiver(user, binding, module)

    def _foreign_reason(
        self,
        user: str,
        attribute: Mapping[str, Any],
        module: str,
        name: str,
        defs: Mapping[str, Mapping[str, Any]],
        own_modules: set[str],
    ) -> str | None:
        """F1 and F3 for any read ``m.name``: why it may not read the binding
        ``module`` keeps for ``name`` after initialization, or None. Whether
        that binding is one static cell is the cell plan's question."""
        reason = self._facts(module).stable_reason(name)
        if reason is not None:
            return f"{module}.{name} may be rebound: {reason}"
        body = self._body(module)
        if body.star:
            return f"{module}'s body star-imports, which may rebind {name}"
        if name in body.deletes:
            return f"{module}'s body deletes {name}"
        binding = _receiver_binding(attribute, defs, own_modules)
        if binding is None:
            return "the receiver is not a read of a module global"
        return self._receiver(user, binding, module)


def _receiver_binding(
    attribute: Mapping[str, Any],
    defs: Mapping[str, Mapping[str, Any]],
    own_modules: set[str],
) -> str | None:
    """The global of the reading module that an attribute read's receiver
    reads, or None when the receiver is not such a read."""
    receiver = defs.get((attribute.get("args") or [None])[0])
    if receiver is None:
        return None
    args = receiver.get("args") or ()
    names = None
    if receiver.get("kind") == "module_get_global" or (
        receiver.get("kind") == "module_get_attr" and args and args[0] in own_modules
    ):
        names = defs.get(args[1]) if len(args) > 1 else None
    if names is not None and names.get("kind") == "const_str":
        value = names.get("s_value")
        return value if isinstance(value, str) else None
    return None


def bind_closed_world_calls(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
    facts: Mapping[str, ModuleNamespaceFacts],
    dep_closures: Mapping[str, Collection[str]],
    import_redirects: ImportRedirects,
) -> tuple[list[str], dict[str, ModuleInitializerFacts]]:
    """Bind calls against the admitted bodies and publish the resulting init facts.

    The binder reads the original publications for the whole program. Later
    passes need coordinates in the rewritten bodies, which may have lost an
    attribute-read op. Reanalyze only modules whose initializer was rewritten.
    """
    initializers = {
        module: summarize_module_initializer(module, funcs)
        for module, funcs in module_functions.items()
    }
    binder = ClosedWorldBinder(initializers, facts, dep_closures, import_redirects)
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
        body_ids = {id(original) for original in initializers[module].body}
        # Same-length binding only annotates call and attribute sites. Its
        # operands, op kinds and initializer coordinates stay the same.
        changed_body = any(
            id(funcs[index]) in body_ids
            and len(bound["ops"]) != len(funcs[index].get("ops", ()))
            for index, bound in rows.items()
        )
        for index, bound in rows.items():
            funcs[index] = bound
        if changed_body:
            initializers[module] = summarize_module_initializer(module, funcs)
    return binder.diagnostics, initializers
