from __future__ import annotations

from collections import deque
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass

from molt import compiled_symbols
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


@dataclass(frozen=True, slots=True)
class FunctionReferenceClosure:
    functions: frozenset[str]
    runtime_symbols: frozenset[str]


class FunctionReferenceIndex:
    """Exact reference facts over one finalized program's function bodies."""

    def __init__(self, functions: Sequence[object]) -> None:
        from molt.frontend.lowering.gpu_kernel_descriptor import (
            DESCRIPTOR_PUBLICATION_SYMBOL,
            descriptor_body_symbols,
        )

        defined: set[str] = set()
        compiled: set[str] = set()
        for function in functions:
            if isinstance(function, Mapping) and isinstance(
                name := function.get("name"), str
            ):
                defined.add(name)
                if not function.get("is_extern", False):
                    compiled.add(name)
        self._targets: dict[str, set[str]] = {}
        self._runtime_symbols: dict[str, set[str]] = {}
        missing: dict[str, str] = {}
        for function in functions:
            if not isinstance(function, Mapping):
                continue
            name = function.get("name")
            ops = function.get("ops")
            targets: set[str] = set()
            symbols: set[str] = set()
            publishes_descriptor = False
            if isinstance(ops, list):
                for index, op in enumerate(ops):
                    if not isinstance(op, dict) and not isinstance(op, Mapping):
                        continue
                    kind = op.get("kind")
                    if not isinstance(kind, str):
                        continue
                    # Explicit runtime markers survive compiled shadowing.
                    carried = op.get("runtime_symbol")
                    if isinstance(carried, str) and carried in _INTRINSIC_SYMBOL_NAMES:
                        symbols.add(carried)
                    target = op.get("s_value")
                    if not isinstance(target, str):
                        continue
                    if kind == "call":
                        if target == DESCRIPTOR_PUBLICATION_SYMBOL:
                            publishes_descriptor = True
                        if (
                            target not in defined
                            and target not in missing
                            and compiled_symbols.is_compiled_symbol(target)
                        ):
                            missing[target] = f"{target} at {name}[{index}]"
                    if kind in SIMPLEIR_FUNCTION_REFERENCE_KINDS and target in defined:
                        targets.add(target)
                    if kind in _UNSHADOWED_RUNTIME_REFERENCE_KINDS or (
                        kind in _SHADOWABLE_RUNTIME_REFERENCE_KINDS
                        and target not in compiled
                    ):
                        canonical = intrinsic_runtime_symbol_name(target)
                        if canonical in _INTRINSIC_SYMBOL_NAMES:
                            symbols.add(canonical)
                if publishes_descriptor and defined:
                    targets.update(descriptor_body_symbols(ops) & defined)
            if isinstance(name, str):
                # The last row owns graph edges. A reached name also consumes
                # runtime references carried by its preceding declarations.
                self._targets[name] = targets
                if symbols:
                    self._runtime_symbols.setdefault(name, set()).update(symbols)
        first = functions[0] if functions else None
        first_name = first.get("name") if isinstance(first, Mapping) else None
        self._roots = frozenset(
            name
            for name in defined
            if name == first_name or is_protected_runtime_entrypoint(name)
        )
        self._missing_compiled_calls = tuple(missing.values())

    @classmethod
    def from_ir(cls, ir: Mapping[str, object]) -> FunctionReferenceIndex:
        functions = ir.get("functions")
        return cls(functions if isinstance(functions, list) else ())

    def compiled_call_closure_issue(self) -> str | None:
        missing = self._missing_compiled_calls
        if not missing:
            return None
        suffix = "" if len(missing) <= 8 else ", ..."
        return (
            "backend IR contains direct calls to compiled functions with no body "
            f"in the program: {', '.join(missing[:8])}{suffix}"
        )

    def closure(self, *, extra_roots: Iterable[str] = ()) -> FunctionReferenceClosure:
        reached = set(self._roots)
        reached.update(name for name in extra_roots if name in self._targets)
        queue = deque(reached)
        symbols: set[str] = set()
        while queue:
            current = queue.popleft()
            runtime = self._runtime_symbols.get(current)
            if runtime:
                symbols.update(runtime)
            for target in self._targets[current]:
                if target not in reached:
                    reached.add(target)
                    queue.append(target)
        return FunctionReferenceClosure(frozenset(reached), frozenset(symbols))


def reachable_function_names(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    return FunctionReferenceIndex(functions).closure(extra_roots=extra_roots).functions


def reached_intrinsic_symbols(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    return (
        FunctionReferenceIndex(functions)
        .closure(extra_roots=extra_roots)
        .runtime_symbols
    )
