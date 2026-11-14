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
``closed_world_bindings``). A later slice consumes it; reads keep their unbound
test until initialization dominance is proven.
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
    ``value_kind`` for the rest of the program."""

    value_kind: InitOnceKind


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

    def diagnostics(self) -> dict[str, object]:
        return {
            "eligible": sorted(self.modules),
            "init_once": {
                module: {name: fact.value_kind for name, fact in sorted(rows.items())}
                for module, rows in sorted(self.init_once.items())
            },
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
    if premise.code_rebinding:
        return (
            "some code reads a code object (__code__, f_code, gi_code, cr_code "
            "or ag_code) or loads a native extension, so FunctionType may run "
            "this module's functions over another globals mapping"
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
        found[name] = InitOnce(value_kind)
    return found


def plan_static_module_cells(
    module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
    facts: Mapping[str, ModuleNamespaceFacts],
    global_layouts: Mapping[str, tuple[str, ...]],
    registered_modules: Collection[str],
    extension_modules: Collection[str] = (),
    entry_module: str | None = None,
    dep_closures: Mapping[str, Collection[str]] | None = None,
) -> StaticCellPlan:
    """Decide C1-C6 for every module with a registry row and a layout."""
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
    return StaticCellPlan(frozenset(eligible), init_once, ineligible, guard_masks)


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
