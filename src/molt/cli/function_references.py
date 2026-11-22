from __future__ import annotations

from collections import deque
from typing import AbstractSet, Iterable, Mapping, Sequence

from molt._intrinsic_symbols import INTRINSIC_SIGNATURES, intrinsic_runtime_symbol_name
from molt._wasm_abi_generated import COMPILED_EXECUTION_ENTRIES
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_CALL_TARGET_ROLES,
    SIMPLEIR_FUNCTION_REFERENCE_KINDS,
    SIMPLEIR_FUNCTION_REFERENCE_S_VALUE_KINDS,
)

# The application-owned program entries hosts call by name.
PROTECTED_RUNTIME_ENTRYPOINTS: frozenset[str] = frozenset(COMPILED_EXECUTION_ENTRIES)

# Direct builtin references and conservative resolver names cannot be shadowed
# by a compiled definition. Ordinary calls and addresses can.
_UNSHADOWED_RUNTIME_REFERENCE_KINDS = frozenset({"builtin_func", "const_str"})
_SHADOWABLE_RUNTIME_REFERENCE_KINDS = (
    frozenset(
        kind
        for kind, role in SIMPLEIR_CALL_TARGET_ROLES.items()
        if role == "external_or_runtime"
    )
    | SIMPLEIR_FUNCTION_REFERENCE_S_VALUE_KINDS
)
_INTRINSIC_SYMBOL_NAMES = frozenset(
    symbol for symbol, _arity in INTRINSIC_SIGNATURES.values()
)


def is_protected_runtime_entrypoint(name: str) -> bool:
    return name in PROTECTED_RUNTIME_ENTRYPOINTS


def function_references(
    func: Mapping[str, object],
    defined: frozenset[str],
    *,
    runtime_symbols: set[str] | None = None,
    compiled: AbstractSet[str] = frozenset(),
) -> set[str]:
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        DESCRIPTOR_PUBLICATION_SYMBOL,
        descriptor_body_symbols,
    )

    refs: set[str] = set()
    ops = func.get("ops")
    if not isinstance(ops, list):
        return refs
    publishes_descriptor = False
    for op in ops:
        if not isinstance(op, Mapping):
            continue
        kind = op.get("kind")
        if not isinstance(kind, str):
            continue
        name = op.get("s_value")
        if kind == "call" and name == DESCRIPTOR_PUBLICATION_SYMBOL:
            publishes_descriptor = True
        if (
            kind in SIMPLEIR_FUNCTION_REFERENCE_KINDS
            and isinstance(name, str)
            and name in defined
        ):
            refs.add(name)
        if runtime_symbols is None:
            continue
        # An explicit runtime callable marker is independent of the spelled
        # target and survives compiled shadowing.
        carried = op.get("runtime_symbol")
        if isinstance(carried, str) and carried in _INTRINSIC_SYMBOL_NAMES:
            runtime_symbols.add(carried)
        if not isinstance(name, str):
            continue
        if kind not in _UNSHADOWED_RUNTIME_REFERENCE_KINDS and (
            kind not in _SHADOWABLE_RUNTIME_REFERENCE_KINDS or name in compiled
        ):
            continue
        canonical = intrinsic_runtime_symbol_name(name)
        if canonical in _INTRINSIC_SYMBOL_NAMES:
            runtime_symbols.add(canonical)
    # A descriptor's body symbols are references too; most functions publish none.
    if publishes_descriptor and defined:
        refs.update(descriptor_body_symbols(ops) & defined)
    return refs


def reachable_function_names(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    return _walk_function_references(functions, extra_roots=extra_roots)


def reached_intrinsic_symbols(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    """Collect runtime references while traversing reached compiled functions."""
    symbols: set[str] = set()
    _walk_function_references(
        functions, extra_roots=extra_roots, runtime_symbols=symbols
    )
    return frozenset(symbols)


def _walk_function_references(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str],
    runtime_symbols: set[str] | None = None,
) -> frozenset[str]:
    if not functions:
        return frozenset()
    by_name: dict[str, Mapping[str, object]] = {}
    compiled: set[str] = set()
    redeclarations: dict[str, list[Mapping[str, object]]] = {}
    for func in functions:
        name = func.get("name")
        if isinstance(name, str):
            if runtime_symbols is not None:
                if name in by_name:
                    redeclarations.setdefault(name, []).append(by_name[name])
                if not func.get("is_extern", False):
                    compiled.add(name)
            by_name[name] = func
    defined = frozenset(by_name)

    reachable: set[str] = set()
    queue: deque[str] = deque()

    def seed(name: str) -> None:
        if name in by_name and name not in reachable:
            reachable.add(name)
            queue.append(name)

    first_name = functions[0].get("name")
    if isinstance(first_name, str):
        seed(first_name)
    for name in by_name:
        if is_protected_runtime_entrypoint(name):
            seed(name)
    for name in extra_roots:
        seed(name)

    while queue:
        current = queue.popleft()
        for target in function_references(
            by_name[current],
            defined,
            runtime_symbols=runtime_symbols,
            compiled=compiled,
        ):
            if target not in reachable:
                reachable.add(target)
                queue.append(target)
        # Repeated declarations do not own graph edges. A retained name still
        # contributes runtime references from every row bearing that name.
        if earlier_rows := redeclarations.get(current):
            for declaration in earlier_rows:
                function_references(
                    declaration,
                    frozenset(),
                    runtime_symbols=runtime_symbols,
                    compiled=compiled,
                )
    return frozenset(reachable)
