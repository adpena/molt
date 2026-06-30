from __future__ import annotations

from collections import deque
from typing import Iterable, Mapping, NamedTuple, Sequence


FUNCTION_REFERENCE_OP_KINDS: frozenset[str] = frozenset(
    {
        "call",
        "call_internal",
        "func_new",
        "func_new_closure",
        "func_new_builtin",
        "code_new",
        "call_guarded",
        "call_indirect",
        "alloc_task",
        "generator_create",
        "coro_create",
        "fn_ptr_code_set",
        "asyncgen_locals_register",
        "gen_locals_register",
        "task_new",
        "generator_send",
        "spawn",
        "call_func",
        "call_method",
        "import_from",
        "import_name",
        "class_def",
        "decorator",
        "super_call",
        "yield_from",
        "await",
    }
)

POLL_COMPANION_OP_KINDS: frozenset[str] = frozenset(
    {"alloc_task", "generator_create", "coro_create"}
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


def is_protected_runtime_entrypoint(name: str) -> bool:
    return name in PROTECTED_RUNTIME_ENTRYPOINTS or any(
        name.startswith(prefix) for prefix in PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES
    )


def function_references(
    func: Mapping[str, object],
    defined: frozenset[str],
) -> frozenset[str]:
    refs: set[str] = set()
    ops = func.get("ops")
    if not isinstance(ops, list):
        return frozenset()
    for op in ops:
        if not isinstance(op, Mapping):
            continue
        kind = op.get("kind")
        if not isinstance(kind, str):
            continue
        if kind not in FUNCTION_REFERENCE_OP_KINDS:
            continue
        name = op.get("s_value")
        if not isinstance(name, str):
            continue
        if name in defined:
            refs.add(name)
        if kind in POLL_COMPANION_OP_KINDS and not name.endswith("_poll"):
            poll = f"{name}_poll"
            if poll in defined:
                refs.add(poll)
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
) -> tuple[FunctionReferenceEdge, ...]:
    module_symbol = module_symbol_name(module_name)
    defined = {
        name
        for func in functions
        if isinstance((name := func.get("name")), str) and name
    }
    missing: list[FunctionReferenceEdge] = []
    for func in functions:
        owner = func.get("name")
        if not isinstance(owner, str):
            continue
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for op_index, op in enumerate(ops):
            if not isinstance(op, Mapping):
                continue
            kind = op.get("kind")
            if not isinstance(kind, str):
                continue
            if kind not in FUNCTION_REFERENCE_OP_KINDS:
                continue
            target = op.get("s_value")
            if not isinstance(target, str):
                continue
            if target not in defined and emitted_name_matches_module_symbol(
                target, module_symbol
            ):
                missing.append(FunctionReferenceEdge(owner, op_index, kind, target))
            if kind in POLL_COMPANION_OP_KINDS and not target.endswith("_poll"):
                poll_target = f"{target}_poll"
                if (
                    poll_target not in defined
                    and emitted_name_matches_module_symbol(poll_target, module_symbol)
                ):
                    missing.append(
                        FunctionReferenceEdge(owner, op_index, kind, poll_target)
                    )
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
