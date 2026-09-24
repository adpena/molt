"""Canonical Python import execution-state and request semantics."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.compiler_analysis.python_binding_facts import (
        PythonCallSiteFact,
        PythonExpressionFact,
        PythonNodeKey,
        PythonStatementFact,
    )

import ast
from collections import deque
from collections.abc import Callable, Collection, Iterable, Mapping, Sequence
from dataclasses import dataclass, field, replace
from types import MappingProxyType
from typing import Literal

from molt.compiler_analysis.python_effects import (
    dotted_expression_name,
    expression_evaluation_children,
    expression_may_execute_python,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    python_node_source_key,
    python_pattern_capture_names,
    python_pattern_irrefutable_reason,
    python_pattern_is_capture_only,
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
class _PythonAstDigestAdmission:
    """Digest fact for one exact, read-only scan generation.

    Retaining the tree rejects substitution; this fact does not make a mutable
    AST immutable and must not outlive the non-mutating scan that created it.
    """

    tree: ast.AST = field(repr=False, compare=False)
    digest: str = field(init=False)

    def __post_init__(self) -> None:
        from molt.compiler_analysis.python_binding_flow import python_ast_digest

        object.__setattr__(self, "digest", python_ast_digest(self.tree))

    @classmethod
    def for_tree(
        cls,
        tree: ast.AST,
        admission: _PythonAstDigestAdmission | None = None,
    ) -> _PythonAstDigestAdmission:
        if admission is None:
            return cls(tree)
        if admission.tree is not tree:
            raise ValueError("AST digest admission belongs to a different tree")
        return admission


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
    proven_pure_calls: frozenset[str] = frozenset()


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
    name: ast.expr
    package: ast.expr | None = None
    globals: ast.expr | None = None
    locals: ast.expr | None = None
    fromlist: ast.expr | None = None
    level: ast.expr | None = None


def bind_static_import_call_arguments(
    call: ast.Call,
    kind: ImportOperationKind,
) -> StaticImportCallArguments:
    """Bind import-call arguments once with CPython duplicate/arity rules."""

    parameter_names = (
        ("name", "package")
        if kind == "import_module"
        else ("name", "globals", "locals", "fromlist", "level")
    )
    if kind == "statement":
        raise ValueError("import statements do not have call arguments")
    if len(call.args) > len(parameter_names):
        raise ValueError(f"too many {kind} positional arguments")
    bound: dict[str, ast.expr] = dict(zip(parameter_names, call.args))
    for keyword in call.keywords:
        if keyword.arg is None:
            raise ValueError(f"dynamic **kwargs are unsupported for {kind}")
        if keyword.arg not in parameter_names:
            raise ValueError(f"unexpected {kind} argument {keyword.arg!r}")
        if keyword.arg in bound:
            raise ValueError(f"duplicate {kind} argument {keyword.arg!r}")
        bound[keyword.arg] = keyword.value
    name = bound.get("name")
    if name is None:
        raise ValueError(f"{kind} requires a name argument")
    return StaticImportCallArguments(
        name=name,
        package=bound.get("package"),
        globals=bound.get("globals"),
        locals=bound.get("locals"),
        fromlist=bound.get("fromlist"),
        level=bound.get("level"),
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
    """Source-ordered abstract metadata states at import/call AST sites."""

    states_by_node: Mapping[ImportNodeKey, tuple[ModuleImportState, ...]]
    final_states: tuple[ModuleImportState, ...]
    all_states: tuple[ModuleImportState, ...]

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

    def states_for(self, node: ast.AST) -> tuple[ModuleImportState, ...]:
        # An unrecorded execution phase (deferred annotations, lambda/generator
        # bodies, or a newly introduced AST form) must never inherit the final
        # module snapshot. Unioning the observed source-order states is the
        # conservative runtime anchor until that phase has an explicit event.
        return self.states_by_node.get(python_node_source_key(node), self.all_states)


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
    proven_pure_calls: Collection[str] = (),
) -> StaticMetadataValue:
    """Evaluate a statically known, CPython-valid ModuleSpec parent."""

    if isinstance(value, ast.Constant) and value.value is None:
        return NONE_VALUE
    if (
        not isinstance(value, ast.Call)
        or dotted_expression_name(value.func) not in proven_pure_calls
        or expression_may_execute_python(value, proven_pure_calls=proven_pure_calls)
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


def _metadata_value(value: ast.AST) -> StaticMetadataValue:
    if isinstance(value, ast.Constant):
        if isinstance(value.value, str):
            return StaticMetadataValue.known(value.value)
        if value.value is None:
            return NONE_VALUE
        return INVALID_VALUE
    return UNKNOWN_VALUE


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


def metadata_value_from_expression(
    expression: ast.expr | None,
    context: ModuleImportContext,
    resolve_string: Callable[[ast.expr], str | None] | None = None,
) -> StaticMetadataValue | None:
    """Evaluate metadata expressions without executing user code."""

    if expression is None:
        return None
    if isinstance(expression, ast.Constant):
        return _metadata_value(expression)
    if isinstance(expression, ast.Name):
        state = context_import_state(context)
        if expression.id == "__package__":
            return state.package
        if expression.id == "__name__":
            return state.name
    resolved = resolve_string(expression) if resolve_string is not None else None
    return (
        StaticMetadataValue.known(resolved) if resolved is not None else UNKNOWN_VALUE
    )


def dunder_globals_state_from_expression(
    expression: ast.expr | None,
    context: ModuleImportContext,
    resolve_string: Callable[[ast.expr], str | None] | None = None,
) -> ModuleImportState | None:
    """Parse a known globals mapping supplied to builtin ``__import__``."""

    if expression is None:
        return None
    if (
        isinstance(expression, ast.Call)
        and isinstance(expression.func, ast.Name)
        and expression.func.id == "globals"
        and not expression.args
        and not expression.keywords
    ):
        return context_import_state(context)
    if not isinstance(expression, ast.Dict):
        return None
    state = ModuleImportState(ABSENT_VALUE, ABSENT_VALUE, ABSENT_VALUE, False)
    for key, value in zip(expression.keys, expression.values):
        if key is None:
            unpacked = (
                dunder_globals_state_from_expression(value, context, resolve_string)
                if isinstance(value, ast.Dict)
                else None
            )
            if unpacked is None:
                state = _unknown_module_import_state(state)
            else:
                state = ModuleImportState(
                    unpacked.package
                    if unpacked.package.kind != "absent"
                    else state.package,
                    unpacked.spec_parent
                    if unpacked.spec_parent.kind != "absent"
                    else state.spec_parent,
                    unpacked.name if unpacked.name.kind != "absent" else state.name,
                    unpacked.has_path or state.has_path,
                    state.proven_pure_calls & unpacked.proven_pure_calls,
                )
            continue
        if not (isinstance(key, ast.Constant) and isinstance(key.value, str)):
            continue
        if key.value == "__package__":
            state = ModuleImportState(
                metadata_value_from_expression(value, context, resolve_string)
                or UNKNOWN_VALUE,
                state.spec_parent,
                state.name,
                state.has_path,
                state.proven_pure_calls,
            )
        elif key.value == "__spec__":
            state = ModuleImportState(
                state.package,
                parse_module_spec_parent(value),
                state.name,
                state.has_path,
                state.proven_pure_calls,
            )
        elif key.value == "__name__":
            state = ModuleImportState(
                state.package,
                state.spec_parent,
                metadata_value_from_expression(value, context, resolve_string)
                or UNKNOWN_VALUE,
                state.has_path,
                state.proven_pure_calls,
            )
        elif key.value == "__path__":
            state = ModuleImportState(
                state.package,
                state.spec_parent,
                state.name,
                True,
                state.proven_pure_calls,
            )
    return state


def update_module_import_state(
    state: ModuleImportState,
    target: ast.AST,
    value: ast.AST,
) -> ModuleImportState:
    target_name = import_metadata_target_name(target)
    if target_name is None:
        return state
    if target_name == "__package__":
        return replace(state, package=_metadata_value(value))
    if target_name == "__spec__":
        return replace(
            state,
            spec_parent=parse_module_spec_parent(value, state.proven_pure_calls),
        )
    if target_name == "__name__":
        return replace(state, name=_metadata_value(value))
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
        "\0".join(sorted(state.proven_pure_calls)),
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
    proven_calls = set.intersection(*(set(state.proven_pure_calls) for state in states))
    return (
        ModuleImportState(
            join_value("package"),
            join_value("spec_parent"),
            join_value("name"),
            next(iter(path_values)) if len(path_values) == 1 else None,
            frozenset(proven_calls),
        ),
    )


def _normalized_import_context(context: ModuleImportContext) -> ModuleImportContext:
    if context.spec_name is not None or context.state is not None:
        return context
    return replace(context, spec_name=context.module_name)


def _call_receives_module_globals(
    node: ast.Call, expression_facts: Mapping[PythonNodeKey, PythonExpressionFact]
) -> bool:
    """Project evaluated receiver/argument provenance, never builtin spelling."""
    from molt.compiler_analysis.python_binding_facts import PythonNodeKey

    expressions = [*node.args, *(keyword.value for keyword in node.keywords)]
    if isinstance(node.func, ast.Attribute):
        expressions.append(node.func.value)
    return any(
        fact is not None and fact.exposes_module_globals
        for expression in expressions
        for fact in (expression_facts.get(PythonNodeKey.from_node(expression)),)
    )


def _analyze_module_import_flow_uncached(
    tree: ast.AST,
    context: ModuleImportContext,
    *,
    statement_facts: Mapping[PythonNodeKey, PythonStatementFact],
    expression_facts: Mapping[PythonNodeKey, PythonExpressionFact],
    assignment_effects: Mapping[PythonNodeKey, int],
    call_facts: Mapping[PythonNodeKey, PythonCallSiteFact],
) -> ModuleImportFlow:
    """Project metadata through the canonical statement completion partitions."""

    from molt.compiler_analysis.python_binding_facts import (
        PythonCompletion,
        PythonCompletionFlow,
        PythonIdentity,
        PythonNodeKey,
    )
    from molt.compiler_analysis.python_effects_generated import (
        NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
        RAISES,
        UNKNOWN_EFFECTS,
        WRITES_MODULE_METADATA,
    )

    initial = (context_import_state(context),)
    by_node: dict[ImportNodeKey, tuple[ModuleImportState, ...]] = {}
    all_states: set[ModuleImportState] = set(initial)
    deferred_bodies: dict[ImportNodeKey, tuple[Sequence[ast.stmt], bool]] = {}
    pending_bodies: deque[ImportNodeKey] = deque()
    deferred_expressions: dict[ImportNodeKey, ast.expr] = {}
    metadata_mutator_functions: set[str] = set()
    future_annotations = any(
        isinstance(statement, ast.ImportFrom)
        and statement.module == "__future__"
        and any(alias.name == "annotations" for alias in statement.names)
        for statement in getattr(tree, "body", ())
    )

    def record(node: ast.AST, states: tuple[ModuleImportState, ...]) -> None:
        if isinstance(node, (ast.Import, ast.ImportFrom, ast.Call)):
            key = python_node_source_key(node)
            previous = by_node.get(key, ())
            by_node[key] = _merge_states(previous, states)

    def metadata_assignment(
        states: tuple[ModuleImportState, ...], target: ast.AST, value: ast.AST
    ) -> tuple[ModuleImportState, ...]:
        updated = _merge_states(
            update_module_import_state(state, target, value) for state in states
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
        value_fact = expression_facts.get(PythonNodeKey.from_node(value))
        if value_fact is not None and value_fact.exposes_module_globals:
            # Publishing a module mapping into a binding escapes the metadata
            # authority; using that same mapping as an immediate store receiver
            # is handled by the precise assignment facts below.
            current = unknown_states(current)
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
        effects = assignment_effects.get(
            PythonNodeKey.from_node(target), UNKNOWN_EFFECTS
        )
        if effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS or (
            effects & WRITES_MODULE_METADATA
            and not target_writes_metadata(target, direct_metadata_names)
        ):
            return unknown_states(states)
        return states

    def unknown_states(
        states: tuple[ModuleImportState, ...],
    ) -> tuple[ModuleImportState, ...]:
        current = _merge_states(_unknown_module_import_state(state) for state in states)
        all_states.update(current)
        return current

    def bind_proven_import_calls(
        states: tuple[ModuleImportState, ...],
        statement: ast.Import | ast.ImportFrom,
    ) -> tuple[ModuleImportState, ...]:
        rebound_targets = tuple(
            ast.Name(id=alias.asname or alias.name.split(".", 1)[0])
            for alias in statement.names
        )
        states = invalidate_proven_call_bindings(states, rebound_targets)
        calls: set[str] = set()
        if isinstance(statement, ast.ImportFrom):
            if statement.level == 0 and statement.module == "importlib.machinery":
                calls.update(
                    alias.asname or alias.name
                    for alias in statement.names
                    if alias.name == "ModuleSpec"
                )
        else:
            for alias in statement.names:
                if alias.name == "importlib.machinery":
                    calls.add(
                        f"{alias.asname}.ModuleSpec"
                        if alias.asname
                        else "importlib.machinery.ModuleSpec"
                    )
        if not calls:
            return states
        current = _merge_states(
            replace(
                state,
                proven_pure_calls=state.proven_pure_calls | frozenset(calls),
            )
            for state in states
        )
        all_states.update(current)
        return current

    def invalidate_proven_call_bindings(
        states: tuple[ModuleImportState, ...],
        targets: Sequence[ast.AST],
    ) -> tuple[ModuleImportState, ...]:
        rebound_names = {
            name
            for target in targets
            if (name := dotted_expression_name(target)) is not None
        }
        if not rebound_names:
            return states
        current = _merge_states(
            replace(
                state,
                proven_pure_calls=frozenset(
                    call
                    for call in state.proven_pure_calls
                    if not any(
                        call == rebound
                        or call.startswith(rebound + ".")
                        or rebound.startswith(call + ".")
                        for rebound in rebound_names
                    )
                ),
            )
            for state in states
        )
        all_states.update(current)
        return current

    def expression_may_be_metadata_mutator(value: ast.AST) -> bool:
        if isinstance(value, ast.Name):
            return value.id in metadata_mutator_functions
        if isinstance(value, (ast.Tuple, ast.List, ast.Set)):
            return any(expression_may_be_metadata_mutator(item) for item in value.elts)
        if isinstance(value, ast.Dict):
            return any(
                expression_may_be_metadata_mutator(item) for item in value.values
            )
        if isinstance(value, ast.IfExp):
            return expression_may_be_metadata_mutator(
                value.body
            ) or expression_may_be_metadata_mutator(value.orelse)
        if isinstance(value, ast.NamedExpr):
            return expression_may_be_metadata_mutator(value.value)
        if isinstance(value, ast.Subscript):
            return expression_may_be_metadata_mutator(value.value)
        return False

    def update_mutator_bindings(
        targets: Sequence[ast.AST],
        value: ast.AST,
    ) -> None:
        target_names = {target.id for target in targets if isinstance(target, ast.Name)}
        source_is_mutator = expression_may_be_metadata_mutator(value)
        for name in target_names:
            metadata_mutator_functions.discard(name)
            if source_is_mutator:
                metadata_mutator_functions.add(name)

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

    def function_mutates_metadata(
        statement: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        relevant_globals = scope_global_metadata_names(statement.body)
        pending: list[ast.AST] = list(statement.body)
        while pending:
            child = pending.pop()
            if isinstance(
                child,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            if isinstance(child, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
                targets = (
                    child.targets if isinstance(child, ast.Assign) else (child.target,)
                )
                if any(
                    import_metadata_target_name(target) in _IMPORT_METADATA_NAMES
                    and (
                        not isinstance(target, ast.Name)
                        or target.id in relevant_globals
                    )
                    for target in targets
                ):
                    return True
            elif isinstance(child, ast.Delete) and any(
                import_metadata_target_name(target) in _IMPORT_METADATA_NAMES
                and (not isinstance(target, ast.Name) or target.id in relevant_globals)
                for target in child.targets
            ):
                return True
            elif isinstance(child, ast.Call):
                if isinstance(child.func, ast.Name) and child.func.id in {
                    "exec",
                    "eval",
                }:
                    return True
                if (
                    isinstance(child.func, ast.Name)
                    and child.func.id == "setattr"
                    and len(child.args) >= 2
                    and isinstance(child.args[1], ast.Constant)
                    and child.args[1].value in _IMPORT_METADATA_NAMES
                ):
                    return True
            pending.extend(ast.iter_child_nodes(child))
        return False

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
        if raised_states is not None and (
            fact is None or (fact.effects | fact.truth_effects) & RAISES
        ):
            raised_states.append(_merge_states(states, current))
        return current

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
        for child in expression_evaluation_children(expression):
            current = expression_effects(
                child,
                current,
                direct_metadata_names=direct_metadata_names,
                raised_states=raised_states,
            )
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
            return invalidate_proven_call_bindings(current, (expression.target,))
        call = call_facts.get(PythonNodeKey.from_node(expression))
        if (
            isinstance(expression, ast.Call)
            and call is not None
            and (
                call.callee_is(PythonIdentity.GLOBALS_SETITEM)
                or call.callee_is(PythonIdentity.GLOBALS_DELITEM)
            )
        ):
            # Binding flow owns method identity, argument admission and old-value
            # release. A spelled globals()/__setitem__ is not mutation authority.
            effects = assignment_effects.get(
                PythonNodeKey.from_node(expression), UNKNOWN_EFFECTS
            )
            if effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS:
                return unknown_states(current)
            key_fact = expression_facts.get(PythonNodeKey.from_node(expression.args[0]))
            name = None if key_fact is None else key_fact.static_value
            if not isinstance(name, str):
                return (
                    unknown_states(current)
                    if effects & WRITES_MODULE_METADATA
                    else current
                )
            if name not in _IMPORT_METADATA_NAMES:
                return current
            target = ast.Name(id=name)
            if call.callee_is(PythonIdentity.GLOBALS_DELITEM):
                return _merge_states(
                    invalidate_module_import_state(state, target, deleted=True)
                    for state in current
                )
            return metadata_assignment(current, target, expression.args[1])
        if isinstance(expression, ast.Call) and (
            isinstance(expression.func, ast.Name)
            and expression.func.id in {"exec", "eval"}
            or expression_may_be_metadata_mutator(expression.func)
        ):
            return unknown_states(current)
        if isinstance(expression, ast.Call) and _call_receives_module_globals(
            expression, expression_facts
        ):
            return unknown_states(current)
        if isinstance(expression, ast.Call):
            if (
                isinstance(expression.func, ast.Name)
                and expression.func.id == "setattr"
                and len(expression.args) >= 3
                and isinstance(expression.args[1], ast.Constant)
                and isinstance(expression.args[1].value, str)
            ):
                attribute_name = expression.args[1].value
                if attribute_name in _IMPORT_METADATA_NAMES:
                    # setattr may invoke a descriptor or release an old value.
                    # It cannot publish a precise caller-module anchor merely
                    # because the supplied member has a metadata spelling.
                    return unknown_states(current)
                owner_name = dotted_expression_name(expression.args[0])
                rebound = (
                    f"{owner_name}.{attribute_name}" if owner_name is not None else None
                )
                if rebound is not None:
                    synthetic_target = ast.parse(rebound, mode="eval").body
                    current = invalidate_proven_call_bindings(
                        current, (synthetic_target,)
                    )
            pure_call_name = dotted_expression_name(expression.func)
            if any(
                pure_call_name in state.proven_pure_calls
                and expression_may_execute_python(
                    expression,
                    proven_pure_calls=state.proven_pure_calls,
                )
                for state in current
            ):
                return unknown_states(current)
        # Executing Python is not by itself authority to poison the caller's
        # module globals. The explicit exec/globals/local-mutator paths above
        # are the mutation boundaries; ordinary callees own a different global
        # mapping. Proven-pure calls remain useful to ModuleSpec parsing.
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
                current = bind_proven_import_calls(current, statement)
            elif isinstance(statement, ast.Assign):
                current = evaluate(statement.value, current)
                current = assign_states(
                    current,
                    statement.targets,
                    statement.value,
                    direct_metadata_names=direct_metadata_names,
                    raised_states=pending_expression_raises,
                )
                current = invalidate_proven_call_bindings(current, statement.targets)
                update_mutator_bindings(statement.targets, statement.value)
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
                    current = invalidate_proven_call_bindings(
                        current, (statement.target,)
                    )
                    update_mutator_bindings((statement.target,), statement.value)
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
                current = invalidate_proven_call_bindings(current, statement.targets)
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
                            unknown_states(header)
                            if iteration is None
                            or iteration.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
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
                        lambda state: PythonCompletionFlow(normal=unknown_states(state))
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
                current = invalidate_proven_call_bindings(
                    current, (ast.Name(id=statement.name),)
                )
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
                mutates_metadata = function_mutates_metadata(statement)
                deferred_key = python_node_source_key(statement)
                if deferred_key not in deferred_bodies:
                    pending_bodies.append(deferred_key)
                deferred_bodies[deferred_key] = (statement.body, mutates_metadata)
                if mutates_metadata:
                    metadata_mutator_functions.add(statement.name)
                if any(
                    dotted_expression_name(decorator) in metadata_mutator_functions
                    for decorator in statement.decorator_list
                ):
                    current = unknown_states(current)
            elif isinstance(statement, ast.ClassDef):
                current = invalidate_proven_call_bindings(
                    current, (ast.Name(id=statement.name),)
                )
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
                if any(
                    dotted_expression_name(decorator) in metadata_mutator_functions
                    for decorator in statement.decorator_list
                ):
                    outcome = outcome.sequence(
                        lambda state: PythonCompletionFlow(
                            normal=unknown_states(state)
                        ),
                        join_states=_merge_states,
                    )
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
                    entered = unknown_states(evaluated)
                    prefix = PythonCompletionFlow(
                        raised=_merge_states(incoming, entered, *raised),
                    )
                    # Assignment and nested acquisition occur inside this manager;
                    # their failures may be suppressed by its exit callback.
                    assigned = PythonCompletionFlow(normal=entered)
                    if item.optional_vars is not None:
                        assigned = PythonCompletionFlow(normal=entered, raised=entered)
                    body = assigned.sequence(
                        lambda normal: enter_context(index + 1, normal),
                        join_states=_merge_states,
                    )
                    return prefix.merge(
                        body.unwind_context(
                            lambda state: PythonCompletionFlow(
                                normal=unknown_states(state),
                                raised=unknown_states(state),
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
                            changed = unknown_states(incoming)
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
        deferred_body, mutates_metadata = deferred_bodies[body_key]
        body_states = (
            unknown_states(deferred_states) if mutates_metadata else deferred_states
        )
        flow_statements(
            deferred_body,
            body_states,
            eager_annotations=False,
            direct_metadata_names=scope_global_metadata_names(deferred_body),
        )
    for deferred_expression in deferred_expressions.values():
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
