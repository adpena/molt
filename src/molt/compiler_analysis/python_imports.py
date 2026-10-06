"""Canonical Python import execution-state and request semantics."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.compiler_analysis.python_binding_facts import (
        PythonCallSiteFact,
        PythonExpressionFact,
        PythonIterationFact,
        PythonNodeKey,
        PythonModuleMetadataProof,
        PythonStatementFact,
    )

import ast
from molt.python_private_names import python_import_binding
from collections import deque
from collections.abc import Callable, Collection, Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from types import MappingProxyType
from typing import Literal

from molt.compiler_analysis.python_call_arguments import call_argument_schedule
from molt.compiler_analysis.python_effects import (
    expression_evaluation_children,
)
from molt.compiler_analysis.python_source_keys import (
    _PythonAstDigestAdmission,
    PythonSourceKey,
    python_node_source_key,
    python_pattern_capture_names,
    python_pattern_irrefutable_reason,
    python_pattern_is_capture_only,
)


from molt.compiler_analysis.static_truth import (
    ExpressionResultLookup,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    static_expression_result,
)


StaticValueKind = Literal["known", "none", "absent", "invalid", "unknown"]
ImportOperationKind = Literal["statement", "import_module", "dunder_import"]
ModuleExecutionKind = Literal["imported", "module", "script"]
ImportNodeKey = PythonSourceKey
ImportResolutionError = Literal[
    "no_parent",
    "beyond_top",
    "empty_name",
    "negative_level",
    "invalid_level",
    "invalid_package",
    "unknown_package",
    "invalid_spec",
    "unknown_spec",
    "missing_name",
    "invalid_name",
    "unknown_name",
    "missing_globals",
]


@dataclass(frozen=True, slots=True)
class StaticMetadataValue:
    kind: StaticValueKind
    value: str | None = None

    @classmethod
    def known(cls, value: str) -> StaticMetadataValue:
        return cls("known", value)


NONE_VALUE = StaticMetadataValue("none")
ABSENT_VALUE = StaticMetadataValue("absent")
INVALID_VALUE = StaticMetadataValue("invalid")
UNKNOWN_VALUE = StaticMetadataValue("unknown")
_IMPORT_METADATA_NAMES = frozenset({"__package__", "__spec__", "__name__", "__path__"})
_MAX_DISJUNCTIVE_IMPORT_STATES = 64


@dataclass(frozen=True, slots=True)
class ModuleImportState:
    """Actual runtime metadata visible to import semantics at one program point."""

    package: StaticMetadataValue
    spec_parent: StaticMetadataValue
    name: StaticMetadataValue
    has_path: bool | None


@dataclass(frozen=True, slots=True)
class ModuleImportContext:
    module_name: str | None
    is_package: bool
    state: ModuleImportState | None = None
    spec_name: str | None = None
    target_python: tuple[int, int] = (3, 12)
    execution_kind: ModuleExecutionKind = "imported"

    def with_state(self, state: ModuleImportState) -> ModuleImportContext:
        return ModuleImportContext(
            self.module_name,
            self.is_package,
            state,
            self.spec_name,
            self.target_python,
            self.execution_kind,
        )


@dataclass(frozen=True, slots=True)
class RelativeImportResolution:
    module: str | None
    error: ImportResolutionError | None = None
    requires_runtime: bool = False


@dataclass(frozen=True, slots=True)
class StaticImportRequest:
    """One import operation without collapsing its distinct runtime contract."""

    kind: ImportOperationKind
    name: str
    level: int = 0
    fromlist: tuple[str, ...] = ()
    package_argument: StaticMetadataValue | None = None
    globals_state: ModuleImportState | None = None
    globals_were_supplied: bool = False
    level_is_invalid: bool = False

    @classmethod
    def statement(
        cls,
        name: str,
        *,
        level: int = 0,
        fromlist: Sequence[str] = (),
    ) -> StaticImportRequest:
        return cls("statement", name, level, tuple(fromlist))

    @classmethod
    def import_module(
        cls,
        name: str,
        package_argument: StaticMetadataValue | None = None,
    ) -> StaticImportRequest:
        return cls(
            "import_module",
            name,
            package_argument=package_argument,
        )


@dataclass(frozen=True, slots=True)
class StaticImportCallArguments:
    name: ast.expr | None
    package: ast.expr | None = None
    globals: ast.expr | None = None
    locals: ast.expr | None = None
    fromlist: ast.expr | None = None
    level: ast.expr | None = None
    requires_runtime_binding: bool = False


def bind_static_import_call_arguments(
    call: ast.Call,
    kind: ImportOperationKind,
) -> StaticImportCallArguments | None:
    """Bind a static request, or return None when Python binding must fail.

    A missing, excess, duplicate, or unexpected argument cannot execute the
    import operation. Leave that call and its argument evaluation to runtime,
    where its TypeError is observable and catchable. Unknown expansion is a
    partial binding that requires runtime import custody, not proof of an
    invalid call. Explicit operands keep their values on every successful
    binding; an expansion cannot replace one without a duplicate error.
    """

    parameter_names = (
        ("name", "package")
        if kind == "import_module"
        else ("name", "globals", "locals", "fromlist", "level")
    )
    if kind == "statement":
        raise ValueError("import statements do not have call arguments")
    positional_minimum = sum(
        not isinstance(argument, ast.Starred) for argument in call.args
    )
    if positional_minimum > len(parameter_names):
        return None
    bound: dict[str, ast.expr] = {}
    requires_runtime_binding = False
    for argument in call.args:
        if isinstance(argument, ast.Starred):
            requires_runtime_binding = True
        elif not requires_runtime_binding:
            bound[parameter_names[len(bound)]] = argument
    for keyword in call.keywords:
        if keyword.arg is None:
            requires_runtime_binding = True
            continue
        if (
            keyword.arg not in parameter_names
            or keyword.arg in bound
            or parameter_names.index(keyword.arg) < positional_minimum
        ):
            return None
        bound[keyword.arg] = keyword.value
    name = bound.get("name")
    if name is None and not requires_runtime_binding:
        return None
    return StaticImportCallArguments(
        name=name,
        package=bound.get("package"),
        globals=bound.get("globals"),
        locals=bound.get("locals"),
        fromlist=bound.get("fromlist"),
        level=bound.get("level"),
        requires_runtime_binding=requires_runtime_binding,
    )


@dataclass(frozen=True, slots=True)
class StaticImportProjection:
    modules: tuple[str, ...]
    error: ImportResolutionError | None = None
    requires_runtime: bool = False
    requires_runtime_execution: bool = False


@dataclass(frozen=True, slots=True)
class StaticImportPlan:
    modules: tuple[str, ...]
    errors: tuple[ImportResolutionError, ...]
    requires_runtime: bool
    requires_runtime_execution: bool


class UnresolvedStaticImportError(ValueError):
    """A dependency cannot be sealed without explicit runtime import custody."""


@dataclass(frozen=True, slots=True)
class ModuleImportFlow:
    """Execution metadata and opt-in source candidates at import/call sites."""

    states_by_node: Mapping[ImportNodeKey, tuple[ModuleImportState, ...]]
    final_states: tuple[ModuleImportState, ...]
    all_states: tuple[ModuleImportState, ...]
    source_states_by_node: (
        Mapping[ImportNodeKey, tuple[ModuleImportState, ...]] | None
    ) = None
    source_all_states: tuple[ModuleImportState, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "states_by_node",
            MappingProxyType(
                {key: tuple(states) for key, states in self.states_by_node.items()}
            ),
        )
        object.__setattr__(self, "final_states", tuple(self.final_states))
        object.__setattr__(self, "all_states", tuple(self.all_states))
        if self.source_states_by_node is not None:
            object.__setattr__(
                self,
                "source_states_by_node",
                MappingProxyType(
                    {
                        key: tuple(states)
                        for key, states in self.source_states_by_node.items()
                    }
                ),
            )
        object.__setattr__(self, "source_all_states", tuple(self.source_all_states))

    def states_for(self, node: ast.AST) -> tuple[ModuleImportState, ...]:
        # An unrecorded execution phase (deferred annotations, lambda/generator
        # bodies, or a newly introduced AST form) must never inherit the final
        # module snapshot. Unioning the observed source-order states is the
        # conservative runtime anchor until that phase has an explicit event.
        return self.states_by_node.get(python_node_source_key(node), self.all_states)

    def source_states_for(self, node: ast.AST) -> tuple[ModuleImportState, ...]:
        """Source candidates are never execution metadata or storage custody."""
        if self.source_states_by_node is None:
            raise ValueError("source import discovery projection was not requested")
        return self.source_states_by_node.get(
            python_node_source_key(node), self.source_all_states
        )


def module_spec_parent(spec_name: str, is_package: bool) -> str:
    return spec_name if is_package else spec_name.rpartition(".")[0]


def loader_module_import_state(context: ModuleImportContext) -> ModuleImportState:
    if context.execution_kind == "script":
        return ModuleImportState(
            package=NONE_VALUE,
            spec_parent=NONE_VALUE,
            name=StaticMetadataValue.known("__main__"),
            has_path=False,
        )
    name = context.spec_name or context.module_name or ""
    parent = module_spec_parent(name, context.is_package)
    return ModuleImportState(
        package=StaticMetadataValue.known(parent),
        spec_parent=StaticMetadataValue.known(parent),
        name=StaticMetadataValue.known(context.module_name or name),
        has_path=context.is_package,
    )


def context_import_state(context: ModuleImportContext) -> ModuleImportState:
    return context.state or loader_module_import_state(context)


def parse_module_spec_parent(
    value: ast.AST,
    call_fact: PythonCallSiteFact | None = None,
) -> StaticMetadataValue:
    """Evaluate a statically known, CPython-valid ModuleSpec parent."""

    from molt.compiler_analysis.python_binding_facts import PythonNodeKey
    from molt.compiler_analysis.python_value_identity import (
        PythonIdentity,
        identity_fact_is_exact,
    )
    from molt.compiler_analysis.python_effects_generated import (
        NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    )

    if isinstance(value, ast.Constant) and value.value is None:
        return NONE_VALUE
    if (
        not isinstance(value, ast.Call)
        or call_fact is None
        or call_fact.node != PythonNodeKey.from_node(value)
        or not call_fact.callee_is(PythonIdentity.MODULE_SPEC_CLASS)
        or not identity_fact_is_exact(
            call_fact.result_identities, PythonIdentity.MODULE_SPEC_INSTANCE
        )
        or call_fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    ):
        return INVALID_VALUE if isinstance(value, ast.Constant) else UNKNOWN_VALUE
    # ModuleSpec(name, loader, *, origin=None, loader_state=None, is_package=None)
    if len(value.args) > 2 or any(keyword.arg is None for keyword in value.keywords):
        return INVALID_VALUE
    allowed_keywords = {"name", "loader", "origin", "loader_state", "is_package"}
    keyword_ids = [keyword.arg for keyword in value.keywords]
    if any(keyword not in allowed_keywords for keyword in keyword_ids) or len(
        keyword_ids
    ) != len(set(keyword_ids)):
        return INVALID_VALUE
    positional_name = value.args[0] if value.args else None
    keyword_names = [
        keyword.value for keyword in value.keywords if keyword.arg == "name"
    ]
    if positional_name is not None and keyword_names or len(keyword_names) > 1:
        return INVALID_VALUE
    name_node = positional_name or (keyword_names[0] if keyword_names else None)
    positional_loader = value.args[1] if len(value.args) > 1 else None
    keyword_loaders = [
        keyword.value for keyword in value.keywords if keyword.arg == "loader"
    ]
    if positional_loader is not None and keyword_loaders or len(keyword_loaders) > 1:
        return INVALID_VALUE
    if positional_loader is None and not keyword_loaders:
        return INVALID_VALUE
    if not (isinstance(name_node, ast.Constant) and isinstance(name_node.value, str)):
        return UNKNOWN_VALUE if name_node is not None else INVALID_VALUE
    is_package: bool | None = None
    package_keywords = [
        keyword.value for keyword in value.keywords if keyword.arg == "is_package"
    ]
    if len(package_keywords) > 1:
        return INVALID_VALUE
    if package_keywords:
        package_node = package_keywords[0]
        if isinstance(package_node, ast.Constant) and (
            isinstance(package_node.value, bool) or package_node.value is None
        ):
            is_package = package_node.value
        else:
            return UNKNOWN_VALUE
    return StaticMetadataValue.known(
        module_spec_parent(name_node.value, is_package is True)
    )


def metadata_value_from_result(result: StaticExpressionResult) -> StaticMetadataValue:
    """Retain invalid and None metadata alongside exact string values."""
    if result.kind == "NoneType":
        return NONE_VALUE
    if result.kind == "str":
        return (
            StaticMetadataValue.known(result.value)
            if result.value_known and isinstance(result.value, str)
            else UNKNOWN_VALUE
        )
    return INVALID_VALUE if result.kind != "unknown" else UNKNOWN_VALUE


def static_import_fromlist_is_empty(result: StaticExpressionResult) -> bool:
    """Prove falsy scalar operands without borrowing mutable container contents."""
    return result.value_known and result.truth is False


def static_import_level_from_result(
    result: StaticExpressionResult,
) -> tuple[int | None, bool]:
    """Separate exact index values, known type errors and callback-dependent levels."""
    if result.value_known:
        value = result.value
        if type(value) is int:
            return value, False
        if type(value) is bool:
            return int(value), False
    invalid = result.kind in {
        "NoneType",
        "float",
        "complex",
        "str",
        "bytes",
        "bytearray",
        "tuple",
        "list",
        "set",
        "frozenset",
        "dict",
        "range",
    }
    return None, invalid


def _metadata_value(
    value: ast.AST, fact_result: ExpressionResultLookup | None = None
) -> StaticMetadataValue:
    if not isinstance(value, ast.expr):
        return UNKNOWN_VALUE
    result = (
        fact_result(value) or UNKNOWN_EXPRESSION_RESULT
        if fact_result is not None
        else static_expression_result(value)
    )
    return metadata_value_from_result(result)


def _globals_subscript_name(target: ast.AST) -> str | None:
    if not (
        isinstance(target, ast.Subscript)
        and isinstance(target.value, ast.Call)
        and isinstance(target.value.func, ast.Name)
        and target.value.func.id in {"globals", "locals", "vars"}
        and not target.value.args
        and not target.value.keywords
        and isinstance(target.slice, ast.Constant)
        and isinstance(target.slice.value, str)
    ):
        return None
    return target.slice.value


def import_metadata_target_name(target: ast.AST) -> str | None:
    if isinstance(target, ast.Name):
        return target.id
    if (
        isinstance(target, ast.Attribute)
        and isinstance(target.value, ast.Name)
        and target.value.id == "__spec__"
        and target.attr in {"name", "submodule_search_locations"}
    ):
        return "__spec__"
    if (
        isinstance(target, ast.Attribute)
        and target.attr in _IMPORT_METADATA_NAMES
        and isinstance(target.value, ast.Subscript)
        and isinstance(target.value.value, ast.Attribute)
        and isinstance(target.value.value.value, ast.Name)
        and target.value.value.value.id == "sys"
        and target.value.value.attr == "modules"
        and isinstance(target.value.slice, ast.Name)
        and target.value.slice.id == "__name__"
    ):
        return target.attr
    if (
        isinstance(target, ast.Subscript)
        and isinstance(target.value, ast.Attribute)
        and target.value.attr == "__globals__"
        and isinstance(target.slice, ast.Constant)
        and isinstance(target.slice.value, str)
    ):
        return target.slice.value
    return _globals_subscript_name(target)


def _unknown_module_import_state(state: ModuleImportState) -> ModuleImportState:
    return ModuleImportState(UNKNOWN_VALUE, UNKNOWN_VALUE, UNKNOWN_VALUE, None)


def module_import_context_with_metadata_proof(
    context: ModuleImportContext, proof: PythonModuleMetadataProof | None
) -> ModuleImportContext:
    """Admit the import-state projection only under binding storage custody."""
    if proof is not None and proof.admits_current_namespace:
        return context
    return context.with_state(
        _unknown_module_import_state(context_import_state(context))
    )


def metadata_value_from_expression(
    expression: ast.expr | None,
    context: ModuleImportContext,
    resolve_string: Callable[[ast.expr], str | None] | None = None,
    *,
    fact_result: ExpressionResultLookup | None = None,
    expression_fact: Callable[[ast.expr], PythonExpressionFact | None] | None = None,
    allow_activation_metadata_for_discovery: bool = False,
    call_fact: PythonCallSiteFact | None = None,
) -> StaticMetadataValue | None:
    """Evaluate metadata expressions without executing user code."""

    if expression is None:
        return None
    metadata = _metadata_value(expression, fact_result)
    if metadata.kind != "unknown":
        return metadata
    if isinstance(expression, ast.Name):
        fact = expression_fact(expression) if expression_fact is not None else None
        if (
            fact is not None
            and fact.name_lookup == "global"
            and expression.id in {"__package__", "__name__"}
        ):
            # Lexical candidates are not execution-state authority. A strict
            # borrow needs clean storage at the read and after argument
            # evaluation. Unknown bound values cannot borrow loader defaults;
            # the slot must remain loader-pristine through invocation; deletion
            # is a stored tombstone, not a loader default. Known scalar
            # values above retain their already-evaluated value independently.
            if not allow_activation_metadata_for_discovery and not (
                fact.module_metadata.admits_loader_borrow(expression.id)
                and not fact.binding_invalidated
                and not fact.binding_is_bound
                and call_fact is not None
                and call_fact.module_metadata_at_invocation.admits_loader_borrow(
                    expression.id
                )
            ):
                return UNKNOWN_VALUE
            state = context_import_state(context)
            value = state.package if expression.id == "__package__" else state.name
            # A name read captures an object, never a missing mapping member.
            # Absence can raise or consult builtins; it cannot make an explicit
            # dictionary entry disappear and trigger __name__ fallback.
            return UNKNOWN_VALUE if value.kind == "absent" else value
    resolved = resolve_string(expression) if resolve_string is not None else None
    return (
        StaticMetadataValue.known(resolved) if resolved is not None else UNKNOWN_VALUE
    )


def _dunder_globals_states_from_expression(
    expression: ast.expr | None,
    context: ModuleImportContext,
    resolve_string: Callable[[ast.expr], str | None] | None = None,
    *,
    fact_result: ExpressionResultLookup | None = None,
    expression_fact: Callable[[ast.expr], PythonExpressionFact | None] | None = None,
    allow_possible_current_globals: bool = False,
    call_fact: PythonCallSiteFact | None = None,
    captured_values: Callable[[ast.expr], Sequence[StaticMetadataValue | None]]
    | None = None,
) -> tuple[ModuleImportState, ...] | None:
    """One dictionary transfer for strict values and captured source alternatives."""
    if expression is None:
        return None
    if expression_fact is not None:
        from molt.compiler_analysis.python_binding_facts import (
            current_globals_dict_is_exact,
        )
        from molt.compiler_analysis.python_value_identity import PythonIdentity

        fact = expression_fact(expression)
        if fact is not None and (
            current_globals_dict_is_exact(fact.identities, fact.result)
            or allow_possible_current_globals
            and fact.identities & int(PythonIdentity.CURRENT_GLOBALS)
        ):
            # Capturing a mapping reference does not snapshot its contents.
            # Both views read an actual current-globals mapping at invocation.
            state = context_import_state(context)
            if allow_possible_current_globals:
                if captured_values is not None and not current_globals_dict_is_exact(
                    fact.identities, fact.result
                ):
                    # A possible mapping identity supplies a candidate, not an
                    # exhaustive operand value. The other branch stays unknown.
                    return _merge_states((state, _unknown_module_import_state(state)))
                return (state,)
            return (
                context_import_state(
                    module_import_context_with_metadata_proof(
                        context,
                        call_fact.module_metadata_at_invocation
                        if call_fact is not None
                        else None,
                    )
                ),
            )
    if not isinstance(expression, ast.Dict):
        return None
    states: tuple[ModuleImportState, ...] = (
        ModuleImportState(ABSENT_VALUE, ABSENT_VALUE, ABSENT_VALUE, False),
    )
    for key, value in zip(expression.keys, expression.values):
        if key is None:
            unpacked = (
                _dunder_globals_states_from_expression(
                    value,
                    context,
                    resolve_string,
                    fact_result=fact_result,
                    expression_fact=expression_fact,
                    allow_possible_current_globals=allow_possible_current_globals,
                    call_fact=call_fact,
                    captured_values=captured_values,
                )
                if isinstance(value, ast.Dict)
                else None
            )
            if unpacked is None:
                states = _merge_states(
                    _unknown_module_import_state(state) for state in states
                )
            else:
                states = _merge_states(
                    ModuleImportState(
                        overlay.package
                        if overlay.package.kind != "absent"
                        else state.package,
                        overlay.spec_parent
                        if overlay.spec_parent.kind != "absent"
                        else state.spec_parent,
                        overlay.name if overlay.name.kind != "absent" else state.name,
                        overlay.has_path or state.has_path,
                    )
                    for state in states
                    for overlay in unpacked
                )
            continue
        key_value = _metadata_value(key, fact_result)
        if key_value.kind == "unknown":
            # An unknown key can equal any metadata member. Retain prior source
            # candidates, but never certify that this dictionary preserved them.
            unknown = tuple(_unknown_module_import_state(state) for state in states)
            states = _merge_states(
                states if captured_values is not None else (), unknown
            )
            continue
        if key_value.kind != "known":
            # Canonical exact non-string results cannot select metadata keys.
            continue
        if key_value.value in {"__package__", "__name__"}:
            values = (
                captured_values(value)
                if captured_values is not None
                else (
                    metadata_value_from_expression(
                        value,
                        context,
                        resolve_string,
                        fact_result=fact_result,
                        expression_fact=expression_fact,
                        allow_activation_metadata_for_discovery=(
                            allow_possible_current_globals
                        ),
                        call_fact=call_fact,
                    ),
                )
            )
            field = "package" if key_value.value == "__package__" else "name"
            states = _merge_states(
                replace(state, **{field: metadata or UNKNOWN_VALUE})
                for state in states
                for metadata in values
            )
        elif key_value.value == "__spec__":
            states = _merge_states(
                replace(state, spec_parent=parse_module_spec_parent(value))
                for state in states
            )
        elif key_value.value == "__path__":
            states = _merge_states(replace(state, has_path=True) for state in states)
    return states


def dunder_globals_state_from_expression(
    expression: ast.expr | None,
    context: ModuleImportContext,
    resolve_string: Callable[[ast.expr], str | None] | None = None,
    *,
    fact_result: ExpressionResultLookup | None = None,
    expression_fact: Callable[[ast.expr], PythonExpressionFact | None] | None = None,
    allow_possible_current_globals: bool = False,
    call_fact: PythonCallSiteFact | None = None,
) -> ModuleImportState | None:
    """Project one strict or lexical request through the shared dictionary transfer."""
    states = _dunder_globals_states_from_expression(
        expression,
        context,
        resolve_string,
        fact_result=fact_result,
        expression_fact=expression_fact,
        allow_possible_current_globals=allow_possible_current_globals,
        call_fact=call_fact,
    )
    if states is None:
        return None
    # Without captured source alternatives each field has exactly one value.
    assert len(states) == 1
    return states[0]


def source_import_requests_from_expressions(
    request: StaticImportRequest,
    context: ModuleImportContext,
    *,
    package_expression: ast.expr | None = None,
    globals_expression: ast.expr | None = None,
    source_contexts_for_read: Callable[[ast.expr], Sequence[ModuleImportContext]],
    resolve_string: Callable[[ast.expr], str | None] | None = None,
    fact_result: ExpressionResultLookup | None = None,
    expression_fact: Callable[[ast.expr], PythonExpressionFact | None] | None = None,
    call_fact: PythonCallSiteFact | None = None,
) -> tuple[StaticImportRequest, ...]:
    """Discover captured operands without re-reading them at invocation.

    Explicit dictionary values and import_module package scalars keep their
    evaluation-point metadata. Actual globals mappings retain invocation-time
    contents. Neither path supplies execution storage or catalog custody.
    """

    def captured_values(
        expression: ast.expr | None,
    ) -> tuple[StaticMetadataValue | None, ...]:
        read_contexts = (
            source_contexts_for_read(expression)
            if isinstance(expression, ast.Name)
            and expression.id in {"__package__", "__name__"}
            else (context,)
        )
        return tuple(
            dict.fromkeys(
                metadata_value_from_expression(
                    expression,
                    read_context,
                    resolve_string,
                    fact_result=fact_result,
                    expression_fact=expression_fact,
                    allow_activation_metadata_for_discovery=True,
                    call_fact=call_fact,
                )
                for read_context in read_contexts
            )
        )

    if request.kind == "import_module":
        return tuple(
            replace(request, package_argument=value)
            for value in captured_values(package_expression)
        )
    if request.kind != "dunder_import":
        raise ValueError("captured metadata projection requires an import call")
    states = _dunder_globals_states_from_expression(
        globals_expression,
        context,
        resolve_string,
        fact_result=fact_result,
        expression_fact=expression_fact,
        allow_possible_current_globals=True,
        call_fact=call_fact,
        captured_values=captured_values,
    )
    return tuple(
        replace(request, globals_state=state)
        for state in (states if states is not None else (None,))
    )


def update_module_import_state(
    state: ModuleImportState,
    target: ast.AST,
    value: ast.AST,
    call_fact: PythonCallSiteFact | None = None,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> ModuleImportState:
    target_name = import_metadata_target_name(target)
    if target_name is None:
        return state
    if target_name == "__package__":
        return replace(state, package=_metadata_value(value, fact_result))
    if target_name == "__spec__":
        return replace(
            state,
            spec_parent=parse_module_spec_parent(value, call_fact),
        )
    if target_name == "__name__":
        return replace(state, name=_metadata_value(value, fact_result))
    if target_name == "__path__":
        return replace(state, has_path=True)
    return state


def invalidate_module_import_state(
    state: ModuleImportState,
    target: ast.AST,
    *,
    deleted: bool = False,
) -> ModuleImportState:
    target_name = import_metadata_target_name(target)
    if target_name is None:
        return state
    unknown = ABSENT_VALUE if deleted else UNKNOWN_VALUE
    if target_name == "__package__":
        return replace(state, package=unknown)
    if target_name == "__spec__":
        return replace(state, spec_parent=unknown)
    if target_name == "__name__":
        return replace(state, name=unknown)
    if target_name == "__path__":
        return replace(state, has_path=False if deleted else None)
    return state


def _state_sort_key(state: ModuleImportState) -> tuple[str, ...]:
    return (
        state.package.kind,
        state.package.value or "",
        state.spec_parent.kind,
        state.spec_parent.value or "",
        state.name.kind,
        state.name.value or "",
        str(state.has_path),
    )


def _merge_states(
    *groups: Iterable[ModuleImportState],
) -> tuple[ModuleImportState, ...]:
    states = {state for group in groups for state in group}
    if len(states) <= _MAX_DISJUNCTIVE_IMPORT_STATES:
        return tuple(sorted(states, key=_state_sort_key))

    def join_value(attribute: str) -> StaticMetadataValue:
        values = {getattr(state, attribute) for state in states}
        return next(iter(values)) if len(values) == 1 else UNKNOWN_VALUE

    path_values = {state.has_path for state in states}
    return (
        ModuleImportState(
            join_value("package"),
            join_value("spec_parent"),
            join_value("name"),
            next(iter(path_values)) if len(path_values) == 1 else None,
        ),
    )


def _normalized_import_context(context: ModuleImportContext) -> ModuleImportContext:
    if context.spec_name is not None or context.state is not None:
        return context
    return replace(context, spec_name=context.module_name)


def _analyze_module_import_flow_uncached(
    tree: ast.AST,
    context: ModuleImportContext,
    *,
    statement_facts: Mapping[PythonNodeKey, PythonStatementFact],
    iteration_facts: Mapping[PythonNodeKey, PythonIterationFact],
    expression_facts: Mapping[PythonNodeKey, PythonExpressionFact],
    assignment_effects: Mapping[PythonNodeKey, int],
    call_facts: Mapping[PythonNodeKey, PythonCallSiteFact],
    source_discovery: bool = False,
) -> ModuleImportFlow:
    """Project one requested metadata view through canonical completion facts.

    Source discovery retains explicit candidates beside unknown callback
    alternatives. Unknown source writes and completion partitions still apply.
    Semantic projection keeps every callback invalidation unchanged.
    """

    from molt.compiler_analysis.python_binding_facts import (
        PythonCompletion,
        PythonCompletionFlow,
        PythonNodeKey,
        globals_mutation_call_identity,
    )
    from molt.compiler_analysis.python_value_identity import PythonIdentity
    from molt.compiler_analysis.python_effects_generated import (
        NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
        RAISES,
        UNKNOWN_EFFECTS,
        WRITES_MODULE_METADATA,
    )

    initial = (context_import_state(context),)
    by_node: dict[ImportNodeKey, tuple[ModuleImportState, ...]] = {}
    all_states: set[ModuleImportState] = set(initial)
    deferred_bodies: dict[ImportNodeKey, Sequence[ast.stmt]] = {}
    pending_bodies: deque[ImportNodeKey] = deque()
    deferred_expressions: dict[ImportNodeKey, ast.expr] = {}
    future_annotations = any(
        isinstance(statement, ast.ImportFrom)
        and statement.module == "__future__"
        and any(alias.name == "annotations" for alias in statement.names)
        for statement in getattr(tree, "body", ())
    )

    def record(node: ast.AST, states: tuple[ModuleImportState, ...]) -> None:
        # Source operands capture metadata before later sibling argument effects.
        if isinstance(node, (ast.Import, ast.ImportFrom, ast.Call)) or (
            source_discovery
            and isinstance(node, ast.Name)
            and isinstance(node.ctx, ast.Load)
            and node.id in _IMPORT_METADATA_NAMES
        ):
            if source_discovery:
                # Completeness consumes the existing storage proof at the exact
                # demand point. This covers imports/getters/calls, truth and
                # release callbacks, and deferred activation without another
                # syntax/effect classifier or a per-query tree walk.
                node_key = PythonNodeKey.from_node(node)
                proof = None
                requires_proof = True
                if isinstance(node, (ast.Import, ast.ImportFrom)):
                    statement = statement_facts.get(node_key)
                    if statement is not None:
                        proof = statement.module_metadata_at_entry
                elif isinstance(node, ast.Name):
                    read = expression_facts.get(node_key)
                    if read is not None:
                        proof = read.module_metadata
                else:
                    call = call_facts.get(node_key)
                    requires_proof = call is not None and bool(
                        call.possible_import_call_kinds()
                    )
                    if requires_proof:
                        assert call is not None
                        proof = call.module_metadata_at_invocation
                if requires_proof and (
                    proof is None or not proof.admits_current_namespace
                ):
                    states = _merge_states(states, unknown_states(states))
            key = python_node_source_key(node)
            previous = by_node.get(key, ())
            by_node[key] = _merge_states(previous, states)

    def expression_result(node: ast.expr) -> StaticExpressionResult:
        fact = expression_facts.get(PythonNodeKey.from_node(node))
        return UNKNOWN_EXPRESSION_RESULT if fact is None else fact.result

    def metadata_assignment(
        states: tuple[ModuleImportState, ...], target: ast.AST, value: ast.AST
    ) -> tuple[ModuleImportState, ...]:
        updated = _merge_states(
            update_module_import_state(
                state,
                target,
                value,
                call_facts.get(PythonNodeKey.from_node(value)),
                fact_result=expression_result,
            )
            for state in states
        )
        all_states.update(updated)
        return updated

    def assign_states(
        states: tuple[ModuleImportState, ...],
        targets: Sequence[ast.AST],
        value: ast.AST,
        *,
        direct_metadata_names: Collection[str] = _IMPORT_METADATA_NAMES,
        raised_states: list[tuple[ModuleImportState, ...]] | None = None,
    ) -> tuple[ModuleImportState, ...]:
        current = states
        for target in targets:
            if isinstance(target, (ast.Tuple, ast.List)):
                if isinstance(value, (ast.Tuple, ast.List)) and len(target.elts) == len(
                    value.elts
                ):
                    for element, element_value in zip(target.elts, value.elts):
                        current = assign_states(
                            current,
                            (element,),
                            element_value,
                            direct_metadata_names=direct_metadata_names,
                            raised_states=raised_states,
                        )
                else:
                    current = expression_effects(
                        target,
                        current,
                        direct_metadata_names=direct_metadata_names,
                        raised_states=raised_states,
                    )
                    if any(
                        target_writes_metadata(element, direct_metadata_names)
                        for element in target.elts
                    ):
                        current = unknown_states(current)
            else:
                current = expression_effects(
                    target,
                    current,
                    direct_metadata_names=direct_metadata_names,
                    raised_states=raised_states,
                )
                if target_writes_metadata(target, direct_metadata_names):
                    current = metadata_assignment(current, target, value)
            current = target_completion_states(target, current, direct_metadata_names)
        all_states.update(current)
        return current

    def target_completion_states(
        target: ast.AST,
        states: tuple[ModuleImportState, ...],
        direct_metadata_names: Collection[str],
    ) -> tuple[ModuleImportState, ...]:
        key = PythonNodeKey.from_node(target)
        effects = assignment_effects.get(key, UNKNOWN_EFFECTS)
        iteration = iteration_facts.get(key)
        if (
            iteration is not None
            and iteration.module_metadata_effects & WRITES_MODULE_METADATA
        ):
            return callback_states(states)
        if effects & WRITES_MODULE_METADATA and not target_writes_metadata(
            target, direct_metadata_names
        ):
            return callback_states(states)
        if effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS:
            return callback_states(states)
        return states

    def unknown_states(
        states: tuple[ModuleImportState, ...],
    ) -> tuple[ModuleImportState, ...]:
        current = _merge_states(_unknown_module_import_state(state) for state in states)
        all_states.update(current)
        return current

    def callback_states(
        states: tuple[ModuleImportState, ...],
    ) -> tuple[ModuleImportState, ...]:
        # Completed callback/escape summaries can choose metadata absent from
        # visible source. Every such summary uses this projection: semantics
        # loses the anchor, discovery retains candidates beside uncertainty.
        # Explicit opaque metadata replacement instead uses unknown_states.
        unknown = unknown_states(states)
        return _merge_states(states, unknown) if source_discovery else unknown

    def bind_imported_names(
        states: tuple[ModuleImportState, ...],
        statement: ast.Import | ast.ImportFrom,
        direct_metadata_names: Collection[str],
        raised_states: list[tuple[ModuleImportState, ...]],
    ) -> tuple[ModuleImportState, ...]:
        # The request itself uses the incoming metadata. Binding its results
        # happens afterwards and may change the anchor of the next import.
        if isinstance(statement, ast.ImportFrom) and any(
            alias.name == "*" for alias in statement.names
        ):
            # __all__ may export metadata and change the next import anchor.
            # It may also fail after a prefix of those bindings was published.
            return unknown_states(states)
        current = states
        for alias in statement.names:
            target = ast.Name(id=python_import_binding(alias))
            if target_writes_metadata(target, direct_metadata_names):
                # Imported values are opaque; a successful __path__ binding
                # still establishes presence, as any ordinary assignment does.
                current = metadata_assignment(current, target, target)
            # A later alias can fail after earlier aliases were bound. Keep
            # those states for handlers, not just the statement endpoints.
            raised_states.append(current)
        return current

    def target_writes_metadata(
        target: ast.AST,
        direct_metadata_names: Collection[str] = _IMPORT_METADATA_NAMES,
    ) -> bool:
        target_name = import_metadata_target_name(target)
        if target_name in _IMPORT_METADATA_NAMES and (
            not isinstance(target, ast.Name) or target_name in direct_metadata_names
        ):
            return True
        if isinstance(target, (ast.Tuple, ast.List)):
            return any(
                target_writes_metadata(element, direct_metadata_names)
                for element in target.elts
            )
        return False

    def scope_global_metadata_names(statements: Sequence[ast.stmt]) -> frozenset[str]:
        names: set[str] = set()
        pending: list[ast.AST] = list(statements)
        while pending:
            node = pending.pop()
            if isinstance(node, ast.Global):
                names.update(node.names)
                continue
            if isinstance(
                node,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            pending.extend(ast.iter_child_nodes(node))
        return frozenset(names & _IMPORT_METADATA_NAMES)

    def expression_effects(
        expression: ast.AST,
        states: tuple[ModuleImportState, ...],
        *,
        direct_metadata_names: Collection[str] = _IMPORT_METADATA_NAMES,
        raised_states: list[tuple[ModuleImportState, ...]] | None = None,
    ) -> tuple[ModuleImportState, ...]:
        current = expression_metadata_transfer(
            expression,
            states,
            direct_metadata_names=direct_metadata_names,
            raised_states=raised_states,
        )
        fact = expression_facts.get(PythonNodeKey.from_node(expression))
        if (
            not isinstance(expression, ast.Call)
            and fact is not None
            and fact.module_metadata_effects & WRITES_MODULE_METADATA
        ):
            current = callback_states(current)
        if raised_states is not None and (
            fact is None or (fact.effects | fact.truth_effects) & RAISES
        ):
            raised_states.append(_merge_states(states, current))
        return current

    def comprehension_metadata_transfer(
        expression: ast.ListComp | ast.SetComp | ast.DictComp | ast.GeneratorExp,
        states: tuple[ModuleImportState, ...],
        *,
        direct_metadata_names: Collection[str],
        raised_states: list[tuple[ModuleImportState, ...]] | None,
    ) -> tuple[ModuleImportState, ...]:
        # The first iterable was evaluated in the enclosing activation. Every
        # nested iterable belongs inside its parent's backedge. Protocol and
        # release effects are projections of completed binding facts.
        def evaluate(
            node: ast.AST, incoming: tuple[ModuleImportState, ...]
        ) -> tuple[ModuleImportState, ...]:
            return expression_effects(
                node,
                incoming,
                direct_metadata_names=direct_metadata_names,
                raised_states=raised_states,
            )

        def unreachable_tail(index: int, condition_index: int = 0) -> None:
            for skipped in expression.generators[index].ifs[condition_index:]:
                record_unreachable(skipped)
            for skipped_generator in expression.generators[index + 1 :]:
                record_unreachable(skipped_generator)
            if isinstance(expression, ast.DictComp):
                record_unreachable(expression.key)
                record_unreachable(expression.value)
            else:
                record_unreachable(expression.elt)

        def generate(
            index: int, incoming: tuple[ModuleImportState, ...]
        ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
            generator = expression.generators[index]
            entry = evaluate(generator.iter, incoming) if index else incoming
            iteration = iteration_facts.get(PythonNodeKey.from_node(generator))

            def filters(
                condition_index: int, current: tuple[ModuleImportState, ...]
            ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
                if condition_index == len(generator.ifs):
                    if index + 1 < len(expression.generators):
                        return generate(index + 1, current)
                    payloads = (
                        (expression.key, expression.value)
                        if isinstance(expression, ast.DictComp)
                        else (expression.elt,)
                    )
                    for payload in payloads:
                        current = evaluate(payload, current)
                    return PythonCompletionFlow(normal=current)
                condition = generator.ifs[condition_index]
                current = evaluate(condition, current)
                truth = expression_truth(condition)
                flow: PythonCompletionFlow[tuple[ModuleImportState, ...]] = (
                    PythonCompletionFlow(
                        continued=current if truth is not True else None
                    )
                )
                if truth is not False:
                    flow = flow.merge(
                        filters(condition_index + 1, current), join_states=_merge_states
                    )
                else:
                    unreachable_tail(index, condition_index + 1)
                return flow

            def advance(
                header: tuple[ModuleImportState, ...],
            ) -> tuple[
                tuple[ModuleImportState, ...] | None,
                PythonCompletionFlow[tuple[ModuleImportState, ...]],
            ]:
                current = (
                    callback_states(header)
                    if iteration is None
                    or (iteration.effects | iteration.module_metadata_effects)
                    & (NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS | WRITES_MODULE_METADATA)
                    else header
                )
                if raised_states is not None and (
                    iteration is None or iteration.effects & RAISES
                ):
                    raised_states.append(current)
                if iteration is not None and iteration.empty:
                    unreachable_tail(index)
                    return current, PythonCompletionFlow()
                assigned = expression_effects(
                    generator.target,
                    current,
                    direct_metadata_names=(),
                    raised_states=raised_states,
                )
                if target_writes_metadata(generator.target, ()):
                    assigned = unknown_states(assigned)
                assigned = target_completion_states(generator.target, assigned, ())
                return current, filters(0, assigned)

            return PythonCompletionFlow.loop(
                entry,
                advance,
                lambda exhausted: PythonCompletionFlow(normal=exhausted),
                join_states=_merge_states,
                equivalent_states=lambda left, right: left == right,
                widen_state=unknown_states,
                finalize=(
                    lambda state: PythonCompletionFlow(normal=callback_states(state))
                )
                if iteration is not None and iteration.release_effects
                else None,
            )

        flow = generate(0, states)
        return flow.normal or ()

    def expression_metadata_transfer(
        expression: ast.AST,
        states: tuple[ModuleImportState, ...],
        *,
        direct_metadata_names: Collection[str],
        raised_states: list[tuple[ModuleImportState, ...]] | None,
    ) -> tuple[ModuleImportState, ...]:
        current = states

        def transfer(
            child: ast.AST, incoming: tuple[ModuleImportState, ...]
        ) -> tuple[ModuleImportState, ...]:
            return expression_effects(
                child,
                incoming,
                direct_metadata_names=direct_metadata_names,
                raised_states=raised_states,
            )

        if isinstance(
            expression, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
        ):
            current = transfer(expression.generators[0].iter, current)
            if isinstance(expression, ast.GeneratorExp):
                # Only first-iterator acquisition runs at creation. Its body is
                # projected with the existing deferred-expression worklist.
                deferred_expressions[python_node_source_key(expression)] = expression
                iteration = iteration_facts.get(
                    PythonNodeKey.from_node(expression.generators[0])
                )
                if iteration is None or (
                    iteration.effects | iteration.module_metadata_effects
                ) & (NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS | WRITES_MODULE_METADATA):
                    current = callback_states(current)
                return current
            return comprehension_metadata_transfer(
                expression,
                current,
                direct_metadata_names=direct_metadata_names,
                raised_states=raised_states,
            )
        if isinstance(expression, ast.IfExp):
            current = transfer(expression.test, current)
            truth = expression_truth(expression.test)
            outcomes: list[tuple[ModuleImportState, ...]] = []
            for live, branch in (
                (truth is not False, expression.body),
                (truth is not True, expression.orelse),
            ):
                if live:
                    outcomes.append(transfer(branch, current))
                else:
                    record_unreachable(branch)
            return _merge_states(*outcomes)
        if isinstance(expression, ast.BoolOp):
            outcomes = []
            for index, value in enumerate(expression.values):
                current = transfer(value, current)
                truth = expression_truth(value)
                last = index == len(expression.values) - 1
                stops = (
                    truth is False
                    if isinstance(expression.op, ast.And)
                    else truth is True
                )
                if last or stops or truth is None:
                    outcomes.append(current)
                if stops:
                    for skipped in expression.values[index + 1 :]:
                        record_unreachable(skipped)
                    break
            return _merge_states(*outcomes)
        if isinstance(expression, ast.Compare):
            current = transfer(expression.left, current)
            outcomes = []
            for comparator in expression.comparators:
                current = transfer(comparator, current)
                # Every comparison can terminate a chain. Preserve correlation
                # rather than forcing the last comparator's writes on all exits.
                outcomes.append(current)
            return _merge_states(*outcomes)
        if isinstance(expression, ast.Call):
            current = transfer(expression.func, current)
            arguments = (*expression.args, *expression.keywords)
            for step in call_argument_schedule(expression):
                if step.action == "evaluate":
                    current = transfer(step.expression, current)
                elif step.action in {"star", "kwstar"}:
                    iteration = iteration_facts.get(
                        PythonNodeKey.from_node(arguments[step.index])
                    )
                    if (
                        iteration is None
                        or iteration.module_metadata_effects & WRITES_MODULE_METADATA
                    ):
                        current = callback_states(current)
        else:
            for child in expression_evaluation_children(expression):
                current = transfer(child, current)
        # Calls observe import metadata after their callee and arguments run.
        # Recursive pre-recording would stamp later siblings with stale state.
        record(expression, current)
        if isinstance(expression, ast.NamedExpr):
            current = assign_states(
                current,
                (expression.target,),
                expression.value,
                direct_metadata_names=direct_metadata_names,
                raised_states=raised_states,
            )
            return current
        call = call_facts.get(PythonNodeKey.from_node(expression))
        if (
            isinstance(expression, ast.Call)
            and call is not None
            and (
                mutation := globals_mutation_call_identity(
                    expression, call.callee_identities
                )
            )
            is not None
            and (source_discovery or call.callee_is(mutation))
        ):
            # Shared binding authority owns possible identity and argument shape.
            # Only execution transfer requires an exact callable; source transfer
            # retains the possible publication beside an unknown alternative.
            effects = assignment_effects.get(
                PythonNodeKey.from_node(expression), UNKNOWN_EFFECTS
            )
            if not source_discovery and effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS:
                return unknown_states(current)
            key_fact = expression_facts.get(PythonNodeKey.from_node(expression.args[0]))
            name = None if key_fact is None else key_fact.static_value
            if not isinstance(name, str):
                if effects & WRITES_MODULE_METADATA:
                    current = callback_states(current)
            elif name in _IMPORT_METADATA_NAMES:
                target = ast.Name(id=name)
                if mutation == PythonIdentity.GLOBALS_DELITEM:
                    current = _merge_states(
                        invalidate_module_import_state(state, target, deleted=True)
                        for state in current
                    )
                else:
                    current = metadata_assignment(current, target, expression.args[1])
            # CPython publishes the replacement/deletion before releasing the
            # previous value. Even an unrelated key can release a metadata writer.
            if (
                not call.callee_is(mutation)
                or effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
            ):
                current = callback_states(current)
            return current
        fact = expression_facts.get(PythonNodeKey.from_node(expression))
        if fact is not None and fact.module_metadata_effects & WRITES_MODULE_METADATA:
            return callback_states(current)
        # Ordinary foreign code has its own global mapping. Completed binding
        # execution facts own local deferred mutation and namespace escape;
        # ModuleSpec parsing consumes the same canonical call-site identity.
        return current

    def record_unreachable(node: ast.AST) -> None:
        # Explicit absence differs from an unobserved deferred execution phase.
        # Never erase a site reached through another incoming completion.
        for child in ast.walk(node):
            if isinstance(child, (ast.stmt, ast.Call)):
                by_node.setdefault(python_node_source_key(child), ())

    def unreachable_block(
        statements: Sequence[ast.stmt],
    ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
        for statement in statements:
            record_unreachable(statement)
        return PythonCompletionFlow()

    def expression_truth(node: ast.expr) -> bool | None:
        fact = expression_facts.get(PythonNodeKey.from_node(node))
        return None if fact is None else fact.result.truth

    def flow_statements(
        statements: Sequence[ast.stmt],
        states: tuple[ModuleImportState, ...],
        *,
        eager_annotations: bool = True,
        direct_metadata_names: Collection[str] = _IMPORT_METADATA_NAMES,
    ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
        pending_expression_raises: list[tuple[ModuleImportState, ...]] = []

        def evaluate(
            expression: ast.AST,
            incoming: tuple[ModuleImportState, ...],
        ) -> tuple[ModuleImportState, ...]:
            return expression_effects(
                expression,
                incoming,
                direct_metadata_names=direct_metadata_names,
                raised_states=pending_expression_raises,
            )

        completion = PythonCompletionFlow(normal=states if states else None)
        for statement in statements:
            pending_expression_raises.clear()
            current = completion.normal
            if current is None:
                record_unreachable(statement)
                continue
            before = current
            # Statement identity is also an explicit reachability projection.
            key = python_node_source_key(statement)
            by_node[key] = _merge_states(by_node.get(key, ()), current)
            outcome: PythonCompletionFlow[tuple[ModuleImportState, ...]] | None = None
            if isinstance(statement, (ast.Import, ast.ImportFrom)):
                record(statement, current)
                current = bind_imported_names(
                    current, statement, direct_metadata_names, pending_expression_raises
                )
            elif isinstance(statement, ast.Assign):
                current = evaluate(statement.value, current)
                current = assign_states(
                    current,
                    statement.targets,
                    statement.value,
                    direct_metadata_names=direct_metadata_names,
                    raised_states=pending_expression_raises,
                )
            elif isinstance(statement, ast.AnnAssign):
                if statement.value is not None:
                    current = evaluate(statement.value, current)
                    current = assign_states(
                        current,
                        (statement.target,),
                        statement.value,
                        direct_metadata_names=direct_metadata_names,
                        raised_states=pending_expression_raises,
                    )
                else:
                    current = evaluate(statement.target, current)
                if (
                    eager_annotations
                    and context.target_python < (3, 14)
                    and not future_annotations
                ):
                    current = evaluate(statement.annotation, current)
            elif isinstance(statement, ast.AugAssign):
                record(statement, current)
                current = evaluate(statement.target, current)
                current = evaluate(statement.value, current)
                if target_writes_metadata(statement.target, direct_metadata_names):
                    current = _merge_states(
                        invalidate_module_import_state(state, statement.target)
                        for state in current
                    )
                    all_states.update(current)
                current = target_completion_states(
                    statement.target, current, direct_metadata_names
                )
            elif isinstance(statement, ast.Delete):
                for target in statement.targets:
                    current = evaluate(target, current)
                    if target_writes_metadata(target, direct_metadata_names):
                        current = _merge_states(
                            invalidate_module_import_state(state, target, deleted=True)
                            for state in current
                        )
                    current = target_completion_states(
                        target, current, direct_metadata_names
                    )
                all_states.update(current)
            elif isinstance(statement, ast.If):
                current = evaluate(statement.test, current)
                truth = expression_truth(statement.test)
                body = (
                    flow_statements(
                        statement.body,
                        current,
                        eager_annotations=eager_annotations,
                        direct_metadata_names=direct_metadata_names,
                    )
                    if truth is not False
                    else unreachable_block(statement.body)
                )
                alternate = (
                    flow_statements(
                        statement.orelse,
                        current,
                        eager_annotations=eager_annotations,
                        direct_metadata_names=direct_metadata_names,
                    )
                    if truth is not True
                    else unreachable_block(statement.orelse)
                )
                outcome = body.merge(alternate, join_states=_merge_states)
            elif isinstance(statement, (ast.For, ast.AsyncFor, ast.While)):
                if not isinstance(statement, ast.While):
                    current = evaluate(statement.iter, current)
                loop_fact = statement_facts.get(PythonNodeKey.from_node(statement))
                iteration = None if loop_fact is None else loop_fact.iteration

                def advance(
                    header: tuple[ModuleImportState, ...],
                ) -> tuple[
                    tuple[ModuleImportState, ...] | None,
                    PythonCompletionFlow[tuple[ModuleImportState, ...]],
                ]:
                    raised: list[tuple[ModuleImportState, ...]] = []
                    if isinstance(statement, ast.While):
                        tested = expression_effects(
                            statement.test,
                            header,
                            direct_metadata_names=direct_metadata_names,
                            raised_states=raised,
                        )
                        truth = expression_truth(statement.test)
                    else:
                        # The binding/completion authority owns iterator and
                        # target callbacks; import consumers project its facts.
                        tested = (
                            callback_states(header)
                            if iteration is None
                            or (iteration.effects | iteration.module_metadata_effects)
                            & (
                                NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
                                | WRITES_MODULE_METADATA
                            )
                            else header
                        )
                        if iteration is None or iteration.effects & RAISES:
                            raised.append(tested)
                        truth = (
                            False if iteration is not None and iteration.empty else None
                        )
                    body_entry = tested
                    if not isinstance(statement, ast.While) and truth is not False:
                        if target_writes_metadata(
                            statement.target, direct_metadata_names
                        ):
                            body_entry = unknown_states(tested)
                        body_entry = target_completion_states(
                            statement.target, body_entry, direct_metadata_names
                        )
                        target_effects = assignment_effects.get(
                            PythonNodeKey.from_node(statement.target), UNKNOWN_EFFECTS
                        )
                        if target_effects & RAISES:
                            raised.append(_merge_states(tested, body_entry))
                    body = (
                        flow_statements(
                            statement.body,
                            body_entry,
                            eager_annotations=eager_annotations,
                            direct_metadata_names=direct_metadata_names,
                        )
                        if truth is not False
                        else unreachable_block(statement.body)
                    )
                    if raised:
                        body = body.merge(
                            PythonCompletionFlow(raised=_merge_states(*raised)),
                            join_states=_merge_states,
                        )
                    return tested if truth is not True else None, body

                outcome = PythonCompletionFlow.loop(
                    current,
                    advance,
                    lambda exhausted: flow_statements(
                        statement.orelse,
                        exhausted,
                        eager_annotations=eager_annotations,
                        direct_metadata_names=direct_metadata_names,
                    ),
                    join_states=_merge_states,
                    equivalent_states=lambda left, right: left == right,
                    widen_state=unknown_states,
                    finalize=(
                        lambda state: PythonCompletionFlow(
                            normal=callback_states(state)
                        )
                    )
                    if iteration is not None and iteration.release_effects
                    else None,
                )
                # A literal-true loop never visits its else suite. Missing facts
                # must not resurrect imports in an unvisited execution phase.
                if (
                    isinstance(statement, ast.While)
                    and expression_truth(statement.test) is True
                ):
                    unreachable_block(statement.orelse)
            elif isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
                for expression in (*statement.decorator_list, *statement.args.defaults):
                    current = evaluate(expression, current)
                for expression in statement.args.kw_defaults:
                    if expression is not None:
                        current = evaluate(expression, current)
                if context.target_python < (3, 14) and not future_annotations:
                    annotations = [
                        argument.annotation
                        for argument in (
                            *statement.args.posonlyargs,
                            *statement.args.args,
                            *statement.args.kwonlyargs,
                        )
                        if argument.annotation is not None
                    ]
                    if (
                        statement.args.vararg is not None
                        and statement.args.vararg.annotation is not None
                    ):
                        annotations.append(statement.args.vararg.annotation)
                    if (
                        statement.args.kwarg is not None
                        and statement.args.kwarg.annotation is not None
                    ):
                        annotations.append(statement.args.kwarg.annotation)
                    if statement.returns is not None:
                        annotations.append(statement.returns)
                    for expression in annotations:
                        current = evaluate(expression, current)
                deferred_key = python_node_source_key(statement)
                if deferred_key not in deferred_bodies:
                    pending_bodies.append(deferred_key)
                deferred_bodies[deferred_key] = statement.body
            elif isinstance(statement, ast.ClassDef):
                for expression in (
                    *statement.decorator_list,
                    *statement.bases,
                    *(keyword.value for keyword in statement.keywords),
                ):
                    current = evaluate(expression, current)
                class_global_metadata = scope_global_metadata_names(statement.body)
                # Class preparation can fail before the body independently of
                # decorator/base expression evaluation.
                pending_expression_raises.append(current)
                outcome = flow_statements(
                    statement.body,
                    current,
                    direct_metadata_names=class_global_metadata,
                )
                if outcome.normal is not None:
                    # Metaclass construction and class-name publication run only
                    # after a normally completed class body.
                    pending_expression_raises.append(outcome.normal)
            elif isinstance(statement, getattr(ast, "TypeAlias", ())):
                deferred_expressions[python_node_source_key(statement.value)] = (
                    statement.value
                )
                for type_param in statement.type_params:
                    for attribute in ("bound", "default_value"):
                        value = getattr(type_param, attribute, None)
                        if isinstance(value, ast.expr):
                            deferred_expressions[python_node_source_key(value)] = value
            elif isinstance(statement, (ast.With, ast.AsyncWith)):

                def enter_context(
                    index: int,
                    incoming: tuple[ModuleImportState, ...],
                ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
                    if index == len(statement.items):
                        return flow_statements(
                            statement.body,
                            incoming,
                            eager_annotations=eager_annotations,
                            direct_metadata_names=direct_metadata_names,
                        )
                    item = statement.items[index]
                    raised: list[tuple[ModuleImportState, ...]] = []
                    evaluated = expression_effects(
                        item.context_expr,
                        incoming,
                        direct_metadata_names=direct_metadata_names,
                        raised_states=raised,
                    )
                    entered = callback_states(evaluated)
                    prefix = PythonCompletionFlow(
                        raised=_merge_states(incoming, entered, *raised),
                    )
                    # Assignment and nested acquisition occur inside this manager;
                    # their failures may be suppressed by its exit callback.
                    assigned = PythonCompletionFlow(normal=entered)
                    if item.optional_vars is not None:
                        if target_writes_metadata(
                            item.optional_vars, direct_metadata_names
                        ):
                            entered = unknown_states(entered)
                        assigned = PythonCompletionFlow(normal=entered, raised=entered)
                    body = assigned.sequence(
                        lambda normal: enter_context(index + 1, normal),
                        join_states=_merge_states,
                    )
                    return prefix.merge(
                        body.unwind_context(
                            lambda state: PythonCompletionFlow(
                                normal=callback_states(state),
                                raised=callback_states(state),
                            ),
                            join_states=_merge_states,
                        ),
                        join_states=_merge_states,
                    )

                outcome = enter_context(0, current)
            elif isinstance(statement, (ast.Try, getattr(ast, "TryStar", ast.Try))):
                body = flow_statements(
                    statement.body,
                    current,
                    eager_annotations=eager_annotations,
                    direct_metadata_names=direct_metadata_names,
                )
                outcome = (
                    PythonCompletionFlow(normal=body.normal)
                    .sequence(
                        lambda state: flow_statements(
                            statement.orelse,
                            state,
                            eager_annotations=eager_annotations,
                            direct_metadata_names=direct_metadata_names,
                        ),
                        join_states=_merge_states,
                    )
                    .merge(
                        PythonCompletionFlow(
                            returned=body.returned,
                            broken=body.broken,
                            continued=body.continued,
                        ),
                        join_states=_merge_states,
                    )
                )
                if body.normal is None:
                    unreachable_block(statement.orelse)
                if body.raised is not None and statement.handlers:

                    def evaluate_handler_type(
                        handler: ast.ExceptHandler,
                        incoming: tuple[ModuleImportState, ...],
                    ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
                        if handler.type is None:
                            return PythonCompletionFlow(normal=incoming)
                        raised: list[tuple[ModuleImportState, ...]] = []
                        evaluated = expression_effects(
                            handler.type,
                            incoming,
                            direct_metadata_names=direct_metadata_names,
                            raised_states=raised,
                        )
                        # Even an inert handler expression can be invalid as an
                        # exception type. Matching failure bypasses all handlers.
                        return PythonCompletionFlow(
                            normal=evaluated,
                            raised=_merge_states(incoming, evaluated, *raised),
                        )

                    def execute_handler(
                        handler: ast.ExceptHandler,
                        incoming: tuple[ModuleImportState, ...],
                    ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
                        scoped_target = handler.name in direct_metadata_names
                        entered = (
                            unknown_states(incoming) if scoped_target else incoming
                        )
                        branch = flow_statements(
                            handler.body,
                            entered,
                            eager_annotations=eager_annotations,
                            direct_metadata_names=direct_metadata_names,
                        )
                        if scoped_target and handler.name is not None:
                            target = ast.Name(id=handler.name, ctx=ast.Del())
                            branch = branch.map_states(
                                lambda states: _merge_states(
                                    invalidate_module_import_state(
                                        state, target, deleted=True
                                    )
                                    for state in states
                                )
                            )
                        return branch

                    if isinstance(statement, ast.TryStar):

                        def group_protocol(
                            incoming: tuple[ModuleImportState, ...],
                        ) -> PythonCompletionFlow[tuple[ModuleImportState, ...]]:
                            # ExceptionGroup subclasses may implement split/derive
                            # in Python, independently of handler type expressions.
                            changed = callback_states(incoming)
                            return PythonCompletionFlow(
                                normal=changed,
                                raised=_merge_states(incoming, changed),
                            )

                        handled = PythonCompletionFlow.exception_group_handlers(
                            body.raised,
                            statement.handlers,
                            evaluate_type=evaluate_handler_type,
                            execute_handler=execute_handler,
                            split_group=group_protocol,
                            merge_group=group_protocol,
                            join_states=_merge_states,
                        )
                        outcome = outcome.merge(handled, join_states=_merge_states)
                    else:
                        unmatched: tuple[ModuleImportState, ...] | None = body.raised
                        for handler in statement.handlers:
                            if unmatched is None:
                                record_unreachable(handler)
                                continue
                            evaluated = evaluate_handler_type(handler, unmatched)
                            outcome = outcome.merge(
                                evaluated.without_normal(),
                                join_states=_merge_states,
                            )
                            unmatched = evaluated.normal
                            if unmatched is not None:
                                outcome = outcome.merge(
                                    execute_handler(handler, unmatched),
                                    join_states=_merge_states,
                                )
                            if handler.type is None:
                                unmatched = None
                        if unmatched is not None:
                            outcome = outcome.merge(
                                PythonCompletionFlow(raised=unmatched),
                                join_states=_merge_states,
                            )
                elif body.raised is not None:
                    outcome = outcome.merge(
                        PythonCompletionFlow(raised=body.raised),
                        join_states=_merge_states,
                    )
                else:
                    for handler in statement.handlers:
                        record_unreachable(handler)
                if statement.finalbody:
                    if not outcome.successors():
                        unreachable_block(statement.finalbody)
                    outcome = outcome.apply_finally(
                        lambda state: flow_statements(
                            statement.finalbody,
                            state,
                            eager_annotations=eager_annotations,
                            direct_metadata_names=direct_metadata_names,
                        ),
                        join_states=_merge_states,
                    )
            elif isinstance(statement, ast.Match):
                current = evaluate(statement.subject, current)
                branches = PythonCompletionFlow()
                unmatched: tuple[ModuleImportState, ...] | None = current
                for case in statement.cases:
                    if unmatched is None:
                        record_unreachable(case)
                        continue
                    irrefutable = (
                        python_pattern_irrefutable_reason(case.pattern) is not None
                    )
                    case_states = unmatched
                    if not python_pattern_is_capture_only(case.pattern):
                        pending_expression_raises.append(case_states)
                    pattern_names = set(python_pattern_capture_names(case.pattern))
                    if pattern_names & set(direct_metadata_names):
                        case_states = unknown_states(case_states)
                    guard_truth = True
                    if case.guard is not None:
                        case_states = evaluate(case.guard, case_states)
                        guard_truth = expression_truth(case.guard)
                    if guard_truth is not False:
                        branches = branches.merge(
                            flow_statements(
                                case.body,
                                case_states,
                                eager_annotations=eager_annotations,
                                direct_metadata_names=direct_metadata_names,
                            ),
                            join_states=_merge_states,
                        )
                    else:
                        unreachable_block(case.body)
                    if irrefutable and guard_truth is True:
                        unmatched = None
                    elif guard_truth is not True:
                        unmatched = _merge_states(unmatched, case_states)
                if unmatched is not None:
                    branches = branches.merge(
                        PythonCompletionFlow(normal=unmatched),
                        join_states=_merge_states,
                    )
                outcome = branches
            else:
                record(statement, current)
                current = evaluate(statement, current)
            fact = statement_facts.get(PythonNodeKey.from_node(statement))
            if (
                fact is not None
                and fact.module_metadata_effects & WRITES_MODULE_METADATA
            ):
                current = callback_states(current)
                if outcome is not None:
                    outcome = outcome.map_states(callback_states)
            mask = (
                fact.completions
                if fact is not None
                else PythonCompletion.NORMAL | PythonCompletion.RAISE
            )
            if outcome is None:
                outcome = PythonCompletionFlow()
                for kind in PythonCompletion:
                    if kind is not PythonCompletion.NONE and mask & kind:
                        outcome = outcome.merge(
                            PythonCompletionFlow.single(
                                kind,
                                _merge_states(before, current)
                                if kind == PythonCompletion.RAISE
                                else current,
                            ),
                            join_states=_merge_states,
                        )
            if pending_expression_raises:
                outcome = outcome.merge(
                    PythonCompletionFlow(
                        raised=_merge_states(*pending_expression_raises)
                    ),
                    join_states=_merge_states,
                )
            if fact is not None:
                projected = PythonCompletionFlow()
                for kind, state in outcome.successors():
                    if mask & kind:
                        projected = projected.merge(
                            PythonCompletionFlow.single(kind, state),
                            join_states=_merge_states,
                        )
                outcome = projected
            completion = completion.without_normal().merge(
                outcome, join_states=_merge_states
            )
            for _kind, state in outcome.successors():
                all_states.update(state)
        return completion

    body = tuple(getattr(tree, "body", ()))
    module_completion = flow_statements(
        body,
        initial,
    )
    final_states = module_completion.normal or ()
    deferred_states = _merge_states(all_states, final_states)
    # Function imports observe globals when called. Graph consumers therefore
    # union every reachable module state; frontend lowering keeps them relative.
    while pending_bodies:
        body_key = pending_bodies.popleft()
        deferred_body = deferred_bodies[body_key]
        body_states = deferred_states
        flow_statements(
            deferred_body,
            body_states,
            eager_annotations=False,
            direct_metadata_names=scope_global_metadata_names(deferred_body),
        )
    while deferred_expressions:
        _key, deferred_expression = deferred_expressions.popitem()
        if isinstance(deferred_expression, ast.GeneratorExp):
            comprehension_metadata_transfer(
                deferred_expression,
                deferred_states,
                direct_metadata_names=_IMPORT_METADATA_NAMES,
                raised_states=None,
            )
        else:
            expression_effects(deferred_expression, deferred_states)
    return ModuleImportFlow(
        by_node, final_states, _merge_states(all_states, final_states)
    )


def analyze_module_import_flow(
    tree: ast.AST,
    context: ModuleImportContext,
    *,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> ModuleImportFlow:
    """Return import facts from the canonical binding/capability index."""

    from molt.compiler_analysis.python_binding_flow import (
        PythonBindingPolicy,
        analyze_python_bindings,
    )

    context = _normalized_import_context(context)
    if isinstance(tree, ast.Module):
        module = tree
    elif isinstance(tree, ast.stmt):
        module = ast.Module(body=[tree], type_ignores=[])
    elif isinstance(tree, ast.expr):
        module = ast.Module(body=[ast.Expr(value=tree)], type_ignores=[])
    else:
        module = ast.Module(body=[], type_ignores=[])
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    index = analyze_python_bindings(
        module,
        source_digest=ast_digest_admission.digest,
        policy=PythonBindingPolicy(
            target_python=context.target_python,
            module_name=context.module_name,
            module_spec_name=context.spec_name,
            module_is_package=context.is_package,
            module_execution_kind=context.execution_kind,
        ),
    )
    return index.module_import_flow


def final_module_import_states(
    tree: ast.AST,
    context: ModuleImportContext,
) -> tuple[ModuleImportState, ...]:
    return analyze_module_import_flow(tree, context).final_states


def _fallback_package(state: ModuleImportState) -> StaticMetadataValue:
    if state.spec_parent.kind == "known":
        return state.spec_parent
    if state.spec_parent.kind in {"invalid", "unknown"}:
        return state.spec_parent
    if state.name.kind != "known":
        return state.name
    assert state.name.value is not None
    return StaticMetadataValue.known(
        state.name.value if state.has_path else state.name.value.rpartition(".")[0]
    )


def effective_relative_package(context: ModuleImportContext) -> StaticMetadataValue:
    state = context_import_state(context)
    if context.target_python >= (3, 15):
        raise ValueError(
            "Python 3.15 import execution semantics are not authorized until "
            "PEP 810 lazy imports and __package__ resolution are implemented "
            "and proven atomically"
        )
    if state.package.kind == "known":
        return state.package
    if state.package.kind == "none":
        return _fallback_package(state)
    return state.package


def resolve_relative_import(
    module: str | None,
    level: int,
    context: ModuleImportContext,
) -> RelativeImportResolution:
    if level < 0:
        return RelativeImportResolution(None, "negative_level")
    if level == 0:
        return RelativeImportResolution(module)
    if context.target_python >= (3, 15):
        effective_relative_package(context)
        raise AssertionError("unreachable Python 3.15 import resolution")
    state = context_import_state(context)
    if state.package.kind == "known":
        if state.spec_parent.kind == "invalid":
            return RelativeImportResolution(None, "invalid_spec")
        package = state.package
        # CPython retains the explicit package before consulting spec.parent.
        # An unknown spec can fail or execute callbacks/warnings, but successful
        # resolution still uses this known anchor. Keep graph identity distinct
        # from the required runtime execution of that protocol.
        requires_runtime = (
            state.spec_parent.kind == "unknown"
            or state.spec_parent.kind == "known"
            and state.spec_parent.value != state.package.value
        )
    elif state.package.kind == "invalid":
        return RelativeImportResolution(None, "invalid_package")
    elif state.package.kind == "unknown":
        return RelativeImportResolution(None, "unknown_package")
    elif state.spec_parent.kind == "known":
        package = state.spec_parent
        requires_runtime = False
    elif state.spec_parent.kind == "invalid":
        return RelativeImportResolution(None, "invalid_spec")
    elif state.spec_parent.kind == "unknown":
        return RelativeImportResolution(None, "unknown_spec")
    elif state.name.kind == "absent":
        return RelativeImportResolution(None, "missing_name")
    elif state.name.kind in {"none", "invalid"}:
        return RelativeImportResolution(None, "invalid_name")
    elif state.name.kind == "unknown":
        return RelativeImportResolution(None, "unknown_name")
    else:
        assert state.name.value is not None
        package = StaticMetadataValue.known(
            state.name.value if state.has_path else state.name.value.rpartition(".")[0]
        )
        # CPython warns when it must fall back to __name__/__path__. Runtime
        # lowering preserves that observable warning even when graph analysis
        # can still derive a conservative module candidate.
        requires_runtime = True
    if not package.value:
        return RelativeImportResolution(None, "no_parent")
    parts = package.value.split(".")
    if level > len(parts):
        return RelativeImportResolution(None, "beyond_top")
    base = ".".join(parts[: len(parts) - (level - 1)])
    return RelativeImportResolution(
        f"{base}.{module}" if base and module else module or base or None,
        requires_runtime=requires_runtime,
    )


def static_import_candidates(base: str, fromlist: Sequence[str]) -> tuple[str, ...]:
    if not base:
        return ()
    return (base, *(f"{base}.{name}" for name in fromlist if name and name != "*"))


def project_static_import_request(
    request: StaticImportRequest,
    context: ModuleImportContext,
) -> StaticImportProjection:
    if request.level_is_invalid:
        return StaticImportProjection((), "invalid_level")
    if not request.name and request.level == 0:
        return StaticImportProjection((), "empty_name")
    if request.level < 0:
        return StaticImportProjection((), "negative_level")
    name = request.name
    level = request.level
    resolution_context = context
    if request.kind == "import_module":
        leading = len(name) - len(name.lstrip("."))
        if not leading:
            return StaticImportProjection(
                static_import_candidates(name, request.fromlist)
            )
        level = leading
        name = name[leading:]
        package = request.package_argument
        if package is None or package.kind == "none":
            return StaticImportProjection((), "no_parent")
        if package.kind == "invalid":
            return StaticImportProjection((), "invalid_package")
        if package.kind == "unknown":
            return StaticImportProjection((), "unknown_package", True)
        resolution_context = context.with_state(
            ModuleImportState(package, package, package, False)
        )
    elif request.kind == "dunder_import" and level > 0:
        if not request.globals_were_supplied:
            return StaticImportProjection((), "missing_globals")
        if request.globals_state is None:
            return StaticImportProjection((), "unknown_package", True)
        resolution_context = context.with_state(request.globals_state)
    resolution = resolve_relative_import(name or None, level, resolution_context)
    if resolution.module is None:
        return StaticImportProjection(
            (),
            resolution.error,
            resolution.error in {"unknown_package", "unknown_spec", "unknown_name"},
        )
    return StaticImportProjection(
        static_import_candidates(resolution.module, request.fromlist),
        requires_runtime_execution=resolution.requires_runtime,
    )


def plan_static_import_request(
    request: StaticImportRequest,
    contexts: Sequence[ModuleImportContext],
) -> StaticImportPlan:
    """Plan graph candidates without erasing errors or runtime custody."""

    modules: list[str] = []
    seen: set[str] = set()
    errors: set[ImportResolutionError] = set()
    requires_runtime = False
    requires_runtime_execution = False
    for context in contexts:
        projection = project_static_import_request(request, context)
        requires_runtime |= projection.requires_runtime
        requires_runtime_execution |= projection.requires_runtime_execution
        if projection.error is not None:
            errors.add(projection.error)
        for module in projection.modules:
            if module not in seen:
                seen.add(module)
                modules.append(module)
    requires_runtime_execution |= bool(modules and errors)
    return StaticImportPlan(
        tuple(modules),
        tuple(sorted(errors)),
        requires_runtime,
        requires_runtime_execution,
    )


@dataclass(frozen=True, slots=True)
class StaticImportDiscovery:
    """Dependency candidates carry no semantic import admission."""

    source_modules: tuple[str, ...] = ()
    lexical_modules: tuple[str, ...] = ()
    # Candidate presence is not completeness: an unresolved sibling context
    # retains its runtime/manifest obligation even beside a known source branch.
    source_complete: bool = False

    @property
    def modules(self) -> tuple[str, ...]:
        return tuple(dict.fromkeys((*self.source_modules, *self.lexical_modules)))


def static_import_discovery(
    request: StaticImportRequest,
    contexts: Sequence[ModuleImportContext],
    *,
    lexical_request: StaticImportRequest | None = None,
) -> StaticImportDiscovery:
    """Project source possibilities and an explicitly separate loader twin.

    Development dependency closure can use a resolved source-state projection
    without claiming the runtime namespace is immutable. A genuinely unknown
    source anchor has no such projection; only product graph discovery may
    retain its lexical twin while arranging exact runtime catalog custody.
    """
    if not contexts:
        return StaticImportDiscovery()
    plan = plan_static_import_request(request, contexts)
    source = StaticImportDiscovery(
        source_modules=plan.modules,
        source_complete=not plan.requires_runtime and not plan.errors,
    )
    if not plan.requires_runtime:
        return source
    if lexical_request is None:
        if request.kind != "statement" or request.level <= 0:
            return source
        lexical_request = request
    lexical_contexts = tuple(
        ModuleImportContext(
            context.module_name,
            context.is_package,
            spec_name=context.spec_name,
            target_python=context.target_python,
            execution_kind=context.execution_kind,
        )
        for context in contexts
    )
    lexical_plan = plan_static_import_request(lexical_request, lexical_contexts)
    if lexical_plan.requires_runtime or lexical_plan.errors:
        return source
    return replace(source, lexical_modules=lexical_plan.modules)


def require_static_import_modules(
    plan: StaticImportPlan,
    *,
    consumer: str,
) -> tuple[str, ...]:
    if plan.requires_runtime:
        raise UnresolvedStaticImportError(
            f"{consumer} requires explicit runtime import custody; "
            "the source-ordered package anchor is dynamic"
        )
    if plan.errors:
        raise UnresolvedStaticImportError(
            f"{consumer} cannot resolve import: {', '.join(plan.errors)}"
        )
    return plan.modules
