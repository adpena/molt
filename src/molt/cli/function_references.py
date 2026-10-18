from __future__ import annotations

from collections import deque
from typing import Iterable, Mapping, NamedTuple, Sequence

from molt.frontend.module_publication import (
    reject_embedded_function_metadata,
    reject_embedded_operation_metadata,
)
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS,
)

PROTECTED_RUNTIME_ENTRYPOINTS: frozenset[str] = frozenset(
    {"molt_main", "molt_host_init", "_start"}
)
PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES: tuple[str, ...] = ("molt_isolate_",)


class FunctionReferenceEdge(NamedTuple):
    owner: str
    op_index: int
    kind: str
    target: str


def module_symbol_name(module_name: str) -> str:
    sanitized = "".join(ch if ch.isalnum() or ch == "_" else "_" for ch in module_name)
    return sanitized or "module"


def emitted_name_matches_module_symbol(name: str, module_symbol: str) -> bool:
    if name.startswith("molt_init_"):
        return name[len("molt_init_") :] == module_symbol
    return name.startswith(f"{module_symbol}__")


def emitted_name_matches_any_module_symbol(
    name: str, module_symbols: frozenset[str]
) -> bool:
    """`emitted_name_matches_module_symbol` for any of ``module_symbols``.

    A symbol matches a name exactly when the name continues it with ``__``, so
    checking the candidate name prefixes avoids one prefix test per module.
    Prefix slicing and hashing still cost their actual lengths.
    """
    if name.startswith("molt_init_"):
        return name[len("molt_init_") :] in module_symbols
    index = name.find("__")
    while index != -1:
        if name[:index] in module_symbols:
            return True
        index = name.find("__", index + 1)
    return False


def is_protected_runtime_entrypoint(name: str) -> bool:
    return name in PROTECTED_RUNTIME_ENTRYPOINTS or any(
        name.startswith(prefix) for prefix in PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES
    )


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
        if kind not in SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS:
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


def missing_local_function_references(
    module_name: str,
    functions: Sequence[Mapping[str, object]],
    *,
    known_modules: Iterable[str] = (),
) -> tuple[FunctionReferenceEdge, ...]:
    module_symbol = module_symbol_name(module_name)
    descendant_module_symbols = frozenset(
        symbol
        for name in known_modules
        if isinstance(name, str)
        and name
        and (symbol := module_symbol_name(name)).startswith(f"{module_symbol}__")
    )
    defined = {
        name
        for func in functions
        if isinstance((name := func.get("name")), str) and name
    }

    def _owned_locally(target: str) -> bool:
        if not emitted_name_matches_module_symbol(target, module_symbol):
            return False
        return not emitted_name_matches_any_module_symbol(
            target, descendant_module_symbols
        )

    missing: list[FunctionReferenceEdge] = []
    for func in functions:
        reject_embedded_function_metadata(func)
        owner = func.get("name")
        if not isinstance(owner, str):
            continue
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for op_index, op in enumerate(ops):
            if not isinstance(op, Mapping):
                continue
            reject_embedded_operation_metadata(op)
            kind = op.get("kind")
            if not isinstance(kind, str):
                continue
            if kind not in SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS:
                continue
            target = op.get("s_value")
            if not isinstance(target, str):
                continue
            if target not in defined and _owned_locally(target):
                missing.append(FunctionReferenceEdge(owner, op_index, kind, target))
    return tuple(missing)


def format_function_reference_edges(
    edges: Sequence[FunctionReferenceEdge],
    *,
    limit: int = 8,
) -> str:
    preview = [
        f"{edge.owner} op {edge.op_index} {edge.kind} -> {edge.target}"
        for edge in edges[:limit]
    ]
    suffix = ", ..." if len(edges) > limit else ""
    return ", ".join(preview) + suffix
