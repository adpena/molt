from __future__ import annotations

from collections import deque
from typing import Iterable, Mapping, Sequence

from molt._wasm_abi_generated import COMPILED_EXECUTION_ENTRIES
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_FUNCTION_REFERENCE_KINDS,
)

# The application-owned program entries hosts call by name.
PROTECTED_RUNTIME_ENTRYPOINTS: frozenset[str] = frozenset(COMPILED_EXECUTION_ENTRIES)


def is_protected_runtime_entrypoint(name: str) -> bool:
    return name in PROTECTED_RUNTIME_ENTRYPOINTS


def function_references(
    func: Mapping[str, object],
    defined: frozenset[str],
) -> frozenset[str]:
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        DESCRIPTOR_PUBLICATION_SYMBOL,
        descriptor_body_symbols,
    )

    refs: set[str] = set()
    ops = func.get("ops")
    if not isinstance(ops, list):
        return frozenset()
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
        if kind not in SIMPLEIR_FUNCTION_REFERENCE_KINDS:
            continue
        if isinstance(name, str) and name in defined:
            refs.add(name)
    # A descriptor's body symbols are references too; most functions publish none.
    if publishes_descriptor:
        refs.update(descriptor_body_symbols(ops) & defined)
    return frozenset(refs)


def reachable_function_names(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    if not functions:
        return frozenset()
    by_name: dict[str, Mapping[str, object]] = {}
    for func in functions:
        name = func.get("name")
        if isinstance(name, str):
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
        for target in function_references(by_name[current], defined):
            if target not in reachable:
                reachable.add(target)
                queue.append(target)
    return frozenset(reachable)
