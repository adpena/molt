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
    (``PythonNamespacePremise.namespace_reflected``).
C3. Every writer of ``M``'s namespace is named: ``M``'s own deferred writes and
    the premise's foreign writes are known sets.
C4. ``M``'s body does not star-import (which binds names the layout lacks), and
    the program writes no ``sys.modules``, import hook or ``__import__``
    (which could rebind ``M``'s identity and re-run its body).
C5. Program-wide: no extension module (the C-API can take any module's
    dictionary) and no isolate thread (``molt_thread_spawn``: the cells are
    one per process until stage 1 gives each isolate its own block).
C6. The body runs once per process: the entry module, whose body runs as
    ``__main__``, is not also imported by its own name, which would run the
    same body for a second module object over the same cells.

Each layout name of an eligible module also gets its ``InitOnce`` fact when
the module's body stores it once, unconditionally, at initialization, and no
code writes or deletes it afterward (the F1/F2 facts of
``closed_world_bindings``). A read of an InitOnce cell in the module's own code
that runs only after that store (``init_dominance``) is one load with no
unbound test and no retain (HF-846); every other read of an InitOnce cell keeps
the checked read, and the build diagnostics name the failed fact.
"""

from __future__ import annotations

from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from molt.compiler_analysis.builtin_binding_guards_generated import (
    BUILTIN_BINDING_GUARD_BITS,
)
from molt.cli.closed_world_bindings import (
    _IMPORT_REDIRECTS,
    _IMPORT_TRANSACTION,
    ModuleNamespaceFacts,
    _body_analysis,
    _module_body_functions,
    InitChain,
)

# The isolate thread entry: an isolate re-runs every module body in its own
# runtime, which would share this process's cells.
_ISOLATE_SPAWN = "molt_thread_spawn"
_NAMESPACE_WRITES = frozenset(
    {"module_set_attr", "dict_set", "module_del_global", "module_del_global_if_present"}
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
            "ineligible": dict(sorted(self.ineligible.items())),
        }


def _program_blocker(
    module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
    extension_modules: Collection[str],
) -> str | None:
    if extension_modules:
        first = sorted(extension_modules)[0]
        return f"the program loads extension module {first}, whose C-API can take any module dictionary"
    for module, funcs in sorted(module_functions.items()):
        for func in funcs:
            for op in func.get("ops", ()):
                if (
                    op.get("kind") in {"call", "builtin_func"}
                    and op.get("s_value") == _ISOLATE_SPAWN
                ):
                    return (
                        f"{module}:{op.get('source_line')} spawns an isolate thread, "
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
    if premise.foreign_writes is None:
        return "the program writes some module namespace by a computed name"
    if premise.member_writes & _IMPORT_REDIRECTS:
        return "the program writes sys.modules, import hooks or __import__"
    _stores, _deletes, star, _builtins = _body_analysis(module, funcs)
    if star:
        return "its body star-imports"
    if module_object_site(module, funcs) is None:
        return "its body does not create its module object"
    return None


def _init_once_facts(
    module: str,
    funcs: Sequence[Mapping[str, Any]],
    facts: ModuleNamespaceFacts,
    layout: Sequence[str],
) -> dict[str, InitOnce]:
    stores, deletes, _star, _builtins = _body_analysis(module, funcs)
    producers: dict[tuple[str, str], Mapping[str, Any]] = {}
    for func in _module_body_functions(module, funcs):
        for op in func.get("ops", ()):
            out = op.get("out")
            if isinstance(out, str):
                producers[(func.get("name", ""), out)] = op
    found: dict[str, InitOnce] = {}
    for name in layout:
        if facts.stable_reason(name) is not None or name in deletes:
            continue
        rows = stores.get(name, [])
        if len(rows) != 1 or not rows[0].unconditional:
            continue
        producer = producers.get((rows[0].function, rows[0].value or ""), {})
        kind = producer.get("kind")
        if kind == "call" and producer.get("s_value") == _IMPORT_TRANSACTION:
            value_kind: InitOnceKind = "module"
        else:
            value_kind = _VALUE_KINDS.get(str(kind), "other")
        found[name] = InitOnce(
            value_kind, rows[0].function, rows[0].index, rows[0].line
        )
    return found


def plan_static_module_cells(
    module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
    facts: Mapping[str, ModuleNamespaceFacts],
    global_layouts: Mapping[str, tuple[str, ...]],
    registered_modules: Collection[str],
    extension_modules: Collection[str] = (),
    entry_module: str | None = None,
    dep_closures: Mapping[str, Collection[str]] | None = None,
    program_functions: Sequence[Mapping[str, Any]] = (),
) -> StaticCellPlan:
    """Decide C1-C6 for every module with a registry row and a layout, and
    which reads of each eligible module's InitOnce cells are dominated by
    their store. ``program_functions`` is every function of the program:
    a function runs only through a reference some op makes to its symbol."""
    candidates = sorted(
        module
        for module in registered_modules
        if global_layouts.get(module) and module in module_functions
    )
    blocker = _program_blocker(module_functions, extension_modules)
    if blocker is not None:
        return StaticCellPlan(ineligible=dict.fromkeys(candidates, blocker))
    eligible: set[str] = set()
    guard_masks: dict[str, int] = {}
    init_once: dict[str, dict[str, InitOnce]] = {}
    init_dominated: dict[str, frozenset[tuple[str, int]]] = {}
    checked: list[str] = []
    references = _symbol_references(program_functions)
    ineligible: dict[str, str] = {}
    for module in candidates:
        funcs = module_functions[module]
        module_facts = facts.get(module)
        reason = _module_reason(module, funcs, module_facts)
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
        guard_masks[module] = _written_guard_mask(
            funcs, module_facts, global_layouts[module]
        )
        rows = _init_once_facts(module, funcs, module_facts, global_layouts[module])
        if rows:
            init_once[module] = rows
            dominance = init_dominance(module, funcs, rows, references)
            if dominance.reads:
                init_dominated[module] = dominance.reads
            checked.extend(dominance.diagnostics)
    return StaticCellPlan(
        frozenset(eligible),
        init_once,
        ineligible,
        guard_masks,
        init_dominated,
        tuple(checked),
    )


def _written_guard_mask(
    funcs: Sequence[Mapping[str, Any]],
    facts: ModuleNamespaceFacts,
    layout: Sequence[str],
) -> int:
    """The guard bits of every layout name some code may store or delete:
    the module's own stores and deletes (module level and ``global``
    declarations) and the premise's named foreign writes."""
    from molt.cli.frontend_integration import _global_name_sites

    written = {
        name
        for op, name in _global_name_sites(funcs)
        if op.get("kind") in _NAMESPACE_WRITES
    }
    written |= facts.own_names or frozenset()
    if facts.premise is not None and facts.premise.foreign_writes is not None:
        written |= set(facts.premise.foreign_writes)
    mask = 0
    for name in written & set(layout):
        mask |= BUILTIN_BINDING_GUARD_BITS.get(name, 0)
    return mask


# Initialization dominance (HF-846, call_anatomy.md 4.5a).

# (function name, op index, source line) of one op naming a symbol.
_Reference = tuple[str, int, int | None]
# A point of the init chain at which a function may start: during the op,
# or only after it completes (the op publishes the function's object).
_Entry = tuple[tuple[str, int], bool]
# Ops that name a function's code without running it.
_INERT_REFERENCES = frozenset({"code_slot_set", "trace_enter_slot"})
# The call that records a new function object's metadata; it runs nothing.
_FUNCTION_METADATA = "molt_function_init_metadata_packed"
# Stores that publish a value into a module namespace without running it.
_PUBLICATIONS = frozenset({"module_set_attr", "dict_set"})


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
                    (name, index, op.get("source_line"))
                )
    return found


def _object_entries(
    ops: Sequence[Mapping[str, Any]], function: str, index: int
) -> list[_Entry]:
    """Where the function object that ``ops[index]`` creates may first run:
    after each store that publishes it, or during any other use. The
    metadata call that completes the object runs nothing."""
    out = ops[index].get("out")
    entries: list[_Entry] = []
    for at in range(index + 1, len(ops)):
        op = ops[at]
        args = op.get("args") or ()
        if out not in args:
            continue
        if op.get("kind") == "call" and op.get("s_value") == _FUNCTION_METADATA:
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
    references: Mapping[str, Sequence[_Reference]],
) -> dict[str, tuple[frozenset[_Entry] | None, str]]:
    """For each of ``module``'s functions, the init-chain points at which it
    may start, or None when it may start at any time, with the reference
    that decides it. A reference in the init chain starts it at that op (a
    ``func_new`` through its object's uses); one inside another of the
    module's functions starts it no earlier than that function; a reference
    that runs many times is one start point, dominated or not as a whole. A
    function with no reference, or one named by code outside the chain
    (another module, a chunk that may run twice), may start at any time. The least fixed point
    admits recursion: a function that only names itself adds no entry."""
    body = {func.get("name", "") for func in _module_body_functions(module, funcs)}
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
    changed = True
    while changed:
        changed = False
        for name in candidates:
            current, _witness = entries[name]
            if current is None:
                continue
            found: set[_Entry] = set(current)
            verdict: tuple[frozenset[_Entry] | None, str] | None = None
            for user, index, line in references.get(name, ()):
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
class InitDominance:
    """Which reads of a module's InitOnce cells run only after their store."""

    # (function name, op index) of each dominated read.
    reads: frozenset[tuple[str, int]] = frozenset()
    # Why each other read of an InitOnce cell keeps the checked read.
    diagnostics: tuple[str, ...] = ()


def init_dominance(
    module: str,
    funcs: Sequence[Mapping[str, Any]],
    init_once: Mapping[str, InitOnce],
    references: Mapping[str, Sequence[_Reference]],
) -> InitDominance:
    """Decide, for each read of an InitOnce cell of ``module`` in its own
    code, whether it runs only after the cell's one store: in the init chain,
    the store dominates the read (``InitChain``); in one of the module's
    functions, the store dominates every point at which the function may
    start. The rule never crosses into another module, so an import cycle
    cannot run a read early."""
    from molt.cli.frontend_integration import _global_name_sites

    if not init_once:
        return InitDominance()
    chain = InitChain(module, funcs)
    entries = _function_entries(module, funcs, chain, references)
    body = {func.get("name", "") for func in _module_body_functions(module, funcs)}
    dominated: set[tuple[str, int]] = set()
    diagnostics: list[str] = []
    for func in funcs:
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
