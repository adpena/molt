"""Static module cells: closed-world module globals (call_anatomy.md 4.5a).

Molt compiles the whole program, and spec 0216 excludes ``eval``/``exec`` and
runtime monkeypatching, so whether code can reach a module's namespace mapping
by identity is a whole-program fact. When no code can, the mapping does not
need to hold the module's globals: each global of the module's layout becomes
one fixed-address word in program data (a static cell), read and stored in
place by compiled code, with no runtime call and no dictionary mirror. The
dictionary keeps dunders and every name outside the layout; the runtime's
by-name paths (module attribute access, ``dir``, star imports, pickle's
``find_class``) consult the cells through the registry's sorted layout.

A module ``M`` is cell-eligible when every fact below holds; eligibility is
all-or-nothing per module, and an ineligible module keeps its dictionary and
slot table unchanged (``stamp_global_slots``):

C1. ``M`` has a registry row with a body and a nonempty layout, and its body
    creates its own module object (``module_new``), where the cells bind.
C2. No code reaches ``M``'s namespace mapping by identity: ``M``'s own code
    calls no ``globals()``, module-scope ``vars()``/``locals()`` or
    ``exec``/``eval`` (``PythonNamespaceSummary.own_namespace_line``), and no
    activated code anywhere reaches ``M.__dict__``, ``vars(M)``, a
    ``sys.modules`` entry's ``__dict__``, a ``__globals__`` or an
    ``f_globals`` except by literal dunder keys
    (``PythonNamespacePremise.namespace_reflected``), and no activated code
    reads a code object that ``FunctionType`` could run over another globals
    mapping (``PythonNamespacePremise.code_rebinding``), whose reads a cell
    would not see.
C3. Every writer of ``M``'s namespace is named: ``M``'s own deferred writes and
    the premise's foreign writes are known sets.
C4. ``M``'s body does not star-import (which binds names the layout lacks);
    no writer redirects ``import M`` (``ImportRedirects.redirect``: a
    ``sys.modules`` write of ``M`` or an ancestor, a hook or ``__import__``
    write), which could rebind ``M``'s identity and re-run its body over the
    same cells; and no handler can swallow a failure of ``M``'s body
    (``ImportRedirects.swallowers``: a guarded import of ``M`` or of a
    transitive importer), after which the next import re-runs the body with
    the first run's cell values (M11, HF-877).
C5. Program-wide: no extension module (the C-API can take any module's
    dictionary) and no isolate thread (``molt_thread_spawn``: the cells are
    one per process until stage 1 gives each isolate its own block).
C6. The body runs once per process: the entry module, whose body runs as
    ``__main__``, is not also imported by its own name, which would run the
    same body for a second module object over the same cells.

Each layout name of an eligible module also gets its ``InitOnce`` fact when
the module's body stores it once, unconditionally, at initialization, and no
code writes or deletes it afterward (the F1/F2 facts of
``closed_world_bindings``). A module whose value escapes (a ``sys.modules``
entry, ``importlib.import_module`` result or import name stored, returned or
passed on) may be written by any named attribute store in the program, so each
such name loses the fact; the build diagnostics name the writer site
(``escape_written``). A read of an InitOnce cell in the module's own code
that runs only after that store (``init_dominance``) is one load with no
unbound test and no retain (HF-846); every other read of an InitOnce cell keeps
the checked read, and the build diagnostics name the failed fact.
"""

from __future__ import annotations

from collections.abc import Callable, Collection, Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from functools import cached_property
from typing import Any, Literal

from molt.typed_cache import typed_cache
from molt.compiler_analysis.builtin_binding_guards_generated import (
    BUILTIN_BINDING_GUARD_BITS,
)
from molt.compiler_analysis.python_import_redirects import ImportRedirects
from molt.cli.closed_world_bindings import (
    FUNCTION_MAKERS,
    CallablePublication,
    ModuleInitializerFacts,
    ModuleNamespaceFacts,
    _module_body_functions,
    InitChain,
    imported_module_of,
)


InitOnceKind = Literal["function", "class", "module", "intrinsic", "other"]

_VALUE_KINDS: Mapping[str, InitOnceKind] = {
    "func_new": "function",
    "func_new_closure": "function",
    "class_def": "class",
    "builtin_func": "intrinsic",
}


@dataclass(frozen=True)
class InitOnce:
    """A static cell its module's initialization stores exactly once, which no
    code writes or deletes afterward: once bound, it holds one object of
    ``value_kind`` for the rest of the program. ``function`` and ``index``
    locate the store op; ``line`` is its source line."""

    value_kind: InitOnceKind
    function: str = ""
    index: int = -1
    line: int | None = None
    # The value the store binds: the producer's ``out`` in ``function``. Later
    # integration steps insert ops into ``function`` (the cell bind), so the
    # producer is found by this name, never by an op index.
    value: str | None = None
    producer: Mapping[str, Any] | None = field(default=None, compare=False, repr=False)


@dataclass(frozen=True)
class StaticCellPlan:
    """The program's cell-eligible modules and the facts on their cells."""

    modules: frozenset[str] = frozenset()
    # module -> layout name -> fact, for the names that have it.
    init_once: Mapping[str, Mapping[str, InitOnce]] = field(default_factory=dict)
    # module -> why it keeps its dictionary namespace.
    ineligible: Mapping[str, str] = field(default_factory=dict)
    # module -> builtin binding guard bits (HF-79) of the layout names some
    # code may bind: a cell store never reaches the dictionary's guard state,
    # so the bind records them when the module is created.
    guard_masks: Mapping[str, int] = field(default_factory=dict)
    # module -> (function name, op index) of each read of an InitOnce cell
    # that runs only after the cell's store (HF-846).
    init_dominated: Mapping[str, frozenset[tuple[str, int]]] = field(
        default_factory=dict
    )
    # Why each other read of an InitOnce cell keeps the checked read.
    checked_init_once_reads: tuple[str, ...] = ()
    # Each name stored once at initialization that is not InitOnce because
    # code stores it through an escaped module value, with that writer site.
    escape_written: tuple[str, ...] = ()

    def diagnostics(self) -> dict[str, object]:
        return {
            "eligible": sorted(self.modules),
            "init_once": {
                module: {name: fact.value_kind for name, fact in sorted(rows.items())}
                for module, rows in sorted(self.init_once.items())
            },
            "init_dominated_reads": {
                module: len(reads)
                for module, reads in sorted(self.init_dominated.items())
            },
            "checked_init_once_reads": list(self.checked_init_once_reads),
            "escape_written": list(self.escape_written),
            "ineligible": dict(sorted(self.ineligible.items())),
        }


def _program_blocker(
    facts: Mapping[str, ModuleNamespaceFacts],
    extension_modules: Collection[str],
) -> str | None:
    if extension_modules:
        first = sorted(extension_modules)[0]
        return f"the program loads extension module {first}, whose C-API can take any module dictionary"
    for module, source in sorted(facts.items()):
        if source.isolate_spawn_line is not None:
            return (
                f"{module}:{source.isolate_spawn_line} may spawn an isolate thread, "
                "which would share the process's cells"
            )
    return None


def module_object_site(
    module: str, funcs: Sequence[Mapping[str, Any]]
) -> tuple[str, int] | None:
    """The function and op index of the ``module_new`` that creates
    ``module``'s own module object in its init, where its cells bind."""
    for func in _module_body_functions(module, funcs):
        for index, op in enumerate(func.get("ops", ())):
            if op.get("kind") == "module_new" and isinstance(op.get("out"), str):
                return func.get("name", ""), index
    return None


def _module_reason(
    module: str,
    funcs: Sequence[Mapping[str, Any]],
    facts: ModuleNamespaceFacts | None,
    import_redirects: ImportRedirects,
    importers: Collection[str],
    initializer: ModuleInitializerFacts,
) -> str | None:
    """Why ``module`` keeps its dictionary namespace (C1-C4), or None."""
    if facts is None or facts.own_names is None:
        return "its own code reflects its namespace dynamically"
    if facts.own_namespace_line is None:
        return "its source scan reported no namespace reach"
    if facts.own_namespace_line:
        return (
            f"line {facts.own_namespace_line} reaches its namespace mapping "
            "(globals(), vars(), locals(), exec or eval)"
        )
    premise = facts.premise
    if premise is None:
        return "it has no namespace premise"
    if premise.namespace_reflected:
        return (
            "some code reaches its namespace mapping by identity (__dict__, "
            "vars(), __globals__, f_globals or a sys.modules entry)"
        )
    if premise.code_rebinding:
        return (
            "some code reads a code object (__code__, f_code, gi_code, cr_code "
            "or ag_code) or loads a native extension, so FunctionType may run "
            "this module's functions over another globals mapping"
        )
    if premise.foreign_writes is None:
        return "the program writes some module namespace by a computed name"
    if writers := import_redirects.redirect(module):
        return f"import {module} may be redirected: " + ", ".join(map(str, writers[:3]))
    if writers := import_redirects.swallowers(module, importers):
        return (
            "a handler can swallow a failure of its body, and the next import "
            "re-runs it: " + ", ".join(map(str, writers[:3]))
        )
    if initializer.star:
        return "its body star-imports"
    if module_object_site(module, funcs) is None:
        return "its body does not create its module object"
    return None


def _init_once_facts(
    module: str,
    funcs: Sequence[Mapping[str, Any]],
    facts: ModuleNamespaceFacts,
    layout: Sequence[str],
    initializer: ModuleInitializerFacts,
    escape_dropped: list[str] | None = None,
) -> dict[str, InitOnce]:
    stores, deletes = initializer.stores, initializer.deletes
    indices: dict[int, int] = {}
    for func in _module_body_functions(module, funcs):
        indices.update((id(op), index) for index, op in enumerate(func.get("ops", ())))
    found: dict[str, InitOnce] = {}
    for name in layout:
        if name in deletes:
            continue
        reason = facts.stable_reason(name)
        if reason is not None:
            if escape_dropped is not None and facts.premise is not None:
                writer = facts.premise.escape_writer(name)
                if writer is not None and len(stores.get(name, ())) == 1:
                    escape_dropped.append(
                        f"{module}.{name} not InitOnce: written at {writer} "
                        "through an escaped module value"
                    )
            continue
        rows = stores.get(name, [])
        if len(rows) != 1 or not rows[0].unconditional:
            continue
        store = rows[0]
        producer = initializer.producers.get((store.function, store.value or ""))
        kind = producer.get("kind") if producer is not None else None
        if producer is not None and imported_module_of(producer) is not None:
            value_kind: InitOnceKind = "module"
        else:
            value_kind = _VALUE_KINDS.get(str(kind), "other")
        found[name] = InitOnce(
            value_kind,
            store.function,
            indices[id(store.operation)],
            store.line,
            store.value,
            producer,
        )
    return found


def plan_module_cells(
    module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
    facts: Mapping[str, ModuleNamespaceFacts],
    global_layouts: Mapping[str, tuple[str, ...]],
    registered_modules: Collection[str],
    import_redirects: ImportRedirects,
    extension_modules: Collection[str] = (),
    entry_module: str | None = None,
    dep_closures: Mapping[str, Collection[str]] | None = None,
    *,
    program_functions: Sequence[Mapping[str, Any]],
    initializers: Mapping[str, ModuleInitializerFacts],
) -> tuple[StaticCellPlan, FunctionCells]:
    """Plan global and function cells from the same post-binding IR.

    Decide C1-C6 for each module with a registry row and a layout, then
    reuse one initialization proof for global reads and method loads.
    ``program_functions`` is every function of the program:
    a function runs only through a reference some op makes to its symbol."""
    candidates = sorted(
        module
        for module in registered_modules
        if global_layouts.get(module) and module in module_functions
    )
    blocker = _program_blocker(facts, extension_modules)
    if blocker is not None:
        return StaticCellPlan(
            ineligible=dict.fromkeys(candidates, blocker)
        ), FunctionCells()
    eligible: set[str] = set()
    guard_masks: dict[str, int] = {}
    init_once: dict[str, dict[str, InitOnce]] = {}
    init_dominated: dict[str, frozenset[tuple[str, int]]] = {}
    checked: list[str] = []
    escape_written: list[str] = []
    cell_indices: dict[str, dict[str, int]] = {}
    cell_stores: dict[str, dict[str, tuple[str, str]]] = {}
    cell_reads: dict[str, dict[str, tuple[str, str]]] = {}
    method_diagnostics: list[str] = []

    @typed_cache(maxsize=None)
    def references() -> dict[str, list[_Reference]]:
        return _symbol_references(program_functions)

    ineligible: dict[str, str] = {}
    # module -> the modules whose import runs its body first.
    importers: dict[str, set[str]] = {}
    for importer, closure in (dep_closures or {}).items():
        for dependency in closure:
            if dependency != importer:
                importers.setdefault(dependency, set()).add(importer)
    for module in candidates:
        funcs = module_functions[module]
        module_facts = facts.get(module)
        reason = _module_reason(
            module,
            funcs,
            module_facts,
            import_redirects,
            importers.get(module, ()),
            initializers[module],
        )
        if reason is None and module == entry_module and dep_closures is not None:
            importer = next(
                (
                    other
                    for other, closure in sorted(dep_closures.items())
                    if other != module and module in closure
                ),
                None,
            )
            if importer is not None:
                reason = (
                    f"{importer} imports the entry module by name, which runs "
                    "its body a second time"
                )
        if reason is not None:
            ineligible[module] = reason
            continue
        assert module_facts is not None
        eligible.add(module)
        guard_masks[module] = _written_guard_mask(module_facts, global_layouts[module])
        rows = _init_once_facts(
            module,
            funcs,
            module_facts,
            global_layouts[module],
            initializers[module],
            escape_written,
        )
        execution = _ModuleExecution(module, funcs, references)
        if rows:
            init_once[module] = rows
            dominance = init_dominance(execution, rows)
            if dominance.reads:
                init_dominated[module] = dominance.reads
            checked.extend(dominance.diagnostics)
        for name, callable_var, symbol, maker in _proven_method_loads(
            execution, initializers[module], method_diagnostics
        ):
            cells = cell_indices.setdefault(module, {})
            if symbol not in cells:
                cells[symbol] = len(global_layouts[module]) + len(cells)
                cell_stores.setdefault(maker.point[0], {})[maker.value] = (
                    module,
                    symbol,
                )
            cell_reads.setdefault(name, {})[callable_var] = (module, symbol)
    return StaticCellPlan(
        frozenset(eligible),
        init_once,
        ineligible,
        guard_masks,
        init_dominated,
        tuple(checked),
        tuple(escape_written),
    ), FunctionCells(cell_indices, cell_stores, cell_reads, tuple(method_diagnostics))


def _written_guard_mask(
    facts: ModuleNamespaceFacts,
    layout: Sequence[str],
) -> int:
    """The guard bits of every layout name some code may store or delete:
    the module's own stores and deletes (module level and ``global``
    declarations) and the premise's named foreign writes."""
    written = set(facts.initializer_bindings | (facts.own_names or frozenset()))
    if facts.premise is not None and facts.premise.foreign_writes is not None:
        written |= set(facts.premise.foreign_writes)
    mask = 0
    for name in written & set(layout):
        mask |= BUILTIN_BINDING_GUARD_BITS.get(name, 0)
    return mask


# Initialization dominance (HF-846, call_anatomy.md 4.5a).

# (function name, op index, source line, op kind) of one op naming a symbol.
_Reference = tuple[str, int, int | None, str]
# A point of the init chain at which a function may start: during the op,
# or only after it completes (the op publishes the function's object).
_Entry = tuple[tuple[str, int], bool]
# Ops that name a function's code without running it.
_INERT_REFERENCES = frozenset({"code_slot_set", "trace_enter_slot"})
# Ops that name a function and also hold its object as an operand: a guarded
# call (the callee word) and a generator frame allocation (the function).
_OBJECT_MEDIATED = frozenset({"call_guarded", "alloc_task"})
# The call that records a new function object's metadata; it runs nothing.
_FUNCTION_METADATA = "molt_function_init_metadata_packed"
# Stores that publish a value into a module namespace without running it.
_PUBLICATIONS = frozenset({"module_set_attr", "dict_set"})
# An attribute store into the new function object itself (its
# ``__annotations__``, ``__annotate__`` or ``__type_params__``): the function
# type has no user ``__setattr__``, the fresh object holds no earlier value
# whose finalizer could run, and the store makes the stored value reachable
# from the function, not the function from anywhere else.
_RECEIVER_STORES = frozenset({"set_attr_generic_obj"})


def _symbol_references(
    program_functions: Sequence[Mapping[str, Any]],
) -> dict[str, list[_Reference]]:
    """Every op of the program that names a symbol in ``s_value``, by symbol:
    the ``func_new`` that creates a function's object, a direct call, and any
    other op that names it. A function runs only through such a reference."""
    found: dict[str, list[_Reference]] = {}
    for func in program_functions:
        name = func.get("name", "")
        for index, op in enumerate(func.get("ops", ())):
            symbol = op.get("s_value")
            if isinstance(symbol, str):
                found.setdefault(symbol, []).append(
                    (name, index, op.get("source_line"), str(op.get("kind")))
                )
    return found


def _object_entries(
    ops: Sequence[Mapping[str, Any]], function: str, index: int
) -> list[_Entry]:
    """Where the function object that ``ops[index]`` creates may first run:
    after each store that publishes it, or during any other use. The
    metadata call that completes the object and a store into the object
    itself run nothing."""
    out = ops[index].get("out")
    entries: list[_Entry] = []
    for at in range(index + 1, len(ops)):
        op = ops[at]
        args = op.get("args") or ()
        if out not in args:
            continue
        if op.get("kind") == "call" and op.get("s_value") == _FUNCTION_METADATA:
            continue
        if (
            op.get("kind") in _RECEIVER_STORES
            and len(args) == 2
            and args[0] == out
            and args[1] != out
        ):
            continue
        publication = (
            op.get("kind") in _PUBLICATIONS
            and len(args) == 3
            and args[2] == out
            and out not in args[:2]
        )
        entries.append(((function, at), publication))
    return entries


def _function_entries(
    module: str,
    funcs: Sequence[Mapping[str, Any]],
    chain: InitChain,
    body: frozenset[str],
    references: Mapping[str, Sequence[_Reference]],
) -> dict[str, tuple[frozenset[_Entry] | None, str]]:
    """For each of ``module``'s functions, the init-chain points at which it
    may start, or None when it may start at any time, with the reference
    that decides it. A reference in the init chain starts it at that op (a
    ``func_new`` through its object's uses); one inside another of the
    module's functions starts it no earlier than that function; a reference
    that runs many times is one start point, dominated or not as a whole. A
    function with no reference may start at any time, and so may one named by
    code outside the chain (another module, a chunk that may run twice),
    except through an op that holds the function's object
    (``_OBJECT_MEDIATED``): that object exists once its ``func_new`` in the
    chain has run and is published (the program reads no code object,
    ``code_rebinding``), so such a reference adds no start point before the
    object's own entries. A call by symbol alone (``call``,
    ``call_internal``) holds no object and may run first, as in an import
    cycle. The least fixed point admits recursion: a function that only names
    itself adds no entry."""
    candidates = sorted(
        func.get("name", "") for func in funcs if func.get("name", "") not in body
    )
    entries: dict[str, tuple[frozenset[_Entry] | None, str]] = {
        name: (
            (frozenset(), "")
            if references.get(name)
            else (None, f"{name} has no static reference")
        )
        for name in candidates
    }
    # Functions whose object a func_new in the init chain creates.
    created_in_chain = {
        name
        for name in candidates
        for user, index, _line, _kind in references.get(name, ())
        if user in chain.functions
        and chain.functions[user].ops[index].get("kind")
        in {"func_new", "func_new_closure"}
    }
    changed = True
    while changed:
        changed = False
        for name in candidates:
            current, _witness = entries[name]
            if current is None:
                continue
            found: set[_Entry] = set(current)
            verdict: tuple[frozenset[_Entry] | None, str] | None = None
            for user, index, line, kind in references.get(name, ()):
                if user == name:
                    continue
                if user in chain.functions:
                    ops = chain.functions[user].ops
                    if ops[index].get("kind") in _INERT_REFERENCES:
                        continue
                    if ops[index].get("kind") in {"func_new", "func_new_closure"}:
                        found.update(_object_entries(ops, user, index))
                    else:
                        found.add(((user, index), False))
                elif name in created_in_chain and (kind in _OBJECT_MEDIATED):
                    # This op holds the function's object, which exists only
                    # after the chain published it: already an entry.
                    continue
                elif user in entries:
                    starts = entries[user][0]
                    if starts is None:
                        verdict = (None, f"{user} names it at line {line}")
                        break
                    found.update(starts)
                else:
                    verdict = (None, f"{user} names it at line {line}")
                    break
            if verdict is None and found != current:
                verdict = (frozenset(found), "")
            if verdict is not None:
                entries[name] = verdict
                changed = True
    return entries


@dataclass(frozen=True)
class _ModuleExecution:
    module: str
    functions: Sequence[Mapping[str, Any]]
    references: Callable[[], Mapping[str, Sequence[_Reference]]]

    @cached_property
    def chain(self) -> InitChain:
        return InitChain(self.module, self.functions)

    @cached_property
    def body(self) -> frozenset[str]:
        return frozenset(
            func.get("name", "")
            for func in _module_body_functions(self.module, self.functions)
        )

    @cached_property
    def entries(self) -> dict[str, tuple[frozenset[_Entry] | None, str]]:
        return _function_entries(
            self.module, self.functions, self.chain, self.body, self.references()
        )


@dataclass(frozen=True)
class InitDominance:
    """Which reads of a module's InitOnce cells run only after their store."""

    # (function name, op index) of each dominated read.
    reads: frozenset[tuple[str, int]] = frozenset()
    # Why each other read of an InitOnce cell keeps the checked read.
    diagnostics: tuple[str, ...] = ()


def init_dominance(
    execution: _ModuleExecution,
    init_once: Mapping[str, InitOnce],
) -> InitDominance:
    """Decide, for each read of an InitOnce cell of ``module`` in its own
    code, whether it runs only after the cell's one store: in the init chain,
    the store dominates the read (``InitChain``); in one of the module's
    functions, the store dominates every point at which the function may
    start. The rule never crosses into another module, so an import cycle
    cannot run a read early."""
    from molt.cli.frontend_integration import _global_name_sites

    module = execution.module
    chain, entries, body = execution.chain, execution.entries, execution.body
    dominated: set[tuple[str, int]] = set()
    diagnostics: list[str] = []
    for func in execution.functions:
        name = func.get("name", "")
        index_of = {id(op): index for index, op in enumerate(func.get("ops", ()))}
        for op, global_name in _global_name_sites([func]):
            fact = init_once.get(global_name)
            if fact is None or op.get("kind") != "module_get_global":
                continue
            index = index_of[id(op)]
            store = (fact.function, fact.index)
            if name in body:
                if chain.dominates(store, (name, index)):
                    dominated.add((name, index))
                    continue
                reason = (
                    f"the store at line {fact.line} does not run before it on every path"
                    if name in chain.functions
                    else f"{name} is a body chunk outside the run-once chain"
                )
            else:
                starts, witness = entries.get(
                    name, (None, f"{name} is not a function of {module}")
                )
                if starts is not None and all(
                    chain.dominates(store, point) and (after or point != store)
                    for point, after in starts
                ):
                    dominated.add((name, index))
                    continue
                if starts is not None:
                    point = next(
                        point
                        for point, after in starts
                        if not (
                            chain.dominates(store, point) and (after or point != store)
                        )
                    )
                    line = chain.functions[point[0]].ops[point[1]].get("source_line")
                    witness = f"it may start at {point[0]} line {line}"
                reason = (
                    f"{name} may run before the store at line {fact.line}: {witness}"
                )
            diagnostics.append(
                f"{module}:{op.get('source_line')} {global_name}: checked read: {reason}"
            )
    return InitDominance(frozenset(dominated), tuple(diagnostics))


# Proven direct calls (HF-846 slice 2).

# The frontend's fact on a ``func_new`` of a synchronous def with positional
# parameters only (``OpIR::direct_shape``).
DIRECT_SHAPE_FIELD = "direct_shape"


def _proven_call_reason(
    call: Mapping[str, Any],
    producer: Mapping[str, Any] | None,
    callee_entry: Mapping[str, Any] | None,
) -> str | None:
    """Why a guarded call whose callee is a dominated read of an
    ``InitOnce(function)`` cell is not a proven call of its target, or None.

    The cell holds the object its one ``func_new`` created; the program
    reaches no code object (``code_rebinding``), so no code replaces that
    object's code. An exact positional call of a positional-only, synchronous
    def binds nothing, so ``__defaults__`` and ``__kwdefaults__`` cannot
    change it either. The entry adopts its arguments exactly when the call
    instruction does, which is the shape the admission call would check."""
    target = call.get("s_value")
    if producer is None or producer.get("kind") != "func_new":
        return "the cell's store is not a func_new without a closure"
    if producer.get("s_value") != target:
        return f"the cell holds {producer.get('s_value')}, not {target}"
    if producer.get(DIRECT_SHAPE_FIELD) is not True:
        return f"{target} has a parameter shape that needs binding"
    supplied = len(call.get("args") or ()) - 1
    if producer.get("value") != supplied:
        return f"the call passes {supplied} of {producer.get('value')} parameters"
    if callee_entry is None:
        return f"{target} has no function record"
    entry_adopts = "transferred" in (callee_entry.get("parameter_custody") or ())
    # The direct call borrows its callable, so only the arguments' custody
    # meets the entry's: a zero-argument call adopts nothing.
    call_adopts = "transferred" in (call.get("argument_custody") or ())[1:]
    if entry_adopts != call_adopts:
        return f"{target}'s entry custody differs from the call's"
    return None


def _init_once_views(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
) -> tuple[dict[str, int], dict[str, dict[str, dict[str, Any]]]]:
    """Each function's position in ``functions`` and, per module, its current
    body. Earlier integration steps replace function dicts copy-on-write in
    ``functions`` only (the global slot stamps), so a body is found by name
    there first."""
    position = {
        function.get("name", ""): index for index, function in enumerate(functions)
    }
    views = {
        module: {
            func.get("name", ""): functions[position[func.get("name", "")]]
            if func.get("name", "") in position
            else func
            for func in module_funcs
        }
        for module, module_funcs in module_functions.items()
    }
    return position, views


def _init_once_reads(ops: Sequence[Mapping[str, Any]]) -> dict[str, tuple[str, str]]:
    """``out`` -> (the module whose cell it reads, the global's name) for each
    ``init_once`` stamp in ``ops``: a module's own init-dominated reads
    (``stamp_global_slots``) and proven foreign reads of another module's cell
    (``stamp_foreign_globals``, HF-845)."""
    constants = {
        op.get("out"): op.get("s_value") for op in ops if op.get("kind") == "const_str"
    }
    reads: dict[str, tuple[str, str]] = {}
    for op in ops:
        slot = op.get("global_slot")
        args = op.get("args") or ()
        out = op.get("out")
        name = constants.get(args[1]) if len(args) > 1 else None
        if (
            op.get("kind") == "module_get_global"
            and isinstance(slot, Mapping)
            and slot.get("cell") is True
            and slot.get("init_once") is True
            and isinstance(out, str)
            and isinstance(name, str)
        ):
            reads[out] = (str(slot.get("module")), name)
    return reads


def _write_back(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
    position: Mapping[str, int],
    rewritten: Mapping[str, dict[str, Any]],
) -> None:
    """Publish rewritten bodies to both views."""
    for name, function in rewritten.items():
        if name in position:
            functions[position[name]] = function
    for module_funcs in module_functions.values():
        for index, func in enumerate(module_funcs):
            name = func.get("name", "")
            if name in position:
                module_funcs[index] = functions[position[name]]


def _task_constructor(
    call: Mapping[str, Any],
    producer: Mapping[str, Any] | None,
    has_record: Callable[[str], bool],
    following: Mapping[str, Any],
    tuple_name: Callable[[], str],
) -> list[dict[str, Any]] | str | None:
    """The ``alloc_task`` that a generic call of a generator function is.

    The callee is a dominated read of an ``InitOnce(function)`` cell whose one
    ``func_new`` created a generator function. Calling that object allocates
    the generator's frame and stores the arguments in its parameter slots;
    the body runs at the first ``next``. An exact positional call of a def with
    positional parameters only binds nothing, so the call is that allocation:
    the frame's poll function, its byte size, the function and the arguments,
    with no callable to find or bind. ``alloc_task`` borrows the function (its
    globals, builtins and code become the frame context) and retains each
    argument into its slot, so the call's argument custody goes.

    A generator with only fixed positional parameters and a final vararg slot
    needs one tuple of the surplus arguments. Pack it at the original call
    after argument evaluation and branch on allocation failure before making
    the frame. Omitted fixed arguments still need the ordinary defaults binder.

    Returns the replacement ops; the reason a generator call stays generic; or
    None when the cell's function is not a generator function."""
    if (
        producer is None
        or producer.get("kind") != "func_new"
        or producer.get("task_kind") != "generator"
    ):
        return None
    poll = producer.get("s_value")
    if not isinstance(poll, str) or not has_record(poll):
        return f"{poll} has no function record"
    args = list(call.get("args") or ())
    supplied = len(args) - 1
    packing: list[dict[str, object]] = []
    slot = producer.get("task_vararg_slot")
    if slot is not None:
        if (
            type(slot) is not int
            or slot < 0
            or producer.get("value") != slot + 1
            or producer.get(DIRECT_SHAPE_FIELD)
        ):
            return f"{poll} has an invalid vararg slot"
        if supplied < slot:
            return f"the call omits fixed parameters of {poll}"
        if (
            following.get("kind") != "check_exception"
            or following.get("exception_edge") != "origin"
        ):
            return f"{poll} has no checked edge for vararg packing"
        name = tuple_name()
        location = {
            key: call[key]
            for key in ("source_line", "col_offset", "end_col_offset")
            if key in call
        }
        packing = [
            {
                "kind": "tuple_new",
                "args": args[1 + slot :],
                "out": name,
                "type_hint": "tuple",
                **location,
            },
            dict(following),
        ]
        args = [*args[: 1 + slot], name]
    elif producer.get(DIRECT_SHAPE_FIELD) is not True:
        return f"{poll} has a parameter shape that needs binding"
    elif producer.get("value") != supplied:
        return f"the call passes {supplied} of {producer.get('value')} parameters"
    frame_bytes = producer.get("task_closure_size")
    if not isinstance(frame_bytes, int) or isinstance(frame_bytes, bool):
        return f"{poll} has no frame size"
    task = {key: value for key, value in call.items() if key != "argument_custody"}
    task.update(
        kind="alloc_task",
        s_value=poll,
        value=frame_bytes,
        task_kind="generator",
        # alloc_task_operands: the function, then the payload words.
        args=args,
    )
    return [*packing, task]


def _intrinsic_call(
    call: Mapping[str, Any], producer: Mapping[str, Any] | None
) -> dict[str, Any] | str:
    """The direct intrinsic call that a generic call of an
    ``InitOnce(intrinsic)`` cell is, or why it stays generic.

    The cell's one store binds a ``builtin_func`` of the canonical intrinsic
    ``S`` with ABI arity ``n``; no code in the program writes the cell
    afterward. A positional call with exactly ``n`` arguments therefore calls
    ``S``: the same ``call`` the frontend emits for ``require_intrinsic("S")(...)``
    (``_emit_intrinsic_call``). A runtime intrinsic is no function of the
    program, so its direct entry borrows its arguments, as there."""
    if producer is None or producer.get("kind") != "builtin_func":
        return "the cell's store is not a builtin_func"
    symbol = producer.get("s_value")
    arity = producer.get("value")
    if not isinstance(symbol, str) or not isinstance(arity, int):
        return "the cell's builtin_func has no symbol and arity"
    args = list(call.get("args") or ())
    supplied = len(args) - 1
    if supplied != arity:
        return f"the call passes {supplied} of {arity} parameters"
    direct = {
        key: value
        for key, value in call.items()
        if key in {"out", "type_hint", "source_line", "col_offset", "end_col_offset"}
    }
    direct.update(kind="call", s_value=symbol, args=args[1:])
    return direct


def prove_init_once_calls(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
    plan: StaticCellPlan,
) -> list[str]:
    """Mark each ``call_guarded`` whose callee is an ``init_once`` read of an
    ``InitOnce(function)`` cell holding its target's object ``proven_target``
    (copy-on-write, as the global slot stamps are). A generic ``call_func``
    whose callee is such a read of a generator function becomes the frame's
    ``alloc_task`` (``_task_constructor``), and one whose callee is a read of
    an ``InitOnce(intrinsic)`` cell becomes the direct intrinsic call
    (``_intrinsic_call``). Return a diagnostic for each such call that keeps
    its guards or stays generic."""
    position, views = _init_once_views(functions, module_functions)
    diagnostics: list[str] = []
    rewritten: dict[str, dict[str, Any]] = {}
    for module, current in views.items():
        for name, func in current.items():
            ops = func.get("ops", ())
            reads = _init_once_reads(ops)
            changed: dict[int, list[dict[str, object]]] = {}
            names: set[str] | None = None

            def tuple_name() -> str:
                nonlocal names
                if names is None:
                    names = set(func.get("params", ()))
                    for instr in ops:
                        names.update(instr.get("args") or ())
                        names.update(
                            instr[key] for key in ("out", "var") if key in instr
                        )
                candidate = f"__molt_varargs_{index}"
                while candidate in names:
                    candidate += "_"
                names.add(candidate)
                return candidate

            for index, op in enumerate(ops):
                kind = op.get("kind")
                if kind not in {"call_guarded", "call_func"}:
                    continue
                args = op.get("args") or ()
                read = reads.get(args[0]) if args else None
                if read is None:
                    continue
                cell_module, global_name = read
                fact = plan.init_once.get(cell_module, {}).get(global_name)
                if (
                    fact is not None
                    and fact.value_kind == "intrinsic"
                    and kind == "call_func"
                ):
                    direct = _intrinsic_call(op, fact.producer)
                    if isinstance(direct, str):
                        diagnostics.append(
                            f"{module}:{op.get('source_line')} {global_name}(): "
                            f"generic intrinsic call: {direct}"
                        )
                    else:
                        changed[index] = [direct]
                    continue
                if fact is None or fact.value_kind != "function":
                    continue
                producer = fact.producer
                if fact.function not in position and fact.function not in current:
                    diagnostics.append(
                        f"{module}:{op.get('source_line')} {global_name}(): "
                        f"guarded call: {cell_module}'s store has no function record"
                    )
                    continue
                if kind == "call_func":
                    following = ops[index + 1] if index + 1 < len(ops) else {}
                    task = _task_constructor(
                        op, producer, position.__contains__, following, tuple_name
                    )
                    if isinstance(task, str):
                        diagnostics.append(
                            f"{module}:{op.get('source_line')} {global_name}(): "
                            f"generic generator call: {task}"
                        )
                    elif task is not None:
                        changed[index] = task
                    continue
                target = str(op.get("s_value"))
                callee = functions[position[target]] if target in position else None
                reason = _proven_call_reason(op, producer, callee)
                if reason is not None:
                    diagnostics.append(
                        f"{module}:{op.get('source_line')} {global_name}(): "
                        f"guarded call: {reason}"
                    )
                    continue
                changed[index] = [{**op, "proven_target": True}]
            if changed:
                rewritten[name] = {
                    **func,
                    "ops": [
                        item
                        for index, op in enumerate(ops)
                        for item in changed.get(index, (op,))
                    ],
                }
    _write_back(functions, module_functions, position, rewritten)
    return diagnostics


# Constants whose copies no program can tell apart: an inline int, a bool,
# None and a float are immediate words, so a copy is the same object (``is``
# holds) with the same value. A big int, a string or a tuple is a heap object
# whose identity a copy would change; their reads keep the cell.
_IMMEDIATE_CONSTANTS = frozenset({"const", "const_bool", "const_none", "const_float"})


def fold_init_once_constants(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, list[dict[str, Any]]],
    plan: StaticCellPlan,
) -> int:
    """Replace each ``init_once`` read of a cell whose one store binds an
    immediate constant with that constant (copy-on-write).

    The InitOnce fact proves the store runs before the read and that no code in
    the program writes or deletes the global afterward (closed world,
    call_anatomy.md 4.5a), so the read always finds the value the store bound.
    An immediate constant's copy is indistinguishable from it, so the read is
    the constant: no cell load, no type test, and a typed value the optimizer
    can use. Returns the number of reads folded."""
    position, views = _init_once_views(functions, module_functions)
    rewritten: dict[str, dict[str, Any]] = {}
    folded = 0
    for current in views.values():
        for name, func in current.items():
            ops = func.get("ops", ())
            reads = _init_once_reads(ops)
            if not reads:
                continue
            changed: dict[int, dict[str, Any]] = {}
            for index, op in enumerate(ops):
                read = (
                    reads.get(op.get("out"))
                    if op.get("kind") == "module_get_global"
                    else None
                )
                if read is None:
                    continue
                cell_module, global_name = read
                fact = plan.init_once.get(cell_module, {}).get(global_name)
                if fact is None or fact.value_kind != "other":
                    continue
                producer = fact.producer
                if producer is None or producer.get("kind") not in _IMMEDIATE_CONSTANTS:
                    continue
                constant = {
                    key: value
                    for key, value in producer.items()
                    if key in {"kind", "value", "f_value", "type_hint"}
                }
                source = {
                    key: value
                    for key, value in op.items()
                    if key in {"source_line", "col_offset", "end_col_offset"}
                }
                changed[index] = {**constant, **source, "out": op["out"]}
            if changed:
                folded += len(changed)
                rewritten[name] = {
                    **func,
                    "ops": [changed.get(index, op) for index, op in enumerate(ops)],
                }
    _write_back(functions, module_functions, position, rewritten)
    return folded


# Function cells (design 78, 5.9).


# The loads that hand a proven call its callee's function object: a method
# load on an exact receiver (5.7) and a static ``super()`` selection (5.10).
_METHOD_LOADS = frozenset({"load_method", "load_super_method"})


@dataclass(frozen=True)
class FunctionCells:
    """Each cell-eligible module's function cells: the words after its layout
    that hold a proven callee's one function object, and the sites that store
    and read them. Sites are named by value, so op insertions keep them."""

    # module -> callee symbol -> word index in the module's cell block.
    cells: Mapping[str, Mapping[str, int]] = field(default_factory=dict)
    # function name -> `func_new` result -> (module, callee symbol).
    stores: Mapping[str, Mapping[str, tuple[str, str]]] = field(default_factory=dict)
    # function name -> proven `load_method` callable -> (module, callee symbol).
    reads: Mapping[str, Mapping[str, tuple[str, str]]] = field(default_factory=dict)
    # Why each other proven method load keeps `load_method`.
    diagnostics: tuple[str, ...] = ()

    def counts(self) -> dict[str, int]:
        return {module: len(rows) for module, rows in self.cells.items()}


def _consuming_proven_call(
    ops: Sequence[Mapping[str, Any]], start: int, callable_var: str
) -> Mapping[str, Any] | None:
    """The proven ``call_guarded`` whose callable is ``callable_var``."""
    for op in ops[start + 1 :]:
        if (
            op.get("kind") == "call_guarded"
            and op.get("proven_target")
            and (op.get("args") or (None,))[0] == callable_var
        ):
            return op
    return None


def _proven_method_loads(
    execution: _ModuleExecution,
    initializer: ModuleInitializerFacts,
    diagnostics: list[str],
) -> Iterator[tuple[str, str, str, CallablePublication]]:
    """Yield proven loads dominated by their callee's sole run-once maker."""
    publications = initializer.callable_publications
    module = execution.module
    for func in execution.functions:
        name = func.get("name", "")
        ops = func.get("ops", ())
        for index, op in enumerate(ops):
            if op.get("kind") not in _METHOD_LOADS or not op.get("proven_target"):
                continue
            callable_var = op.get("var")
            call = (
                _consuming_proven_call(ops, index, callable_var)
                if isinstance(callable_var, str)
                else None
            )
            symbol = call.get("s_value") if call is not None else None
            if not isinstance(callable_var, str) or not isinstance(symbol, str):
                continue
            pair = op.get("out")
            if any(pair in (other.get("args") or ()) for other in ops):
                diagnostics.append(
                    f"{module}:{op.get('source_line')} {symbol}: method load kept: "
                    "its bound pair is read"
                )
                continue
            makers = publications.get(symbol, ())
            reason = None
            maker = None
            if len(makers) != 1:
                reason = f"{len(makers)} function makers make {symbol}"
            else:
                (maker,) = makers
                if not maker.runs_once:
                    reason = f"{symbol}'s func_new does not run once in {module}'s init"
            if reason is None and maker is not None:
                store = maker.point
                point = (name, index)
                chain = execution.chain
                if name in execution.body:
                    dominated = chain.dominates(store, point)
                else:
                    starts, _witness = execution.entries.get(name, (None, ""))
                    dominated = starts is not None and all(
                        chain.dominates(store, start) and (after or start != store)
                        for start, after in starts
                    )
                if not dominated:
                    reason = f"{name} may run before {symbol}'s func_new"
            if reason is not None or maker is None:
                diagnostics.append(
                    f"{module}:{op.get('source_line')} {symbol}: method load kept: {reason}"
                )
                continue
            yield name, callable_var, symbol, maker


def apply_function_cells(functions: list[dict[str, Any]], plan: FunctionCells) -> None:
    """Store each planned callee's object into its cell after its
    ``func_new``, and read it at each planned site in place of its method
    load. A rewritten function is a copy-on-write replacement."""
    if not plan.cells:
        return
    for position, func in enumerate(functions):
        name = func.get("name", "")
        stores = plan.stores.get(name, {})
        reads = plan.reads.get(name, {})
        if not stores and not reads:
            continue
        ops: list[dict[str, Any]] = []
        for op in func.get("ops", ()):
            callable_var = op.get("var")
            if (
                op.get("kind") in _METHOD_LOADS
                and op.get("proven_target")
                and callable_var in reads
            ):
                module, symbol = reads[callable_var]
                ops.append(
                    {
                        "kind": "static_cell_load",
                        "out": callable_var,
                        "global_slot": {
                            "module": module,
                            "index": plan.cells[module][symbol],
                            "cell": True,
                            "init_once": True,
                        },
                        **{k: op[k] for k in ("source_line",) if k in op},
                    }
                )
                continue
            ops.append(op)
            made = op.get("out")
            if op.get("kind") in FUNCTION_MAKERS and made in stores:
                module, symbol = stores[made]
                base = f"function_cells_base_{made}"
                ops.append({"kind": "static_cell_base", "s_value": module, "out": base})
                ops.append(
                    {
                        "kind": "static_cell_set",
                        "args": [base, made],
                        "global_slot": {
                            "module": module,
                            "index": plan.cells[module][symbol],
                            "cell": True,
                        },
                    }
                )
        functions[position] = {**func, "ops": ops}
