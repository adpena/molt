from __future__ import annotations

from collections.abc import Collection, Iterator, Mapping, Sequence
from typing import TYPE_CHECKING, Any, cast

from molt.frontend import ClassId, ClassModel, layout_definition_symbol
from molt.cli.models import _FrontendIntegrationState
from molt.cli.module_registry import is_dunder_name
from molt.frontend._types import _MOLT_MODULE_CHUNK_PARAM
from molt.frontend.module_publication import (
    FunctionAssemblyMetadata,
    inspect_source_module_publication,
    parse_function_metadata,
    reject_embedded_function_metadata,
    reject_embedded_operation_metadata,
)


if TYPE_CHECKING:
    from molt.cli.function_records import FunctionDigestSeed


def _register_global_code_id_with_state(
    integration_state: _FrontendIntegrationState,
    symbol: str,
) -> int:
    code_id = integration_state.global_code_ids.get(symbol)
    if code_id is None:
        code_id = len(integration_state.global_code_ids)
        integration_state.global_code_ids[symbol] = code_id
    return code_id


def _module_class_definitions(local_classes: Mapping[ClassId, ClassModel]) -> set[str]:
    """The class statement symbols a module owns: the ones it stamps (HF-220)."""
    return {
        symbol
        for class_id, info in local_classes.items()
        if (symbol := layout_definition_symbol(class_id, info)) is not None
    }


def _plan_module_code_slots(
    module_name: str,
    funcs: list[dict[str, Any]],
    local_symbols: set[str],
    class_definitions: Collection[str],
) -> tuple[dict[str, dict[str, Any]], dict[str, None], dict[str, str]]:
    """Validate one module and stage its ordered additions without mutation."""
    if "molt_main" in local_symbols:
        raise ValueError(
            f"Noncanonical module code owner 'molt_main' in module {module_name}"
        )
    owned_functions: dict[str, dict[str, Any]] = {}
    code_symbols: dict[str, None] = {}
    class_references: dict[str, str] = {}
    for func in funcs:
        reject_embedded_function_metadata(func)
        name = func.get("name")
        if not isinstance(name, str) or name in owned_functions:
            raise ValueError(f"Invalid or duplicate executable function owner {name!r}")
        owned_functions[name] = func
        if func.get("name") == "molt_main":
            raise ValueError(
                f"Legacy program entry 'molt_main' in module {module_name}"
            )
        for op in func.get("ops", []):
            reject_embedded_operation_metadata(op)
            kind = op.get("kind")
            if kind == "code_slots_init":
                raise ValueError(
                    f"Module {module_name} cannot initialize program code slots"
                )
            if kind in {"code_slot_set", "trace_enter_slot"}:
                symbol = op.get("s_value")
                value = op.get("value")
                # A frame entry may carry its synchronous frame's home count
                # and clear order; the symbol alone is the code identity.
                frame_shape = (
                    kind == "trace_enter_slot" and type(value) is int and value >= 0
                )
                if symbol not in local_symbols or not (value is None or frame_shape):
                    raise ValueError(
                        f"Missing canonical code owner {symbol!r} in module {module_name}"
                    )
                code_symbols.setdefault(symbol, None)
            definition = op.get("class_definition")
            if definition is not None:
                # A stamp names its own module's statement; a layout guard may
                # name any module's statement (an imported class), so its owner
                # is checked once the whole program is integrated.
                if (
                    kind == "class_set_definition"
                    and definition not in class_definitions
                ):
                    raise ValueError(
                        f"Class statement {definition!r} is stamped outside its "
                        f"owning module {module_name}"
                    )
                class_references.setdefault(definition, module_name)
                code_symbols.setdefault(definition, None)
    return owned_functions, code_symbols, class_references


# Ops whose name operand is a global of the module that owns the executing
# frame (``module_get_global`` reads the frame's globals) or of the module
# object operand (the others, when that operand is the module's own object).
_FRAME_GLOBAL_OPS = frozenset({"module_get_global"})
_MODULE_OBJECT_GLOBAL_OPS = frozenset(
    {
        "module_set_attr",
        "module_get_attr",
        "module_del_global",
        "module_del_global_if_present",
    }
)
# The sites a compiled global slot serves: reads of the frame's globals and
# stores into the module's own namespace (module level) or the frame's
# globals (``global`` declarations in functions).
_SLOTTED_SITE_OPS = frozenset({"module_get_global", "module_set_attr", "dict_set"})


def _global_name_sites(
    funcs: Sequence[Mapping[str, Any]],
) -> Iterator[tuple[Mapping[str, Any], str]]:
    """Identify materialized global access sites for slot stamping."""
    for func in funcs:
        constants: dict[str, str] = {}
        globals_dicts: set[str] = set()
        own_modules: set[str] = {
            param
            for param in func.get("params") or ()
            if param == _MOLT_MODULE_CHUNK_PARAM
        }
        for op in func.get("ops", ()):
            kind = op.get("kind")
            out = op.get("out")
            if kind == "const_str":
                value = op.get("s_value")
                if isinstance(out, str) and isinstance(value, str):
                    constants[out] = value
                continue
            if kind == "module_new" and isinstance(out, str):
                own_modules.add(out)
                continue
            if kind == "call" and op.get("s_value") == "molt_globals_builtin":
                if isinstance(out, str):
                    globals_dicts.add(out)
                continue
            args = op.get("args") or ()
            if len(args) < 2 or args[1] not in constants:
                continue
            if (
                kind in _FRAME_GLOBAL_OPS
                or (kind in _MODULE_OBJECT_GLOBAL_OPS and args[0] in own_modules)
                or (kind == "dict_set" and args[0] in globals_dicts)
            ):
                yield op, constants[args[1]]


def module_global_layout(names: Collection[str]) -> tuple[str, ...]:
    """Sorted slots for names the module's source may write or delete."""
    return tuple(sorted({name for name in names if not is_dunder_name(name)}))


def stamp_global_slots(
    functions: list[dict[str, Any]],
    module_functions: Mapping[str, Sequence[dict[str, Any]]],
    global_layouts: Mapping[str, tuple[str, ...]],
    registered_modules: Collection[str],
    static_modules: Collection[str] = (),
    guard_masks: Mapping[str, int] | None = None,
    init_dominated: Mapping[str, Collection[tuple[str, int]]] | None = None,
    function_cells: Mapping[str, int] | None = None,
) -> None:
    """Mark each read and store of a slotted global with its slot
    ``{"module": M, "index": k}``: slot k of M's layout. Only modules with a
    registry row (which binds the runtime table) are stamped; everything else
    keeps the by-name op unchanged. A stamped function is a copy-on-write
    replacement in ``functions``, so its admitted body digest is never stale.

    A module in ``static_modules`` (``static_module_cells``) keeps its globals
    in static cells instead: its stamps carry ``"cell": true`` (slot k is the
    k-th word of its cell block), and its init binds the block to its module
    object right after creating it, before any store
    (``molt_static_cells_bind``). A read in ``init_dominated`` (function
    name, op index) runs only after its InitOnce cell's store; its stamp also
    carries ``"init_once": true``, and it becomes one load (HF-846).
    """
    from molt.cli.static_module_cells import module_object_site

    position = {id(function): index for index, function in enumerate(functions)}
    for module_name, funcs in module_functions.items():
        if module_name not in registered_modules:
            continue
        cells = module_name in static_modules
        index_of = {
            name: index
            for index, name in enumerate(global_layouts.get(module_name, ()))
        }
        bind_site = module_object_site(module_name, funcs) if cells else None
        dominated = (init_dominated or {}).get(module_name, frozenset())
        for func in funcs:
            stamps: dict[int, dict[str, Any]] = {}
            positions = (
                {id(op): at for at, op in enumerate(func["ops"])} if dominated else {}
            )
            for op, name in _global_name_sites([func]):
                index = index_of.get(name)
                if index is not None and op.get("kind") in _SLOTTED_SITE_OPS:
                    stamp: dict[str, Any] = {"module": module_name, "index": index}
                    if cells:
                        stamp["cell"] = True
                        if (func.get("name", ""), positions.get(id(op))) in dominated:
                            stamp["init_once"] = True
                    stamps[id(op)] = stamp
            binds = bind_site is not None and bind_site[0] == func.get("name")
            if (not stamps and not binds) or id(func) not in position:
                continue
            ops = [
                {**op, "global_slot": stamps[id(op)]} if id(op) in stamps else op
                for op in func["ops"]
            ]
            if binds:
                assert bind_site is not None
                at = bind_site[1] + 1
                if at < len(ops) and ops[at].get("kind") == "check_exception":
                    at += 1
                module_var = ops[bind_site[1]]["out"]
                base = f"static_cells_base_{module_var}"
                mask = f"static_cells_guards_{module_var}"
                function_cell_count = f"static_cells_functions_{module_var}"
                ops[at:at] = [
                    {"kind": "static_cell_base", "s_value": module_name, "out": base},
                    {
                        "kind": "const",
                        "value": (guard_masks or {}).get(module_name, 0),
                        "out": mask,
                    },
                    {
                        "kind": "const",
                        "value": (function_cells or {}).get(module_name, 0),
                        "out": function_cell_count,
                    },
                    {
                        "kind": "call",
                        "s_value": "molt_static_cells_bind",
                        "args": [module_var, base, mask, function_cell_count],
                        "out": f"static_cells_bound_{module_var}",
                    },
                ]
            functions[position[id(func)]] = {**func, "ops": ops}


def stamp_foreign_globals(
    functions: list[dict[str, Any]],
    global_layouts: Mapping[str, tuple[str, ...]],
    static_modules: Collection[str],
    init_once: Mapping[str, Mapping[str, Any]],
    ineligible: Mapping[str, str] | None = None,
) -> list[str]:
    """Read each proven foreign global ``m.f`` (HF-845) from module ``M``'s
    static cell, and consume every ``foreign_global`` record.

    ``closed_world_bindings`` records the read when the receiver is ``M``
    (F3) and ``M`` never rebinds ``f`` after initialization (F1). F3 also puts
    the reading module outside ``M``'s import closure, so ``M``'s body has run
    before the read: when ``M`` keeps ``f`` in an ``InitOnce`` cell, the read
    is one load of that cell (the same stamp an init-dominated read in ``M``
    gets). Any other read keeps its by-name attribute read; the returned
    build diagnostics name why, one per such read."""
    from molt.cli.closed_world_bindings import FOREIGN_GLOBAL_FIELD

    diagnostics: list[str] = []
    for position, func in enumerate(functions):
        ops = func.get("ops", ())
        if not any(FOREIGN_GLOBAL_FIELD in op for op in ops):
            continue
        rewritten: list[dict[str, Any]] = []
        for op in ops:
            module = op.get(FOREIGN_GLOBAL_FIELD)
            if module is None:
                rewritten.append(op)
                continue
            plain = {k: v for k, v in op.items() if k != FOREIGN_GLOBAL_FIELD}
            name = op.get("s_value")
            layout = global_layouts.get(module, ())
            receiver = (op.get("args") or (None,))[0]
            out = op.get("out")
            reason = None
            if module not in static_modules:
                reason = (
                    f"{module} keeps a namespace dictionary: "
                    f"{(ineligible or {}).get(module, 'not cell-eligible')}"
                )
            elif not isinstance(name, str) or name not in layout:
                reason = f"{module} has no cell for {name}"
            elif name not in init_once.get(module, {}):
                reason = (
                    f"{module}'s cell for {name} is not stored once at initialization"
                )
            elif not isinstance(receiver, str) or not isinstance(out, str):
                reason = "the read has no receiver or result"
            if reason is not None:
                diagnostics.append(
                    f"{func.get('name', '')}:{op.get('source_line')} "
                    f"{module}.{name}: by-name read: {reason}"
                )
                rewritten.append(plain)
                continue
            assert isinstance(name, str) and isinstance(out, str)
            name_var = f"foreign_global_name_{out}"
            rewritten.append({"kind": "const_str", "s_value": name, "out": name_var})
            read = {
                key: value
                for key, value in plain.items()
                if key not in {"kind", "args", "s_value"}
            }
            read.update(
                {
                    "kind": "module_get_global",
                    "args": [receiver, name_var],
                    "global_slot": {
                        "module": module,
                        "index": layout.index(name),
                        "cell": True,
                        "init_once": True,
                    },
                }
            )
            rewritten.append(read)
        functions[position] = {**func, "ops": rewritten}
    return diagnostics


def _validate_class_definition_references_with_state(
    integration_state: _FrontendIntegrationState,
) -> str | None:
    """Every class statement a stamp or guard names has exactly one owner."""
    for symbol, module_name in sorted(
        integration_state.class_definition_references.items()
    ):
        if symbol not in integration_state.class_definition_owners:
            return (
                f"Class statement {symbol!r} named by module {module_name} is "
                "defined by no module of the program"
            )
    return None


def _integrate_module_frontend_result_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    *,
    ir_functions: list[dict[str, Any]],
    function_metadata: object,
    func_code_ids: dict[str, int],
    local_classes: dict[ClassId, ClassModel],
    function_digest_seeds: Sequence[FunctionDigestSeed] = (),
) -> str | None:
    class_definitions = _module_class_definitions(local_classes)
    for symbol in sorted(class_definitions):
        owner = integration_state.class_definition_owners.get(symbol)
        if owner is not None and owner != module_name:
            return (
                f"Class statement {symbol!r} is defined by both module {owner} "
                f"and module {module_name}"
            )
    try:
        owned_functions, code_symbols, class_references = _plan_module_code_slots(
            module_name,
            ir_functions,
            set(func_code_ids),
            class_definitions,
        )
        metadata = parse_function_metadata(
            function_metadata, function_symbols=owned_functions.keys()
        )
        for symbol, row in metadata.items():
            if symbol in integration_state.function_metadata:
                raise ValueError(f"Duplicate function metadata owner {symbol!r}")
            if row.get("declaration_only"):
                if symbol not in code_symbols:
                    raise ValueError(
                        f"Missing code slot for declaration-only owner {symbol!r}"
                    )
            else:
                inspect_source_module_publication(owned_functions[symbol], row)
        if not isinstance(function_digest_seeds, Sequence):
            raise ValueError("Invalid module function digest evidence")
        if function_digest_seeds:
            from molt.cli.function_records import FunctionDigestSeed

            body_identities = {id(function) for function in ir_functions}
            seen_seed_owners: set[int] = set()
            for seed in function_digest_seeds:
                if (
                    not isinstance(seed, FunctionDigestSeed)
                    or not isinstance(seed.function, dict)
                    or id(seed.function) not in body_identities
                    or id(seed.function) in seen_seed_owners
                    or not isinstance(seed.digest, bytes)
                    or len(seed.digest) != 32
                ):
                    raise ValueError(
                        "Function digest evidence does not own an admitted module body"
                    )
                seen_seed_owners.add(id(seed.function))
    except ValueError as exc:
        return str(exc)
    # Admission succeeded. Commit only this module's staged additions; copying
    # accumulated program state here would grow quadratically with modules.
    # Publication and trace entry can both name a slot. Its order cannot
    # depend on whether a published callable has a trace entry.
    for symbol in sorted(code_symbols):
        _register_global_code_id_with_state(integration_state, symbol)
    for symbol in sorted(class_definitions):
        integration_state.class_definition_owners.setdefault(symbol, module_name)
    for symbol, owner in class_references.items():
        integration_state.class_definition_references.setdefault(symbol, owner)
    integration_state.function_digest_seeds.extend(function_digest_seeds)
    integration_state.functions.extend(ir_functions)
    integration_state.module_functions[module_name] = ir_functions
    for symbol in sorted(metadata):
        row = metadata[symbol]
        if "constructor_fallbacks" in row:
            # A diagnostic, not an assembly fact: it leaves the metadata here,
            # so no later function pruning can orphan it. The frontend's
            # payload stays unchanged for any cache that holds it.
            integration_state.constructor_fallbacks.extend(row["constructor_fallbacks"])
            row = cast(
                FunctionAssemblyMetadata,
                {k: v for k, v in row.items() if k != "constructor_fallbacks"},
            )
        if row:
            integration_state.function_metadata[symbol] = row
    # ClassIds are program-wide identities: one module's classes never
    # replace another module's same-named classes.
    integration_state.known_classes.update(local_classes)
    return None
