"""Leaf module: shared frontend data types, constants, and lookup tables.

Extracted from frontend/__init__.py (F1 decomposition, move-only). This is the
bottom of the frontend package import graph: both __init__.py (the
SimpleTIRGenerator assembly) and every visitor/lowering mixin import from here.
It must never import from molt.frontend.__init__ or any mixin (cycle break).
"""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from typing import (
    TYPE_CHECKING,
    Any,
    Iterable,
    Literal,
    NotRequired,
    SupportsIndex,
    TypedDict,
    cast,
    overload,
)

from molt._wasm_abi_generated import wasm_runtime_callable_arity
from molt.compat import CompatibilityError, CompatibilityReporter, FallbackPolicy
from molt.frontend.cfg_analysis import CFGGraph, ControlMaps, build_cfg
from molt.frontend.module_publication import SourceModulePublication
from molt.type_facts import normalize_type_hint
from molt.source_root import compiler_source_root

if TYPE_CHECKING:
    # _TrackedOpsList's `owner` is the assembled generator. Imported under
    # TYPE_CHECKING only: there is no runtime import cycle back into __init__.
    from molt.frontend import SimpleTIRGenerator
    from molt.frontend.sema.funcmeta import (
        StatefulFunctionFramePlan,
        StatefulLocalsLayout,
    )


@dataclass
class MoltValue:
    name: str
    type_hint: str = "Unknown"
    # Concrete runtime class identity proven by the operation that produced
    # this value.  Unlike ``type_hint``, this may authorize fixed-layout access.
    exact_class: str | None = None
    # Exact identity is a temporal fact.  A callback or other invalidating
    # boundary advances the generator token, making already-held values stale.
    exact_class_token: int | None = None
    # A value read from an activation slot, without an independent reference.
    # Transparent expression joins preserve this storage fact; heap-producing
    # operations and explicit BINDING_ALIAS captures own their result instead.
    borrows_binding: bool = False


@dataclass(frozen=True)
class ExactClassFact:
    class_id: str
    token: int


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
class ScratchCell:
    """Opaque compiler-temporary storage, never a Python frame binding."""

    value: MoltValue | None
    async_slot: AsyncFrameSlot | None
    type_hint: str

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
    type_hint: str = "Any"
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
    kind: str
    args: list[Any]
    result: MoltValue
    metadata: dict[str, Any] | None = None
    col_offset: int | None = None
    end_col_offset: int | None = None
    source_line: int | None = None


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
    class_node: ast.ClassDef | None = None
    class_cell: MoltValue | None = None
    methods: dict[str, MethodInfo] = field(default_factory=dict)


@dataclass(frozen=True)
class SCCPResult:
    in_values: dict[int, dict[str, Any]]
    out_values: dict[int, dict[str, Any]]
    executable_blocks: set[int]
    executable_edges: set[tuple[int, int]]
    branch_choice_by_if_index: dict[int, bool]
    loop_break_choice_by_index: dict[int, bool]


@dataclass(frozen=True)
class LoopBoundFact:
    iv_name: str
    start: int
    step: int
    bound: int
    compare_op: str
    compare_index: int
    compare_result: str


# 47-bit signed inline integer range for NaN-boxing.
_INLINE_INT_MIN = -(1 << 46)
_INLINE_INT_MAX = (1 << 46) - 1

_FAST_ARITH_OPS = frozenset(
    {
        "ADD",
        "SUB",
        "MUL",
        "NEG",
        "POS",
        "INPLACE_ADD",
        "INPLACE_SUB",
        "INPLACE_MUL",
        "BIT_OR",
        "BIT_AND",
        "BIT_XOR",
        "INPLACE_BIT_OR",
        "INPLACE_BIT_AND",
        "INPLACE_BIT_XOR",
        # Comparison ops: when both operands are int/bool, the backend
        # emits inline Cranelift icmp instead of calling molt_le/lt/etc.
        "LT",
        "LE",
        "GT",
        "GE",
        "EQ",
        "NE",
        "DIV",
        "FLOORDIV",
        "MOD",
        "LT",
        "LE",
        "GT",
        "GE",
        "EQ",
        "NE",
    }
)

_SCCP_OVERDEFINED = object()
_SCCP_UNKNOWN = object()
_SCCP_MISSING = object()  # Sentinel for MISSING values — must never propagate or fold
MidendProfile = Literal["dev", "release"]
MidendTier = Literal["A", "B", "C"]
_MIDEND_ENV_KEYS = (
    "MOLT_MIDEND_SKIP_OP_THRESHOLD",
    "MOLT_MIDEND_MONOLITH_FUNCTION_THRESHOLD",
    "MOLT_MIDEND_MONOLITH_TOTAL_OPS_THRESHOLD",
    "MOLT_MIDEND_HOT_TIER_PROMOTION",
    "MOLT_MIDEND_WORK_BUDGET",
    "MOLT_MIDEND_BUDGET_ALPHA",
    "MOLT_MIDEND_BUDGET_BETA",
    "MOLT_MIDEND_BUDGET_SCALE",
    "MOLT_MIDEND_MAX_ROUNDS",
    "MOLT_SCCP_MAX_ITERS",
    "MOLT_CSE_MAX_ITERS",
    "MOLT_CSE_FP_MAX_ITERS",
)


@dataclass(frozen=True)
class MidendTierClassification:
    tier: MidendTier
    source: str
    allow_hot_promotion: bool


# --- Deterministic mid-end degrade-ladder work model (#73) ------------------
# The mid-end's pass-degrade ladder used to gate on wall-clock elapsed time,
# which made the compiled IR depend on machine speed (a determinism-contract
# violation: identical source could emit different IR across processes).  The
# ladder now charges a DETERMINISTIC work cost — the live op count — at each
# inter-pass checkpoint and degrades when the running total exceeds a
# deterministic per-function work budget.  These constants calibrate that
# budget so non-pathological functions never degrade (preserving optimisation
# quality) while a pass that pathologically grows the op count still trips the
# ladder and bounds compile time.
#
# Number of degrade checkpoints reached per optimisation round on the
# non-degraded path (the count of `maybe_apply_budget_degrade(...)` calls
# inside the per-round body of `_canonicalize_control_aware_ops_impl`).  Used
# only to size the budget headroom; an exact match is not required (the growth
# headroom multiplier absorbs drift), but keep it in the right ballpark.
_MIDEND_DEGRADE_CHECKPOINTS = 9
# Multiplier applied to the nominal per-round work so a function whose op count
# stays roughly stable across its permitted rounds never degrades.  Pathological
# op-count explosion (a pass that balloons the IR) still exceeds the budget.
_MIDEND_WORK_GROWTH_HEADROOM = 4.0
# Conversion from the per-tier `budget_base_ms` constant into work-units of
# base headroom, so the deterministic budget keeps the same relative ordering
# across tiers that the old millisecond base provided.
_MIDEND_WORK_BASE_UNITS_PER_MS = 50.0


@dataclass(frozen=True)
class MidendFunctionPolicy:
    profile: MidendProfile
    tier: MidendTier
    tier_base: MidendTier
    tier_source: str
    promoted: bool
    promotion_source: str
    promotion_signal: str
    max_rounds: int
    sccp_iter_cap: int
    cse_iter_cap: int
    enable_deep_edge_thread: bool
    enable_cse: bool
    enable_guard_hoist: bool
    budget_ms: float
    # Deterministic work-unit budget for the mid-end pass-degrade ladder.
    # The degrade ladder MUST gate on this (a pure function of the IR — op and
    # block counts), never on wall-clock elapsed time: a time-gated optimiser
    # makes the compiled IR depend on machine speed / scheduling, which silently
    # violated the determinism contract (#73 — identical source + seed produced
    # divergent IR across processes whenever a compile happened to run slow
    # enough to trip the old `time.perf_counter()` budget and disable CSE).
    # `budget_ms` is retained for telemetry/logging only.
    work_budget: float
    allow_hot_promotion: bool
    module_function_count: int
    module_total_ops: int
    monolith_pressure_level: int


@dataclass(frozen=True)
class MidendEnvConfig:
    skip_op_threshold: int
    monolith_function_threshold: int
    monolith_total_ops_threshold: int
    hot_tier_promotion_enabled: bool
    # Deterministic work-unit budget override (env: MOLT_MIDEND_WORK_BUDGET).
    # When set, replaces the computed per-function work budget the degrade
    # ladder gates on.  This is the only supported mid-end budget override.
    work_budget_override: float | None
    budget_alpha: float
    budget_beta: float
    budget_scale: float
    max_rounds_override: int | None
    sccp_iter_cap_override: int | None
    cse_iter_cap_override: int | None
    cse_fp_max_iters: int


@dataclass
class ActiveException:
    value: MoltValue
    scope: TryScope
    slot: int | None = None
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


@dataclass(frozen=True)
class BuiltinFuncSpec:
    runtime: str
    params: tuple[str, ...]
    defaults: tuple[ast.expr, ...] = ()
    vararg: str | None = None
    pos_or_kw_params: tuple[str, ...] = ()
    kwonly_params: tuple[str, ...] = ()
    kw_defaults: tuple[ast.expr | None, ...] = ()
    module: str = "builtins"
    bind_kind: int | None = None
    text_signature: str | None = None


GEN_SEND_OFFSET = 0
GEN_THROW_OFFSET = 8
GEN_CLOSED_OFFSET = 16
GEN_YIELD_FROM_OFFSET = 32
GEN_CONTROL_SIZE = 48

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
IMPLICIT_CLASSMETHOD_NAMES = frozenset(
    {
        "__init_subclass__",
        "__class_getitem__",
    }
)

# Method names that are implicitly staticmethods (CPython treats `__new__` as
# a staticmethod implicitly).  The first parameter is the class but the method
# is unbound; same exclusion rules apply for instance-field collection.
IMPLICIT_STATICMETHOD_NAMES = frozenset(
    {
        "__new__",
        "__init_subclass__",
        "__class_getitem__",
    }
)


def _function_is_instance_method(item: ast.AST) -> bool:
    """Return True iff `item` is a regular instance method.

    Excludes `@classmethod`, `@staticmethod`, and the implicit-classmethod /
    implicit-staticmethod names (`__new__`, `__init_subclass__`,
    `__class_getitem__`).  Only instance methods may legitimately collect
    field/static-attribute names from `self.X = ...` assignments — using `cls`
    inside a classmethod must not feed the instance-layout machinery.
    """
    if not isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return False
    if item.name in IMPLICIT_CLASSMETHOD_NAMES:
        return False
    if item.name in IMPLICIT_STATICMETHOD_NAMES:
        return False
    for deco in item.decorator_list:
        # `@classmethod`, `@staticmethod` directly applied at the bare-name level.
        if isinstance(deco, ast.Name) and deco.id in {"classmethod", "staticmethod"}:
            return False
        # `@functools.classmethod` etc. — not standard, but matching attribute
        # form keeps us conservative.
        if isinstance(deco, ast.Attribute) and deco.attr in {
            "classmethod",
            "staticmethod",
        }:
            return False
    return True


# Methods on built-in types that the native backend can fast-dispatch when the
# callee's type_hint is "BoundMethod:<type>:<method>".  The value is the set of
# method names supported per type.  Only methods that have a corresponding
# fast-path implementation in function_compiler.rs (the `s_value` match arm)
# should appear here.
_BUILTIN_FAST_METHODS: dict[str, frozenset[str]] = {
    "str": frozenset({"upper", "lower", "strip", "startswith", "join"}),
    "list": frozenset({"append"}),
    "dict": frozenset({"get"}),
}

BUILTIN_EXCEPTION_NAMES = {
    "BaseException",
    "BaseExceptionGroup",
    "Exception",
    "ExceptionGroup",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BufferError",
    "EOFError",
    "FloatingPointError",
    "GeneratorExit",
    "ImportError",
    "ModuleNotFoundError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "NameError",
    "UnboundLocalError",
    "NotImplementedError",
    "PythonFinalizationError",
    "OSError",
    "EnvironmentError",
    "IOError",
    "WindowsError",
    "BlockingIOError",
    "ChildProcessError",
    "ConnectionError",
    "BrokenPipeError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "FileExistsError",
    "OverflowError",
    "PermissionError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "NotADirectoryError",
    "RecursionError",
    "ReferenceError",
    "RuntimeError",
    "StopIteration",
    "StopAsyncIteration",
    "SyntaxError",
    "IndentationError",
    "TabError",
    "SystemError",
    "SystemExit",
    "TimeoutError",
    "ProcessLookupError",
    "TypeError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeTranslateError",
    "ValueError",
    "ZeroDivisionError",
    "Warning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "RuntimeWarning",
    "SyntaxWarning",
    "UserWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "ResourceWarning",
    "EncodingWarning",
}

BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS = {
    name: idx
    for idx, name in enumerate(
        (
            "BaseException",
            "Exception",
            "KeyError",
            "IndexError",
            "ValueError",
            "TypeError",
            "RuntimeError",
            "StopIteration",
            "StopAsyncIteration",
            "AssertionError",
            "ImportError",
            "NameError",
            "UnboundLocalError",
            "NotImplementedError",
        ),
        start=1,
    )
}

_MOLT_MISSING = ast.Name(id="__molt_missing__", ctx=ast.Load())
_MOLT_CLOSURE_PARAM = "__molt_closure__"
_MOLT_MODULE_CHUNK_PARAM = "__molt_module_obj__"
_MOLT_MODULE_CHUNK_PREFIX = "molt_module_chunk"
# Fixed all-named Clinic binding, selected by sealed callable metadata.
MOLT_BIND_KIND_CLINIC_NAMED = 1

BUILTIN_FUNC_SPECS: dict[str, BuiltinFuncSpec] = {
    "isinstance": BuiltinFuncSpec(
        "molt_isinstance",
        ("obj", "classinfo"),
        text_signature="($module, obj, class_or_tuple, /)",
    ),
    "issubclass": BuiltinFuncSpec(
        "molt_issubclass",
        ("sub", "classinfo"),
        text_signature="($module, cls, class_or_tuple, /)",
    ),
    "len": BuiltinFuncSpec("molt_len", ("obj",), text_signature="($module, obj, /)"),
    "hash": BuiltinFuncSpec(
        "molt_hash_builtin", ("obj",), text_signature="($module, obj, /)"
    ),
    "ord": BuiltinFuncSpec("molt_ord", ("obj",), text_signature="($module, c, /)"),
    "chr": BuiltinFuncSpec("molt_chr", ("obj",), text_signature="($module, i, /)"),
    "abs": BuiltinFuncSpec(
        "molt_abs_builtin", ("obj",), text_signature="($module, x, /)"
    ),
    "ascii": BuiltinFuncSpec(
        "molt_ascii_from_obj", ("obj",), text_signature="($module, obj, /)"
    ),
    "bin": BuiltinFuncSpec(
        "molt_bin_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    "oct": BuiltinFuncSpec(
        "molt_oct_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    "hex": BuiltinFuncSpec(
        "molt_hex_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    "divmod": BuiltinFuncSpec(
        "molt_divmod_builtin", ("a", "b"), text_signature="($module, x, y, /)"
    ),
    "repr": BuiltinFuncSpec(
        "molt_repr_builtin", ("obj",), text_signature="($module, obj, /)"
    ),
    "format": BuiltinFuncSpec(
        "molt_format_builtin",
        ("value",),
        (ast.Constant(""),),
        pos_or_kw_params=("format_spec",),
        text_signature="($module, value, format_spec='', /)",
    ),
    "callable": BuiltinFuncSpec(
        "molt_callable_builtin", ("obj",), text_signature="($module, obj, /)"
    ),
    "id": BuiltinFuncSpec("molt_id", ("obj",), text_signature="($module, obj, /)"),
    "enumerate": BuiltinFuncSpec(
        "molt_enumerate_builtin",
        ("iterable", "start"),
        (_MOLT_MISSING,),
        text_signature="(iterable, start=0)",
    ),
    "pow": BuiltinFuncSpec(
        "molt_pow_mod",
        (),
        (ast.Constant(None),),
        pos_or_kw_params=("base", "exp", "mod"),
        bind_kind=MOLT_BIND_KIND_CLINIC_NAMED,
        text_signature="($module, /, base, exp, mod=None)",
    ),
    "round": BuiltinFuncSpec(
        "molt_round_builtin",
        (),
        (_MOLT_MISSING,),
        pos_or_kw_params=("number", "ndigits"),
        bind_kind=MOLT_BIND_KIND_CLINIC_NAMED,
        text_signature="($module, /, number, ndigits=None)",
    ),
    "iter": BuiltinFuncSpec("molt_iter_checked", ("obj",)),
    "map": BuiltinFuncSpec("molt_map_builtin", ("func",), vararg="iterables"),
    "filter": BuiltinFuncSpec("molt_filter_builtin", ("func", "iterable")),
    "zip": BuiltinFuncSpec(
        "molt_zip_builtin",
        (),
        vararg="iterables",
        kwonly_params=("strict",),
        kw_defaults=(ast.Constant(False),),
    ),
    "reversed": BuiltinFuncSpec(
        "molt_reversed_builtin", ("seq",), text_signature="(sequence, /)"
    ),
    "any": BuiltinFuncSpec(
        "molt_any_builtin", ("iterable",), text_signature="($module, iterable, /)"
    ),
    "all": BuiltinFuncSpec(
        "molt_all_builtin", ("iterable",), text_signature="($module, iterable, /)"
    ),
    "sum": BuiltinFuncSpec(
        "molt_sum_builtin",
        ("iterable",),
        (ast.Constant(0),),
        pos_or_kw_params=("start",),
        text_signature="($module, iterable, /, start=0)",
    ),
    "min": BuiltinFuncSpec(
        "molt_min_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    "max": BuiltinFuncSpec(
        "molt_max_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    "sorted": BuiltinFuncSpec(
        "molt_sorted_builtin",
        ("iterable", "key", "reverse"),
        defaults=(ast.Constant(None), ast.Constant(False)),
        text_signature="($module, iterable, /, *, key=None, reverse=False)",
    ),
    # CPython: dir([object]) uses the caller's locals() when called with no args.
    # Lower as a single-arg runtime call with an explicit MOLT_MISSING sentinel
    # default so the runtime can detect the no-arg case cheaply.
    "dir": BuiltinFuncSpec("molt_dir_builtin", ("obj",), (_MOLT_MISSING,)),
    "open": BuiltinFuncSpec(
        "molt_open_builtin",
        (),
        (
            ast.Constant("r"),
            ast.Constant(-1),
            ast.Constant(None),
            ast.Constant(None),
            ast.Constant(None),
            ast.Constant(True),
            ast.Constant(None),
        ),
        pos_or_kw_params=(
            "file",
            "mode",
            "buffering",
            "encoding",
            "errors",
            "newline",
            "closefd",
            "opener",
        ),
        module="_io",
        bind_kind=MOLT_BIND_KIND_CLINIC_NAMED,
        text_signature="($module, /, file, mode='r', buffering=-1, encoding=None,\n     errors=None, newline=None, closefd=True, opener=None)",
    ),
    "next": BuiltinFuncSpec(
        "molt_next_builtin", ("iterator", "default"), (_MOLT_MISSING,)
    ),
    "aiter": BuiltinFuncSpec(
        "molt_aiter", ("obj",), text_signature="($module, async_iterable, /)"
    ),
    "anext": BuiltinFuncSpec(
        "molt_anext_builtin",
        ("aiter", "default"),
        (_MOLT_MISSING,),
        text_signature="($module, aiterator, default=<unrepresentable>, /)",
    ),
    "getattr": BuiltinFuncSpec(
        "molt_getattr_builtin", ("obj", "name", "default"), (_MOLT_MISSING,)
    ),
    "setattr": BuiltinFuncSpec(
        "molt_set_attr_name",
        ("obj", "name", "value"),
        text_signature="($module, obj, name, value, /)",
    ),
    "delattr": BuiltinFuncSpec(
        "molt_del_attr_name", ("obj", "name"), text_signature="($module, obj, name, /)"
    ),
    "hasattr": BuiltinFuncSpec(
        "molt_has_attr_name", ("obj", "name"), text_signature="($module, obj, name, /)"
    ),
    "compile": BuiltinFuncSpec(
        "molt_compile_builtin",
        ("source", "filename", "mode", "flags", "dont_inherit", "optimize"),
        (ast.Constant(0), ast.Constant(False), ast.Constant(-1)),
        text_signature="($module, /, source, filename, mode, flags=0,\n        dont_inherit=False, optimize=-1, *, _feature_version=-1)",
    ),
    "print": BuiltinFuncSpec(
        "molt_print_builtin",
        (),
        (),
        vararg="args",
        kwonly_params=("sep", "end", "file", "flush"),
        kw_defaults=(
            ast.Constant(" "),
            ast.Constant("\n"),
            ast.Constant(None),
            ast.Constant(False),
        ),
        text_signature="($module, /, *args, sep=' ', end='\\n', file=None, flush=False)",
    ),
    # CPython parity: vars() is equivalent to locals() with no arguments.
    "vars": BuiltinFuncSpec("molt_vars_builtin", ("obj",), (_MOLT_MISSING,)),
    "globals": BuiltinFuncSpec(
        "molt_globals_builtin", (), text_signature="($module, /)"
    ),
    "locals": BuiltinFuncSpec("molt_locals_builtin", (), text_signature="($module, /)"),
    "__import__": BuiltinFuncSpec(
        "molt_importlib_import_transaction",
        (),
        (
            _MOLT_MISSING,
            ast.Constant(None),
            ast.Tuple(elts=[], ctx=ast.Load()),
            ast.Constant(0),
        ),
        pos_or_kw_params=("name", "globals", "locals", "fromlist", "level"),
        bind_kind=MOLT_BIND_KIND_CLINIC_NAMED,
        text_signature="($module, /, name, globals=None, locals=None, fromlist=(),\n           level=0)",
    ),
}

# ── intrinsic arity lookup (compile-time optimisation) ────────────
# Build a reverse map from runtime name -> arity so that compile-time
# _require_intrinsic validation can confirm known intrinsic symbols
# before lowering them to runtime resolver calls.

_INTRINSIC_ARITY_CACHE: dict[str, int] | None = None
_INTRINSIC_SYMBOL_CACHE: dict[str, str] | None = None


def _intrinsic_signature_paths() -> list[Path]:
    return [
        Path(__file__).resolve().parent.parent / "_intrinsics.pyi",
        compiler_source_root()
        / "runtime"
        / "molt-runtime"
        / "src"
        / "intrinsics"
        / "manifest.pyi",
    ]


def _split_intrinsic_params(params_str: str) -> list[str]:
    if not params_str:
        return []
    depth = 0
    parts: list[str] = []
    current = ""
    for ch in params_str:
        if ch in "([{":
            depth += 1
            current += ch
        elif ch in ")]}":
            depth -= 1
            current += ch
        elif ch == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += ch
    if current.strip():
        parts.append(current.strip())
    return [p for p in parts if p and not p.startswith("*")]


def _iter_intrinsic_signatures() -> Iterable[tuple[str, list[str]]]:
    import re as _re

    sig_re = _re.compile(r"^def\s+(\w+)\(([^)]*)\)")
    for pyi_path in _intrinsic_signature_paths():
        if not pyi_path.exists():
            continue
        text = pyi_path.read_text()
        collapsed: list[str] = []
        buf = ""
        for line in text.splitlines():
            if buf:
                buf += " " + line.strip()
                if ")" in buf:
                    collapsed.append(buf)
                    buf = ""
            elif line.startswith("def "):
                if ")" in line:
                    collapsed.append(line)
                else:
                    buf = line.strip()
        for line in collapsed:
            match = sig_re.match(line)
            if match:
                yield match.group(1), _split_intrinsic_params(match.group(2).strip())


def _ensure_intrinsic_arity_cache() -> dict[str, int]:
    """Return the cached runtime-name -> arity map for intrinsic callables."""
    global _INTRINSIC_ARITY_CACHE
    if _INTRINSIC_ARITY_CACHE is None:
        cache: dict[str, int] = {}
        for spec in BUILTIN_FUNC_SPECS.values():
            cache[spec.runtime] = _builtin_func_abi_arity(spec)
        for name, params in _iter_intrinsic_signatures():
            cache.setdefault(name, len(params))
        _INTRINSIC_ARITY_CACHE = cache
    return _INTRINSIC_ARITY_CACHE


def _builtin_func_declared_arity(spec: BuiltinFuncSpec) -> int:
    arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
    if spec.vararg is not None:
        arity += 1
    return arity


def _builtin_func_abi_arity(spec: BuiltinFuncSpec) -> int:
    """Return the manifest-backed ABI arity for a frontend builtin callable."""
    declared_arity = _builtin_func_declared_arity(spec)
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


def _ensure_intrinsic_symbol_cache() -> dict[str, str]:
    """Return the cached intrinsic name -> canonical runtime symbol map."""
    global _INTRINSIC_SYMBOL_CACHE
    if _INTRINSIC_SYMBOL_CACHE is None:
        import re as _re

        cache: dict[str, str] = {}
        generated_path = (
            compiler_source_root()
            / "runtime"
            / "molt-runtime"
            / "src"
            / "intrinsics"
            / "generated.rs"
        )
        if generated_path.exists():
            text = generated_path.read_text()
            entry_re = _re.compile(
                r'IntrinsicSpec\s*\{\s*name:\s*"(?P<name>[^"]+)"\s*,\s*symbol:\s*"(?P<symbol>[^"]+)"',
                _re.DOTALL,
            )
            for match in entry_re.finditer(text):
                cache.setdefault(match.group("name"), match.group("symbol"))
        _INTRINSIC_SYMBOL_CACHE = cache
    return _INTRINSIC_SYMBOL_CACHE


def _canonical_intrinsic_runtime_name(runtime_name: str) -> str:
    return _ensure_intrinsic_symbol_cache().get(runtime_name, runtime_name)


def _intrinsic_arity_exact(runtime_name: str) -> int | None:
    """Return the parameter count for *runtime_name*, or ``None`` if unknown."""
    return _ensure_intrinsic_arity_cache().get(runtime_name)


def _intrinsic_arity(runtime_name: str) -> int:
    """Return the parameter count for *runtime_name*.

    1. Check BUILTIN_FUNC_SPECS (keyed by Python name, value has .runtime).
    2. Fall back to parsing molt/_intrinsics.pyi for ``def <name>(...)``.
    3. Default to 0 if not found anywhere.
    """
    arity = _intrinsic_arity_exact(runtime_name)
    return 0 if arity is None else arity


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
    break_flag: int | ScratchCell | None = None
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
    abandon_on_unwind: ScratchCell | None = None
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
    return_hint: str | None
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


class ClassInfo(TypedDict, total=False):
    fields: dict[str, int]
    field_hints: dict[str, str]
    size: int
    field_order: list[str]
    defaults: dict[str, ast.expr]
    class_attrs: dict[str, ast.expr]
    module: str
    base: str | None
    bases: list[str]
    mro: list[str]
    dynamic: bool
    static: bool
    dataclass: bool
    frozen: bool
    repr: bool
    slots: bool
    dataclass_params: dict[str, bool]
    methods: dict[str, MethodInfo]
    pending_methods: set[str]
    layout_version: int
    exception_subclass: bool
    needs_classcell: bool
    custom_metaclass: bool
    class_value_name: str
    constructor_fold_safe: bool
    decorated: bool
    heap_kind: str | None


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


class _TrackedOpsList(list[MoltOp]):
    def __init__(
        self,
        owner: "SimpleTIRGenerator",
        initial: list[MoltOp] | None = None,
    ) -> None:
        super().__init__(initial or [])
        self._owner = owner

    def append(self, item: MoltOp) -> None:
        super().append(item)
        self._owner._adjust_module_pressure_counts(ops_delta=1)

    def extend(self, items: Iterable[MoltOp]) -> None:
        items_list = list(items)
        super().extend(items_list)
        self._owner._adjust_module_pressure_counts(ops_delta=len(items_list))

    def insert(self, index: SupportsIndex, item: MoltOp) -> None:
        super().insert(index, item)
        self._owner._adjust_module_pressure_counts(ops_delta=1)

    def pop(self, index: SupportsIndex = -1) -> MoltOp:
        item = super().pop(index)
        self._owner._adjust_module_pressure_counts(ops_delta=-1)
        return item

    def remove(self, item: MoltOp) -> None:
        super().remove(item)
        self._owner._adjust_module_pressure_counts(ops_delta=-1)

    def clear(self) -> None:
        old_len = len(self)
        super().clear()
        self._owner._adjust_module_pressure_counts(ops_delta=-old_len)

    @overload
    def __setitem__(self, index: SupportsIndex, value: MoltOp) -> None: ...

    @overload
    def __setitem__(self, index: slice, value: Iterable[MoltOp]) -> None: ...

    def __setitem__(
        self,
        index: SupportsIndex | slice,
        value: MoltOp | Iterable[MoltOp],
    ) -> None:
        if isinstance(index, slice):
            replacement = list(cast(Iterable[MoltOp], value))
            current = list(self[index])
            super().__setitem__(index, replacement)
            self._owner._adjust_module_pressure_counts(
                ops_delta=len(replacement) - len(current)
            )
            return
        super().__setitem__(index, cast(MoltOp, value))

    def __delitem__(self, index: SupportsIndex | slice) -> None:
        if isinstance(index, slice):
            removed = list(self[index])
        else:
            removed = None
        super().__delitem__(index)
        self._owner._adjust_module_pressure_counts(
            ops_delta=-len(removed) if removed is not None else -1
        )

    def __iadd__(self, items: Iterable[MoltOp]) -> "_TrackedOpsList":
        items_list = list(items)
        result = cast("_TrackedOpsList", super().__iadd__(items_list))
        self._owner._adjust_module_pressure_counts(ops_delta=len(items_list))
        return result


class CanonicalizationState(TypedDict):
    aliases: dict[str, MoltValue]
    const_int_values: dict[str, int]
    value_type_tags: dict[str, int]
    available_values: dict[tuple[Any, ...], MoltValue]
    guard_dict_shapes: dict[str, tuple[str, str]]
    alias_epochs: dict[str, int]
    object_epochs: dict[str, int]
    memory_epoch: int


_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY = "__signature_cache"

__all__ = [
    "MoltValue",
    "MoltOp",
    "SCCPResult",
    "LoopBoundFact",
    "_INLINE_INT_MIN",
    "_INLINE_INT_MAX",
    "_FAST_ARITH_OPS",
    "_SCCP_OVERDEFINED",
    "_SCCP_UNKNOWN",
    "_SCCP_MISSING",
    "MidendProfile",
    "MidendTier",
    "_MIDEND_ENV_KEYS",
    "MidendTierClassification",
    "MidendFunctionPolicy",
    "MidendEnvConfig",
    "ActiveException",
    "BuiltinFuncSpec",
    "GEN_SEND_OFFSET",
    "GEN_THROW_OFFSET",
    "GEN_CLOSED_OFFSET",
    "GEN_YIELD_FROM_OFFSET",
    "GEN_CONTROL_SIZE",
    "BUILTIN_TYPE_TAGS",
    "BUILTIN_LAYOUT_MIN",
    "IMPLICIT_CLASSMETHOD_NAMES",
    "IMPLICIT_STATICMETHOD_NAMES",
    "_function_is_instance_method",
    "_BUILTIN_FAST_METHODS",
    "BUILTIN_EXCEPTION_NAMES",
    "BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS",
    "_MOLT_MISSING",
    "_MOLT_CLOSURE_PARAM",
    "_MOLT_MODULE_CHUNK_PARAM",
    "_MOLT_MODULE_CHUNK_PREFIX",
    "MOLT_BIND_KIND_CLINIC_NAMED",
    "BUILTIN_FUNC_SPECS",
    "_INTRINSIC_ARITY_CACHE",
    "_INTRINSIC_SYMBOL_CACHE",
    "_builtin_func_abi_arity",
    "_ensure_intrinsic_arity_cache",
    "_ensure_intrinsic_symbol_cache",
    "_canonical_intrinsic_runtime_name",
    "_intrinsic_arity_exact",
    "_intrinsic_arity",
    "TryScope",
    "MethodInfo",
    "ClassInfo",
    "FuncInfo",
    "_TrackedOpsList",
    "CanonicalizationState",
    "_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY",
    "CompatibilityError",
    "CompatibilityReporter",
    "FallbackPolicy",
    "CFGGraph",
    "ControlMaps",
    "build_cfg",
    "normalize_type_hint",
]
