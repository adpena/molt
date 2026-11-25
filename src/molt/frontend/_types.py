"""Leaf module: shared frontend data types, constants, and lookup tables.

Extracted from frontend/__init__.py (F1 decomposition, move-only). This is the
bottom of the frontend package import graph: both __init__.py (the
SimpleTIRGenerator assembly) and every visitor/lowering mixin import from here.
It must never import from molt.frontend.__init__ or any mixin (cycle break).
"""

from __future__ import annotations

from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity
from molt.compiler_analysis.python_source_keys import PythonSourceKey

from molt.frontend.lowering.op_kinds_generated import (
    ExceptionEdgeKind,
    FrontendOpKind,
    WireOpKind,
)

import ast
from dataclasses import dataclass, field
from enum import StrEnum
from typing import (
    TYPE_CHECKING,
    Any,
    Literal,
    NotRequired,
    TypedDict,
)

from molt import builtin_callables
from molt._intrinsic_symbols import INTRINSIC_SIGNATURES
from molt._wasm_abi_generated import wasm_runtime_callable_arity
from molt.compat import CompatibilityError, CompatibilityReporter
from molt.frontend.cfg_analysis import CFGGraph, ControlMaps, build_cfg
from molt.frontend.exception_kinds_generated import BUILTIN_EXCEPTION_NAMES
from molt.frontend.module_publication import SourceModulePublication
from molt.type_facts import normalize_type_hint

_LOADED_METHOD_SELF_NAME_KINDS_1: frozenset[FrontendOpKind] = frozenset(
    {
        "CALLARGS_PUSH_METHOD_RECEIVER",
        "LOAD_METHOD",
        "CALL_LOADED_METHOD",
        "LOAD_SUPER_METHOD",
    }
)


if TYPE_CHECKING:
    from molt.frontend.class_model import ClassDraft, ClassView
    from molt.compiler_analysis.python_binding_facts import PythonBindingFacts

    from molt.frontend.sema.funcmeta import (
        StatefulFunctionFramePlan,
        StatefulLocalsLayout,
    )


# One import statement's form (``import_name``, bedrock §13 D2): ``dunder`` is
# ``__import__(name, globals, None, fromlist, level)``, ``module`` is
# ``importlib.import_module(name)``, ``root`` is a compiler root (M9).
ImportForm = Literal["dunder", "module", "root"]


class SimpleIROp(TypedDict):
    class_frame: NotRequired[str]
    exception_edge: NotRequired[ExceptionEdgeKind]
    exception_source: NotRequired[ExceptionEdgeKind]
    """Serialized operation: closed identity, existing operation-specific payload."""

    kind: WireOpKind
    args: NotRequired[Any]
    argument_custody: NotRequired[Any]
    async_work_poll: NotRequired[Any]
    bound_local: NotRequired[Any]
    builtin_name: NotRequired[Any]
    bytes: NotRequired[Any]
    class_definition: NotRequired[Any]
    stable_instance_class: NotRequired[bool]
    # Wire array containing the class statement identity and method name.
    exact_method: NotRequired[list[str]]
    exact_field_slots: NotRequired[list[list[object]]]
    col_offset: NotRequired[Any]
    container_type: NotRequired[Any]
    defines_del: NotRequired[Any]
    end_col_offset: NotRequired[Any]
    f_value: NotRequired[Any]
    # ``import_name`` only; backend_ir lowers it before the Rust boundary.
    fromlist: NotRequired[list[str]]
    import_form: NotRequired[ImportForm]
    global_slot: NotRequired[Any]
    method_site: NotRequired[Any]
    module_attribute: NotRequired[str]
    # A def with positional parameters only on ``func_new``: synchronous, or a
    # generator without a closure (OpIR::direct_shape, HF-846).
    direct_shape: NotRequired[bool]
    # A ``call_guarded`` whose callee the program proves (HF-846 slice 2;
    # a resolved method of an exact receiver, design 78 3.8).
    proven_target: NotRequired[bool]
    # An ``object_new_bound`` whose class layout proves its instance plan
    # (``class_model.release_plan``): the field word count (OpIR::release_plan).
    release_plan: NotRequired[int]
    native_callable_abi: NotRequired[Any]
    native_callable_binding: NotRequired[Any]
    native_callable_export: NotRequired[Any]
    native_callable_symbol: NotRequired[Any]
    out: NotRequired[Any]
    passes_execution_context: NotRequired[Any]
    runtime_candidate: NotRequired[str]
    runtime_requirement_bits: NotRequired[Any]
    runtime_symbol: NotRequired[Any]
    s_value: NotRequired[Any]
    source_line: NotRequired[Any]
    source_end_line: NotRequired[Any]
    source_module_publication_boundary: NotRequired[Any]
    arena_eligible: NotRequired[Any]
    state_targets: NotRequired[Any]
    task_closure_size: NotRequired[Any]
    task_vararg_slot: NotRequired[int]
    task_kind: NotRequired[Any]
    type_hint: NotRequired[Any]
    value: NotRequired[Any]
    var: NotRequired[Any]


@dataclass(frozen=True, slots=True)
class CallableId:
    """One source callable definition in its owning module."""

    module: str
    definition: PythonSourceKey


@dataclass(frozen=True, slots=True)
class GeneratedCallableId:
    """A generated definition within its emitted enclosing function."""

    owner_symbol: str
    definition: PythonSourceKey


@dataclass(frozen=True, slots=True)
class GeneratedCallableReservation:
    definition: PythonSourceKey
    symbol: str


@dataclass(frozen=True, slots=True)
class ClassId:
    """One class statement, program-wide (HF-203).

    ``qualname`` is the statement's Python ``__qualname__``. ``occurrence`` is
    the statement's ``(line, column)`` only when its qualname repeats in its
    module, so a class defined once keeps a position-free identity that
    survives edits elsewhere in the file. Identity is the whole value: two
    statements that share a name never compare equal.
    """

    module: str
    qualname: str
    occurrence: tuple[int, int] | None = None

    @property
    def name(self) -> str:
        """The Python-visible ``__name__`` (the last qualname component)."""
        return self.qualname.rpartition(".")[2]

    @property
    def definition_symbol(self) -> str:
        """The statement's program slot symbol (HF-220).

        The statement stamps it on every class it creates and layout guards
        compare receivers against it; the program binds it to a dense slot.
        Module names and qualnames never contain ``:`` or ``@``, and code-owner
        symbols never contain ``:``, so the spelling is injective and never
        names compiled code.
        """
        symbol = f"class:{self.module}:{self.qualname}"
        if self.occurrence is None:
            return symbol
        line, column = self.occurrence
        return f"{symbol}@{line}:{column}"

    def sort_key(self) -> tuple[str, str, int, int, int]:
        """Total, deterministic order; a unique qualname sorts first."""
        if self.occurrence is None:
            return (self.module, self.qualname, 0, 0, 0)
        line, column = self.occurrence
        return (self.module, self.qualname, 1, line, column)

    @property
    def body_code_symbol(self) -> str:
        return f"class_body:{self.definition_symbol}"

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, ClassId):
            return NotImplemented
        return self.sort_key() < other.sort_key()

    def __le__(self, other: object) -> bool:
        if not isinstance(other, ClassId):
            return NotImplemented
        return self.sort_key() <= other.sort_key()

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, ClassId):
            return NotImplemented
        return self.sort_key() > other.sort_key()

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, ClassId):
            return NotImplemented
        return self.sort_key() >= other.sort_key()


@dataclass(frozen=True, slots=True)
class BuiltinClass:
    """A builtin class such as ``object``, ``dict`` or ``Exception``.

    It never compares equal to a :class:`ClassId`, even when a user class has
    the same name (``class Exception(...)`` in user code).
    """

    name: str

    def __post_init__(self) -> None:
        if self.name not in BUILTIN_TYPE_TAGS and self.name not in (
            BUILTIN_EXCEPTION_NAMES
        ):
            raise ValueError(f"{self.name!r} is not a builtin class")


type ClassRef = ClassId | BuiltinClass


class ReceiverKind(StrEnum):
    """What a declared class value is: an instance, or the class object itself."""

    INSTANCE = "instance"
    CLASS_OBJECT = "class_object"


@dataclass(frozen=True, slots=True)
class DeclaredClass:
    """A declared class: an annotation, a return hint or a method receiver.

    Subclasses stay possible and the runtime does not enforce it, so it never
    authorizes an unguarded fixed layout; only a runtime layout guard or an
    exact :class:`ClassId` fact does.
    """

    class_id: ClassId
    kind: ReceiverKind = ReceiverKind.INSTANCE


@dataclass(frozen=True, slots=True)
class BoundMethodHint:
    """A bound method whose owner class and method name the producer proved."""

    owner: ClassRef
    method: str


# A value hint: a builtin or transport spelling, a declared class, or a bound
# method. User class identity never travels as a string.
type TypeHint = str | DeclaredClass | BoundMethodHint


@dataclass
class MoltValue:
    name: str
    type_hint: TypeHint = "Unknown"
    # Concrete runtime class identity proven by the operation that produced
    # this value.  Unlike ``type_hint``, this may authorize fixed-layout access.
    exact_class: ExactFact | None = None
    # Exact identity is a temporal fact.  A callback or other invalidating
    # boundary advances the generator token, making already-held values stale.
    exact_class_token: int | None = None
    # The stamp came from a durable fact (``ExactClassFact.durable``): only a
    # field access may consume it.
    exact_class_durable: bool = False
    # A value read from an activation slot, without an independent reference.
    # Transparent expression joins preserve this storage fact; heap-producing
    # operations and explicit BINDING_ALIAS captures own their result instead.
    borrows_binding: bool = False


@dataclass(frozen=True, slots=True)
class BuiltinLayout:
    """A value's proven builtin layout (`ExceptionLayoutKind` name): a builtin
    tag construction produced it. It fixes typed member slots, not the exact
    class (errno promotion may pick an OSError subclass), and no `__class__`
    store can change a builtin exception's layout (call_anatomy.md 4.5a,
    "Builtin layouts")."""

    kind: str


# A lowered value's exact-identity fact: a user class, or a builtin layout.
type ExactFact = ClassId | BuiltinLayout


@dataclass(frozen=True)
class ExactClassFact:
    class_id: ExactFact
    token: int
    # The fact survived a callback (no ``__class__`` writer in the closed
    # graph): it proves the class, which fixes field offsets. A callback may
    # have stored an instance attribute that shadows a class attribute or
    # method, so a member consumer also needs a namespace-closed class: the
    # source demands expose a class whose instances get a shadowing store or
    # reach code through their mapping (design 78, 5.6). Field access and
    # direct calls of resolved methods consume it.
    durable: bool = False


@dataclass(frozen=True, slots=True)
class CodeSlotDeclaration:
    """Immutable projection of a code object's public local/cell/free tables.

    Every local keeps its index, including PEP 709 locals that also occur in
    ``cellvars``. Only cells absent from ``varnames`` need another slot. Free
    variables follow these slots. A logical cell does not imply that every
    source point physically stores a cell: an inlined comprehension saves and
    restores its enclosing binding. Storage publication carries that fact.
    """

    parameters: tuple[str, ...]
    varnames: tuple[str, ...]
    cellvars: tuple[str, ...]
    freevars: tuple[str, ...]
    _slots: tuple[str, ...] = field(init=False, repr=False)

    def __post_init__(self) -> None:
        if self.varnames[: len(self.parameters)] != self.parameters:
            raise ValueError("code slot parameters must prefix co_varnames")
        locals_ = set(self.varnames)
        object.__setattr__(
            self,
            "_slots",
            self.varnames
            + tuple(name for name in self.cellvars if name not in locals_)
            + self.freevars,
        )

    def slots(self) -> tuple[str, ...]:
        return self._slots

    def release_order(self, target_python: tuple[int, int]) -> range:
        """CPython clears upwards through 3.13, downwards from 3.14."""
        count = len(self._slots)
        return range(count - 1, -1, -1) if target_python >= (3, 14) else range(count)

    def frame_entry_value(self, target_python: tuple[int, int]) -> int:
        """The ``trace_enter_slot`` value of a synchronous frame of this code:
        its home count in bits 0..32 and, in bit 32, whether it clears its
        slots last to first (molt-codegen-abi ``FrameShape``)."""
        descending = self.release_order(target_python).step < 0
        return len(self._slots) | (1 << 32 if descending else 0)


# The ``trace_enter_slot`` value of a stateful activation's entry (a generator,
# coroutine or async-generator poll), which links its task's persistent frame
# record (molt-codegen-abi ``FRAME_ENTRY_STATEFUL``).
FRAME_ENTRY_STATEFUL = 1 << 33


class AsyncFrameSlotRole(StrEnum):
    PUBLIC = "public"
    INTERNAL = "internal"
    SCRATCH = "scratch"


@dataclass(frozen=True)
class AsyncFrameSlot:
    """One typed slot from the canonical stateful-function frame allocator."""

    offset: int
    role: AsyncFrameSlotRole
    public_name: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.role, AsyncFrameSlotRole):
            raise ValueError("async frame slot role is invalid")
        if self.offset < 0 or self.offset % 8 != 0:
            raise ValueError("async frame slot offset must be nonnegative and aligned")
        if (self.role is AsyncFrameSlotRole.PUBLIC) != (self.public_name is not None):
            raise ValueError("only public async frame slots may carry a name")


@dataclass(frozen=True)
class CompilerLocalHome:
    """Promotable compiler transport, separate from Python binding storage."""

    name: str
    type_hint: TypeHint


@dataclass(frozen=True)
class ScratchCell:
    """Opaque compiler-temporary storage, never a Python frame binding."""

    value: MoltValue | None
    async_slot: AsyncFrameSlot | None
    type_hint: TypeHint

    def __post_init__(self) -> None:
        if (self.value is None) == (self.async_slot is None):
            raise ValueError(
                "scratch cell must have exactly one sync value or async slot"
            )
        if (
            self.async_slot is not None
            and self.async_slot.role is not AsyncFrameSlotRole.SCRATCH
        ):
            raise ValueError("async scratch cell requires a scratch-role slot")


@dataclass
class ComprehensionBinding:
    """One scoped Python binding with SSA or stateful-frame transport.

    Captured iteration variables store a real closure-cell object in that same
    slot. The public name never aliases the surrounding frame's storage.
    """

    variable_slot: str | None
    async_slot: AsyncFrameSlot | None
    is_cell: bool = False
    type_hint: TypeHint = "Any"
    definitely_bound: bool = False

    def __post_init__(self) -> None:
        if (self.variable_slot is None) == (self.async_slot is None):
            raise ValueError("comprehension binding requires exactly one slot")
        if (
            self.async_slot is not None
            and self.async_slot.role is not AsyncFrameSlotRole.SCRATCH
        ):
            raise ValueError("comprehension transport requires a scratch-role slot")


@dataclass
class MoltOp:
    kind: FrontendOpKind
    args: list[Any]
    result: MoltValue
    metadata: dict[str, Any] | None = None
    col_offset: int | None = None
    end_col_offset: int | None = None
    source_line: int | None = None
    source_end_line: int | None = None
    exception_edge: ExceptionEdgeKind | None = None
    exception_source: ExceptionEdgeKind | None = None
    class_frame: ClassId | None = None


def loaded_method_self_name(op: MoltOp) -> str:
    """The wire name of a method pair's `self_or_null` word.

    A method load (CPython's LOAD_ATTR or LOAD_SUPER_ATTR in method form) is one
    frontend value, its callable; the second result exists only on the wire.
    The load and its CALL_LOADED_METHOD or CALLARGS_PUSH_METHOD_RECEIVER consumer
    both record it. The nullable word never becomes a frontend object value.
    """
    if op.kind not in _LOADED_METHOD_SELF_NAME_KINDS_1:
        raise AssertionError(f"{op.kind} carries no method pair")
    name = (op.metadata or {}).get("self_or_null")
    if not isinstance(name, str) or not name:
        raise AssertionError(f"{op.kind} {op.result.name} names no self_or_null")
    return name


@dataclass(frozen=True)
class TypeParameterBinding:
    """One current-frame declaration value and its planned shared lexical cell."""

    value: MoltValue
    cell: MoltValue | None


@dataclass(frozen=True)
class FunctionDefaults:
    """Both definition-time values and their parameter shape."""

    positional: MoltValue
    keyword: MoltValue
    positional_count: int
    keyword_names: tuple[str, ...]


@dataclass(frozen=True)
class FunctionDefinitionValues:
    """Definition inputs evaluated in Python order before the body closure."""

    decorators: tuple[MoltValue, ...]
    defaults: FunctionDefaults
    type_params: tuple[MoltValue, ...]
    type_parameter_bindings: dict[str, TypeParameterBinding]


@dataclass
class _ClassNsScope:
    """Active class-body namespace while the body is lowered as a block (P0 #50).

    CPython executes a ``class`` body as a code object whose ``f_locals`` is the
    (possibly custom) class namespace mapping: ``STORE_NAME`` writes ``ns[k]=v``,
    ``LOAD_NAME`` reads ``ns[k]`` (falling through to globals/builtins on
    KeyError), ``DELETE_NAME`` does ``del ns[k]``.  Because that mapping is the
    single mutable home for every class-body name, arbitrary control flow
    (for/if/while/try/with) and ``del`` work with no per-node special-casing.

    Molt mirrors this: when ``ns`` is set (the class is built dynamically), each
    class-body name store emits ``STORE_INDEX(ns, name, value)`` and each load
    probes ``molt_namespace_get(ns, name, missing)`` before lexical/global
    fallback. The mapping is loop-carried-correct without namespace SSA phis.
    ``attr_values`` additionally snapshots name->MoltValue for the static
    ``CLASS_DEF`` fast path (straight-line bodies that never need the dict).
    ``names`` tracks published attributes; immutable declaration sets retain
    lexical ownership even after deletion. Never-stored names still probe the
    live mapping, which may have been populated by __prepare__ or a callback.

    The instance is pushed/popped on ``SimpleTIRGenerator._class_ns_stack`` by
    ``visit_ClassDef`` and consulted by ``_store_local_value`` /
    ``_load_local_value`` / ``_emit_delete_name``.
    """

    ns: "MoltValue | None"
    attr_values: dict[str, MoltValue]
    names: set[str]
    class_name: str
    module_name: str
    local_names: frozenset[str] = frozenset()
    global_names: frozenset[str] = frozenset()
    nonlocal_names: frozenset[str] = frozenset()
    enclosing_locals: dict[str, MoltValue] = field(default_factory=dict)
    annotation_namespace_cell: MoltValue | None = None
    # Whether deferred annotation evaluators need the class body's mapping
    # (``class_annotations_need_namespace``).
    annotation_namespace_required: bool = False
    class_node: ast.ClassDef | None = None
    class_cell: MoltValue | None = None
    # The statement's class draft: the body's bindings run through it in
    # source order (``molt.frontend.class_model``).
    draft: ClassDraft | None = None


@dataclass(frozen=True, slots=True)
class MethodValues:
    """A compiled method's SSA values in its defining module: the function
    object and the value the class binds (after decorators). Lowering-only:
    never published, never crosses a module."""

    func: MoltValue
    attr: MoltValue


# 47-bit signed inline integer range for NaN-boxing.
_INLINE_INT_MIN = -(1 << 46)
_INLINE_INT_MAX = (1 << 46) - 1


@dataclass
class ActiveException:
    value: MoltValue
    scope: TryScope
    handler_name: str | None = None
    is_handler: bool = False
    # Number of `try` blocks active at the point this handler's body begins
    # executing (``len(self.try_end_labels)``).  A `raise` inside the handler
    # body only *exits* this handler — and so only triggers its implicit
    # ``del NAME`` — when it is not caught by a nested `try` opened after the
    # handler started, i.e. when the raise propagates from the same try-nesting
    # depth recorded here.  A larger live depth means an inner `try` protects
    # the raise and the handler is not left.
    handler_try_depth: int = 0


# The generator control region's edge slots that compiled bodies read and
# write (molt-runtime-constants owns the layout; its raw state and exception
# depth words are runtime-only).
GEN_SEND_OFFSET = 0
GEN_THROW_OFFSET = 8
GEN_YIELD_FROM_OFFSET = 32
GEN_CONTROL_SIZE = 48
# The resume label a generator body stores at its normal return: the poll
# returns the value itself, and this label reports completion
# (molt-codegen-abi GENERATOR_RESUME_RETURNED).
GEN_RESUME_RETURNED = -(2**63)

BUILTIN_TYPE_TAGS = {
    "int": 1,
    "float": 2,
    "bool": 3,
    "str": 5,
    "bytes": 6,
    "bytearray": 7,
    "list": 8,
    "tuple": 9,
    "dict": 10,
    "set": 17,
    "frozenset": 18,
    "range": 11,
    "slice": 12,
    "memoryview": 15,
    "object": 100,
    "type": 101,
    "classmethod": 226,
    "staticmethod": 227,
    "property": 228,
    # CPython: `super` is a builtin type. Model it as a builtin type tag so that
    # `builtins.super` is a `type` object (not a function), and indirect calls like
    # `alias = builtins.super; alias()` match CPython (raising when no `__class__`
    # cell is present).
    "super": 229,
    "BaseException": 102,
    "Exception": 103,
}

BUILTIN_LAYOUT_MIN = {
    "int": 16,
    "bool": 16,
    "dict": 16,
}

# Method names that are implicitly classmethods (CPython treats them as
# classmethod-like even without an explicit @classmethod decorator).  Inside
# these methods, the first parameter (`cls`) is the class itself, not an
# instance, so attribute assignments through that name must NOT be collected
# as instance fields or as `__static_attributes__` entries.
from molt.compiler_analysis.python_function_kind import (  # noqa: E402
    IMPLICIT_CLASSMETHOD_NAMES as IMPLICIT_CLASSMETHOD_NAMES,
    IMPLICIT_STATICMETHOD_NAMES as IMPLICIT_STATICMETHOD_NAMES,
)


def _function_is_instance_method(
    item: ast.AST, bindings: PythonBindingFacts | None
) -> bool:
    """Return True iff `item` is a regular instance method.

    Excludes `@classmethod`, `@staticmethod`, and the implicit-classmethod /
    implicit-staticmethod names (`__new__`, `__init_subclass__`,
    `__class_getitem__`).  Only instance methods may legitimately collect
    instance-field names from `self.X = ...` assignments — using `cls`
    inside a classmethod must not feed the instance-layout machinery.
    """
    if not isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return False
    if item.name in IMPLICIT_CLASSMETHOD_NAMES:
        return False
    if item.name in IMPLICIT_STATICMETHOD_NAMES:
        return False
    for deco in item.decorator_list:
        if bindings is None or not bindings.expression_is(
            deco, PythonIdentity.BUILTIN_PROPERTY
        ):
            # Unresolved decorators can replace the method or its receiver
            # protocol, so they cannot prove an instance field layout.
            return False
    return True


# Attribute loads on these exact builtin receivers carry a BoundMethodHint.
_BUILTIN_FAST_METHODS: dict[str, frozenset[str]] = {
    "str": frozenset({"upper", "lower", "strip", "startswith", "join"}),
    "list": frozenset({"append"}),
    "dict": frozenset({"get"}),
}


OBJECT_CLASS = BuiltinClass("object")

_MOLT_CLOSURE_PARAM = "__molt_closure__"
_MOLT_MODULE_CHUNK_PARAM = "__molt_module_obj__"
# Native code reads a chunk's role from `__molt_module_chunk_` in its symbol
# (`molt_ir::ir::is_module_chunk_function`).
_MOLT_MODULE_CHUNK_PREFIX = "__molt_module_chunk"


# Intrinsic declarations and builtin specifications are separate closed domains.
# Index the declarative records without validating a generated projection
# during import; executable frontend consumers validate it at the query boundary.
_BUILTIN_FUNC_SPECS_BY_RUNTIME = {
    spec.runtime: spec for spec in builtin_callables.BUILTIN_FUNC_SPECS.values()
}


def _builtin_func_abi_arity(spec: builtin_callables.BuiltinFuncSpec) -> int:
    """Return the manifest-backed ABI arity for a frontend builtin callable."""
    declared_arity = builtin_callables.builtin_func_declared_arity(spec)
    manifest_arity = wasm_runtime_callable_arity(spec.runtime)
    if manifest_arity is None:
        raise RuntimeError(
            f"frontend builtin {spec.runtime} missing from WASM callable ABI manifest"
        )
    if manifest_arity != declared_arity:
        raise RuntimeError(
            f"frontend builtin {spec.runtime} arity drift: "
            f"manifest {manifest_arity} vs frontend metadata {declared_arity}"
        )
    return manifest_arity


def _intrinsic_signature(runtime_name: str) -> tuple[str, int] | None:
    """Resolve a declared intrinsic or frontend builtin without source discovery."""
    signature = INTRINSIC_SIGNATURES.get(runtime_name)
    if signature is not None:
        return signature
    builtin = _BUILTIN_FUNC_SPECS_BY_RUNTIME.get(runtime_name)
    if builtin is None:
        return None
    return runtime_name, _builtin_func_abi_arity(builtin)


def _intrinsic_arity_exact(runtime_name: str) -> int | None:
    """Return total ABI argument slots; unknown names remain distinct from zero."""
    signature = _intrinsic_signature(runtime_name)
    return None if signature is None else signature[1]


@dataclass(frozen=True)
class SyncContextExit:
    """Runtime-owned manager consumed by the common context cleanup path."""

    manager: ScratchCell


@dataclass(frozen=True)
class AsyncContextExit:
    """Captured special method retained across entry, body and exit suspension."""

    callback: ScratchCell


@dataclass
class LoopScope:
    """Lexical transfer destinations independent of inlined cleanup position."""

    break_label: int
    continue_label: int
    try_depth: int
    break_flag: ScratchCell | None = None
    break_used: bool = False
    continue_used: bool = False
    body_terminated: bool = False

    @property
    def needs_latch(self) -> bool:
        return not self.body_terminated or self.continue_used


@dataclass
class TryScope:
    finalbody: list[ast.stmt] | None
    done_label: int | None = None
    handler_label: int | None = None
    context_exit: SyncContextExit | AsyncContextExit | None = None
    finalbody_running: bool = False
    lexical_loops: tuple[LoopScope, ...] = ()


type MethodDescriptor = Literal[
    "function",
    "classmethod",
    "staticmethod",
    "property",
    "decorated",
    "property_update",
]


class MethodInfo(TypedDict):
    func: MoltValue
    attr: MoltValue
    descriptor: MethodDescriptor
    param_count: int
    defaults: list[dict[str, Any]]
    posonly_count: int
    kwonly_count: int
    has_vararg: bool
    has_varkw: bool
    has_closure: bool
    property_field: str | None
    property_update: Literal["setter", "deleter"] | None
    inline_return: NotRequired[ast.expr | None]
    inline_params: NotRequired[list[str] | None]
    inline_init_assigns: NotRequired[list[tuple[str, ast.expr]] | None]
    # Every return of the synchronous body yields None (no value, no yield).
    returns_none: NotRequired[bool]


def layout_definition_symbol(class_id: ClassId, view: ClassView) -> str | None:
    """The class statement identity a layout guard may trust, or None (HF-220).

    Only a statement with a static layout publishes its field offsets with
    ``CLASS_DEF`` and stamps the class it creates. A dynamic class (a
    metaclass call, several bases, ``__slots__``, attribute hooks) has no
    offsets to trust, and a metaclass may return a class that another
    statement created. Every module applies this one rule: the defining
    module stamps exactly the statements a guard in any module may name.
    """
    return class_id.definition_symbol if view.layout.static else None


class FuncInfo(TypedDict):
    params: list[str]
    param_types: list[str]  # type hints from annotations ("int", "float", "Any", ...)
    return_abi: Literal["void", "value"]
    return_hint: str | None
    ops: list[MoltOp]
    frame_entry_failure_label: NotRequired[int]
    stateful_frame_plan: NotRequired[StatefulFunctionFramePlan]
    stateful_locals_layout: NotRequired[StatefulLocalsLayout]
    source_module_publication: NotRequired[SourceModulePublication]
    gpu_body_origin: NotRequired[dict[str, Any]]


__all__ = [
    "MoltValue",
    "MoltOp",
    "_INLINE_INT_MIN",
    "_INLINE_INT_MAX",
    "ActiveException",
    "GEN_SEND_OFFSET",
    "GEN_THROW_OFFSET",
    "GEN_RESUME_RETURNED",
    "FRAME_ENTRY_STATEFUL",
    "GEN_YIELD_FROM_OFFSET",
    "GEN_CONTROL_SIZE",
    "BUILTIN_TYPE_TAGS",
    "BUILTIN_LAYOUT_MIN",
    "IMPLICIT_CLASSMETHOD_NAMES",
    "IMPLICIT_STATICMETHOD_NAMES",
    "_function_is_instance_method",
    "_BUILTIN_FAST_METHODS",
    "BUILTIN_EXCEPTION_NAMES",
    "_MOLT_CLOSURE_PARAM",
    "_MOLT_MODULE_CHUNK_PARAM",
    "_MOLT_MODULE_CHUNK_PREFIX",
    "_builtin_func_abi_arity",
    "_intrinsic_signature",
    "_intrinsic_arity_exact",
    "TryScope",
    "MethodInfo",
    "layout_definition_symbol",
    "FuncInfo",
    "CompatibilityError",
    "CompatibilityReporter",
    "CFGGraph",
    "ControlMaps",
    "build_cfg",
    "normalize_type_hint",
]
