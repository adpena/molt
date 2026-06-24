"""Leaf module: shared frontend data types, constants, and lookup tables.

Extracted from frontend/__init__.py (F1 decomposition, move-only). This is the
bottom of the frontend package import graph: both __init__.py (the
SimpleTIRGenerator assembly) and every visitor/lowering mixin import from here.
It must never import from molt.frontend.__init__ or any mixin (cycle break).
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
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

from molt.compat import CompatibilityError, CompatibilityReporter, FallbackPolicy
from molt.frontend.cfg_analysis import CFGGraph, ControlMaps, build_cfg
from molt.type_facts import normalize_type_hint

if TYPE_CHECKING:
    # _TrackedOpsList's `owner` is the assembled generator. Imported under
    # TYPE_CHECKING only: there is no runtime import cycle back into __init__.
    from molt.frontend import SimpleTIRGenerator

# ---------------------------------------------------------------------------
# Inline cache (IC) site index allocator
# ---------------------------------------------------------------------------
# Each GETATTR_GENERIC_PTR site gets a unique IC index so the runtime can
# map it to a slot in the lock-free InlineCache table (4096 entries).
# The counter wraps around at the table capacity.

_IC_TABLE_CAPACITY = 4096
_ic_counter: list[int] = [0]  # mutable counter in list for closure capture
_STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF = "static_module_class_binding"


def _next_ic_index() -> int:
    """Return a monotonically increasing IC site index (mod table capacity)."""
    idx = _ic_counter[0] % _IC_TABLE_CAPACITY
    _ic_counter[0] += 1
    return idx


class _InlineSuperFoldRequired(Exception):
    """Raised inside ``_try_inline_method_call`` when an inlined method body
    contains a ``super()`` call that cannot be folded statically.

    The inlined body is spliced into the caller's scope, which carries no
    ``__class__`` closure cell, so a ``super()`` that lowers to the runtime
    super path would raise ``RuntimeError: super(): __class__ cell not found``
    (or bind to the wrong class) at execution time.  This sentinel aborts the
    whole inline expansion; the caller then falls back to the general dispatch
    path, which threads the method's real ``__class__`` closure cell.
    """


@dataclass
class MoltValue:
    name: str
    type_hint: str = "Unknown"


@dataclass
class MoltOp:
    kind: str
    args: list[Any]
    result: MoltValue
    metadata: dict[str, Any] | None = None
    col_offset: int | None = None
    end_col_offset: int | None = None


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
    emits ``INDEX(ns, name)`` — the heap dict is loop-carried-correct without SSA
    phi participation, the same way the module dict backs module-scope loops.
    ``attr_values`` additionally snapshots name->MoltValue for the static
    ``CLASS_DEF`` fast path (straight-line bodies that never need the dict).
    ``names`` is the set of names bound in this class body; a Name not in it
    resolves through the enclosing/global/builtin chain (CPython LOAD_NAME).

    The instance is pushed/popped on ``SimpleTIRGenerator._class_ns_stack`` by
    ``visit_ClassDef`` and consulted by ``_store_local_value`` /
    ``_load_local_value`` / ``_emit_delete_name``.
    """

    ns: "MoltValue | None"
    attr_values: dict[str, MoltValue]
    names: set[str]


@dataclass(frozen=True)
class SCCPResult:
    in_values: dict[int, dict[str, Any]]
    out_values: dict[int, dict[str, Any]]
    executable_blocks: set[int]
    executable_edges: set[tuple[int, int]]
    branch_choice_by_if_index: dict[int, bool]
    loop_break_choice_by_index: dict[int, bool]
    try_exception_possible_by_start: dict[int, bool]
    try_normal_possible_by_start: dict[int, bool]
    guard_fail_indices: set[int]


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
_MIDEND_DEGRADE_CHECKPOINTS = 12
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
    enable_licm: bool
    enable_guard_hoist: bool
    budget_ms: float
    # Deterministic work-unit budget for the mid-end pass-degrade ladder.
    # The degrade ladder MUST gate on this (a pure function of the IR — op and
    # block counts), never on wall-clock elapsed time: a time-gated optimiser
    # makes the compiled IR depend on machine speed / scheduling, which silently
    # violated the determinism contract (#73 — identical source + seed produced
    # divergent IR across processes whenever a compile happened to run slow
    # enough to trip the old `time.perf_counter()` budget and disable CSE/LICM).
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


@dataclass(frozen=True)
class FormatLiteral:
    text: str


@dataclass(frozen=True)
class FormatField:
    key: int | str
    rest: list[tuple[bool, int | str]]
    conversion: str | None
    format_spec: list["FormatToken"] | None


FormatToken = FormatLiteral | FormatField


@dataclass
class FormatParseState:
    next_auto: int = 0
    used_auto: bool = False
    used_manual: bool = False


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
_MOLT_LOCALS_CACHE = "__molt_locals_cache__"
_MOLT_GLOBALS_BUILTIN = "__molt_globals_builtin__"
_MOLT_MODULE_CHUNK_PARAM = "__molt_module_obj__"
_MOLT_MODULE_CHUNK_PREFIX = "molt_module_chunk"
_BOOTSTRAP_TRACE_EXEMPT_MODULES = frozenset({"importlib.machinery"})
MOLT_BIND_KIND_OPEN = 1

BUILTIN_FUNC_SPECS: dict[str, BuiltinFuncSpec] = {
    "isinstance": BuiltinFuncSpec("molt_isinstance", ("obj", "classinfo")),
    "issubclass": BuiltinFuncSpec("molt_issubclass", ("sub", "classinfo")),
    "len": BuiltinFuncSpec("molt_len", ("obj",)),
    "hash": BuiltinFuncSpec("molt_hash_builtin", ("obj",)),
    "ord": BuiltinFuncSpec("molt_ord", ("obj",)),
    "chr": BuiltinFuncSpec("molt_chr", ("obj",)),
    "abs": BuiltinFuncSpec("molt_abs_builtin", ("obj",)),
    "ascii": BuiltinFuncSpec("molt_ascii_from_obj", ("obj",)),
    "bin": BuiltinFuncSpec("molt_bin_builtin", ("obj",)),
    "oct": BuiltinFuncSpec("molt_oct_builtin", ("obj",)),
    "hex": BuiltinFuncSpec("molt_hex_builtin", ("obj",)),
    "divmod": BuiltinFuncSpec("molt_divmod_builtin", ("a", "b")),
    "repr": BuiltinFuncSpec("molt_repr_builtin", ("obj",)),
    "format": BuiltinFuncSpec(
        "molt_format_builtin",
        ("value",),
        (ast.Constant(""),),
        pos_or_kw_params=("format_spec",),
    ),
    "callable": BuiltinFuncSpec("molt_callable_builtin", ("obj",)),
    "id": BuiltinFuncSpec("molt_id", ("obj",)),
    "enumerate": BuiltinFuncSpec(
        "molt_enumerate_builtin", ("iterable", "start"), (_MOLT_MISSING,)
    ),
    "round": BuiltinFuncSpec(
        "molt_round_builtin",
        (),
        (_MOLT_MISSING,),
        pos_or_kw_params=("number", "ndigits"),
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
    "reversed": BuiltinFuncSpec("molt_reversed_builtin", ("seq",)),
    "any": BuiltinFuncSpec("molt_any_builtin", ("iterable",)),
    "all": BuiltinFuncSpec("molt_all_builtin", ("iterable",)),
    "sum": BuiltinFuncSpec(
        "molt_sum_builtin",
        ("iterable",),
        (ast.Constant(0),),
        pos_or_kw_params=("start",),
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
    ),
    "next": BuiltinFuncSpec(
        "molt_next_builtin", ("iterator", "default"), (_MOLT_MISSING,)
    ),
    "aiter": BuiltinFuncSpec("molt_aiter", ("obj",)),
    "anext": BuiltinFuncSpec(
        "molt_anext_builtin", ("aiter", "default"), (_MOLT_MISSING,)
    ),
    "getattr": BuiltinFuncSpec(
        "molt_getattr_builtin", ("obj", "name", "default"), (_MOLT_MISSING,)
    ),
    "setattr": BuiltinFuncSpec("molt_set_attr_name", ("obj", "name", "value")),
    "delattr": BuiltinFuncSpec("molt_del_attr_name", ("obj", "name")),
    "hasattr": BuiltinFuncSpec("molt_has_attr_name", ("obj", "name")),
    "compile": BuiltinFuncSpec(
        "molt_compile_builtin",
        ("source", "filename", "mode", "flags", "dont_inherit", "optimize"),
        (ast.Constant(0), ast.Constant(False), ast.Constant(-1)),
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
    ),
    "_molt_getrecursionlimit": BuiltinFuncSpec("molt_getrecursionlimit", ()),
    "_molt_setrecursionlimit": BuiltinFuncSpec("molt_setrecursionlimit", ("limit",)),
    "_molt_getargv": BuiltinFuncSpec("molt_getargv", ()),
    "_molt_getframe": BuiltinFuncSpec("molt_getframe", ("depth",)),
    "_molt_trace_enter_slot": BuiltinFuncSpec("molt_trace_enter_slot", ("code_id",)),
    "_molt_trace_exit": BuiltinFuncSpec("molt_trace_exit", ()),
    "_molt_sys_version_info": BuiltinFuncSpec("molt_sys_version_info", ()),
    "_molt_sys_version": BuiltinFuncSpec("molt_sys_version", ()),
    "_molt_sys_stdin": BuiltinFuncSpec("molt_sys_stdin", ()),
    "_molt_sys_stdout": BuiltinFuncSpec("molt_sys_stdout", ()),
    "_molt_sys_stderr": BuiltinFuncSpec("molt_sys_stderr", ()),
    "molt_sys_set_version_info": BuiltinFuncSpec(
        "molt_sys_set_version_info",
        ("major", "minor", "micro", "releaselevel", "serial", "version"),
    ),
    "_molt_sys_executable": BuiltinFuncSpec("molt_sys_executable", ()),
    "_molt_exception_last": BuiltinFuncSpec("molt_exception_last", ()),
    "_molt_exception_active": BuiltinFuncSpec("molt_exception_active", ()),
    "_molt_asyncgen_hooks_get": BuiltinFuncSpec("molt_asyncgen_hooks_get", ()),
    "_molt_asyncgen_hooks_set": BuiltinFuncSpec(
        "molt_asyncgen_hooks_set", ("firstiter", "finalizer")
    ),
    "_molt_asyncgen_locals": BuiltinFuncSpec("molt_asyncgen_locals", ("asyncgen",)),
    "_molt_gen_locals": BuiltinFuncSpec("molt_gen_locals", ("gen",)),
    "_molt_code_new": BuiltinFuncSpec(
        "molt_code_new",
        (
            "filename",
            "name",
            "firstlineno",
            "linetable",
            "varnames",
            "names",
            "argcount",
            "posonlyargcount",
            "kwonlyargcount",
        ),
    ),
    "molt_future_cancel_msg": BuiltinFuncSpec(
        "molt_future_cancel_msg", ("future", "msg")
    ),
    "molt_future_cancel_clear": BuiltinFuncSpec(
        "molt_future_cancel_clear", ("future",)
    ),
    "_molt_module_new": BuiltinFuncSpec("molt_module_new", ("name",)),
    "_molt_function_set_builtin": BuiltinFuncSpec(
        "molt_function_set_builtin", ("func",)
    ),
    "_molt_class_new": BuiltinFuncSpec("molt_class_new", ("name",)),
    "_molt_class_set_base": BuiltinFuncSpec("molt_class_set_base", ("cls", "base")),
    "_molt_class_apply_set_name": BuiltinFuncSpec(
        "molt_class_apply_set_name", ("cls",)
    ),
    "_molt_getpid": BuiltinFuncSpec("molt_getpid", ()),
    "_molt_getcwd": BuiltinFuncSpec("molt_getcwd", ()),
    "_molt_os_name": BuiltinFuncSpec("molt_os_name", ()),
    "_molt_os_close": BuiltinFuncSpec("molt_os_close", ("fd",)),
    "_molt_os_dup": BuiltinFuncSpec("molt_os_dup", ("fd",)),
    "_molt_os_get_inheritable": BuiltinFuncSpec("molt_os_get_inheritable", ("fd",)),
    "_molt_os_set_inheritable": BuiltinFuncSpec(
        "molt_os_set_inheritable", ("fd", "inheritable")
    ),
    "_molt_os_urandom": BuiltinFuncSpec("molt_os_urandom", ("n",)),
    "_molt_math_log": BuiltinFuncSpec("molt_math_log", ("value",)),
    "_molt_math_log2": BuiltinFuncSpec("molt_math_log2", ("value",)),
    "_molt_math_exp": BuiltinFuncSpec("molt_math_exp", ("value",)),
    "_molt_math_sin": BuiltinFuncSpec("molt_math_sin", ("value",)),
    "_molt_math_cos": BuiltinFuncSpec("molt_math_cos", ("value",)),
    "_molt_math_acos": BuiltinFuncSpec("molt_math_acos", ("value",)),
    "_molt_math_lgamma": BuiltinFuncSpec("molt_math_lgamma", ("value",)),
    "_molt_struct_pack": BuiltinFuncSpec("molt_struct_pack", ("format", "values")),
    "_molt_struct_unpack": BuiltinFuncSpec("molt_struct_unpack", ("format", "buffer")),
    "_molt_struct_calcsize": BuiltinFuncSpec("molt_struct_calcsize", ("format",)),
    "_molt_codecs_decode": BuiltinFuncSpec(
        "molt_codecs_decode", ("obj", "encoding", "errors")
    ),
    "_molt_codecs_encode": BuiltinFuncSpec(
        "molt_codecs_encode", ("obj", "encoding", "errors")
    ),
    "_molt_sys_platform": BuiltinFuncSpec("molt_sys_platform", ()),
    "_molt_time_monotonic": BuiltinFuncSpec("molt_time_monotonic", ()),
    "_molt_time_monotonic_ns": BuiltinFuncSpec("molt_time_monotonic_ns", ()),
    "_molt_time_time": BuiltinFuncSpec("molt_time_time", ()),
    "_molt_time_time_ns": BuiltinFuncSpec("molt_time_time_ns", ()),
    "_molt_env_get_raw": BuiltinFuncSpec("molt_env_get", ("key", "default")),
    "_molt_env_snapshot": BuiltinFuncSpec("molt_env_snapshot", ()),
    "_molt_errno_constants": BuiltinFuncSpec("molt_errno_constants", ()),
    "_molt_socket_constants": BuiltinFuncSpec("molt_socket_constants", ()),
    "_molt_socket_has_ipv6": BuiltinFuncSpec("molt_socket_has_ipv6", ()),
    "_molt_socket_new": BuiltinFuncSpec(
        "molt_socket_new", ("family", "type", "proto", "fileno")
    ),
    "_molt_socket_close": BuiltinFuncSpec("molt_socket_close", ("sock",)),
    "_molt_socket_drop": BuiltinFuncSpec("molt_socket_drop", ("sock",)),
    "_molt_socket_clone": BuiltinFuncSpec("molt_socket_clone", ("sock",)),
    "_molt_socket_fileno": BuiltinFuncSpec("molt_socket_fileno", ("sock",)),
    "_molt_socket_gettimeout": BuiltinFuncSpec("molt_socket_gettimeout", ("sock",)),
    "_molt_socket_settimeout": BuiltinFuncSpec(
        "molt_socket_settimeout", ("sock", "timeout")
    ),
    "_molt_socket_setblocking": BuiltinFuncSpec(
        "molt_socket_setblocking", ("sock", "flag")
    ),
    "_molt_socket_getblocking": BuiltinFuncSpec("molt_socket_getblocking", ("sock",)),
    "_molt_socket_bind": BuiltinFuncSpec("molt_socket_bind", ("sock", "addr")),
    "_molt_socket_listen": BuiltinFuncSpec("molt_socket_listen", ("sock", "backlog")),
    "_molt_socket_accept": BuiltinFuncSpec("molt_socket_accept", ("sock",)),
    "_molt_socket_connect": BuiltinFuncSpec("molt_socket_connect", ("sock", "addr")),
    "_molt_socket_connect_ex": BuiltinFuncSpec(
        "molt_socket_connect_ex", ("sock", "addr")
    ),
    "_molt_socket_recv": BuiltinFuncSpec("molt_socket_recv", ("sock", "size", "flags")),
    "_molt_socket_recv_into": BuiltinFuncSpec(
        "molt_socket_recv_into", ("sock", "buffer", "size", "flags")
    ),
    "_molt_socket_send": BuiltinFuncSpec("molt_socket_send", ("sock", "data", "flags")),
    "_molt_socket_sendall": BuiltinFuncSpec(
        "molt_socket_sendall", ("sock", "data", "flags")
    ),
    "_molt_socket_sendto": BuiltinFuncSpec(
        "molt_socket_sendto", ("sock", "data", "flags", "addr")
    ),
    "_molt_socket_recvfrom": BuiltinFuncSpec(
        "molt_socket_recvfrom", ("sock", "size", "flags")
    ),
    "_molt_socket_shutdown": BuiltinFuncSpec("molt_socket_shutdown", ("sock", "how")),
    "_molt_socket_getsockname": BuiltinFuncSpec("molt_socket_getsockname", ("sock",)),
    "_molt_socket_getpeername": BuiltinFuncSpec("molt_socket_getpeername", ("sock",)),
    "_molt_socket_setsockopt": BuiltinFuncSpec(
        "molt_socket_setsockopt", ("sock", "level", "optname", "value")
    ),
    "_molt_socket_getsockopt": BuiltinFuncSpec(
        "molt_socket_getsockopt", ("sock", "level", "optname", "buflen")
    ),
    "_molt_socket_detach": BuiltinFuncSpec("molt_socket_detach", ("sock",)),
    "_molt_socketpair": BuiltinFuncSpec("molt_socketpair", ("family", "type", "proto")),
    "_molt_socket_getaddrinfo": BuiltinFuncSpec(
        "molt_socket_getaddrinfo",
        ("host", "port", "family", "type", "proto", "flags"),
    ),
    "_molt_socket_getnameinfo": BuiltinFuncSpec(
        "molt_socket_getnameinfo", ("addr", "flags")
    ),
    "_molt_socket_gethostname": BuiltinFuncSpec("molt_socket_gethostname", ()),
    "_molt_socket_getservbyname": BuiltinFuncSpec(
        "molt_socket_getservbyname", ("name", "proto")
    ),
    "_molt_socket_getservbyport": BuiltinFuncSpec(
        "molt_socket_getservbyport", ("port", "proto")
    ),
    "_molt_socket_inet_pton": BuiltinFuncSpec(
        "molt_socket_inet_pton", ("family", "address")
    ),
    "_molt_socket_inet_ntop": BuiltinFuncSpec(
        "molt_socket_inet_ntop", ("family", "packed")
    ),
    "_molt_io_wait_new": BuiltinFuncSpec(
        "molt_io_wait_new", ("socket", "events", "timeout")
    ),
    "_molt_ws_wait_new": BuiltinFuncSpec(
        "molt_ws_wait_new", ("ws", "events", "timeout")
    ),
    "molt_block_on": BuiltinFuncSpec("molt_block_on", ("task",)),
    "molt_asyncgen_shutdown": BuiltinFuncSpec("molt_asyncgen_shutdown", ()),
    "molt_process_spawn": BuiltinFuncSpec(
        "molt_process_spawn", ("args", "env", "cwd", "stdin", "stdout", "stderr")
    ),
    "molt_process_wait_future": BuiltinFuncSpec("molt_process_wait_future", ("proc",)),
    "molt_process_pid": BuiltinFuncSpec("molt_process_pid", ("proc",)),
    "molt_process_returncode": BuiltinFuncSpec("molt_process_returncode", ("proc",)),
    "molt_process_kill": BuiltinFuncSpec("molt_process_kill", ("proc",)),
    "molt_process_terminate": BuiltinFuncSpec("molt_process_terminate", ("proc",)),
    "molt_process_stdin": BuiltinFuncSpec("molt_process_stdin", ("proc",)),
    "molt_process_stdout": BuiltinFuncSpec("molt_process_stdout", ("proc",)),
    "molt_process_stderr": BuiltinFuncSpec("molt_process_stderr", ("proc",)),
    "molt_process_drop": BuiltinFuncSpec("molt_process_drop", ("proc",)),
    "molt_stream_new": BuiltinFuncSpec("molt_stream_new", ("capacity",)),
    "molt_stream_clone": BuiltinFuncSpec("molt_stream_clone", ("stream",)),
    "molt_stream_send_obj": BuiltinFuncSpec("molt_stream_send_obj", ("stream", "data")),
    "molt_stream_recv": BuiltinFuncSpec("molt_stream_recv", ("stream",)),
    "molt_stream_close": BuiltinFuncSpec("molt_stream_close", ("stream",)),
    "molt_stream_drop": BuiltinFuncSpec("molt_stream_drop", ("stream",)),
    "molt_pending": BuiltinFuncSpec("molt_pending", ()),
    "molt_chan_new": BuiltinFuncSpec("molt_chan_new", ("maxsize",), (ast.Constant(0),)),
    "molt_chan_send": BuiltinFuncSpec("molt_chan_send", ("chan", "val")),
    "molt_chan_recv": BuiltinFuncSpec("molt_chan_recv", ("chan",)),
    "molt_chan_try_send": BuiltinFuncSpec("molt_chan_try_send", ("chan", "val")),
    "molt_chan_try_recv": BuiltinFuncSpec("molt_chan_try_recv", ("chan",)),
    "molt_chan_send_blocking": BuiltinFuncSpec(
        "molt_chan_send_blocking", ("chan", "val")
    ),
    "molt_chan_recv_blocking": BuiltinFuncSpec("molt_chan_recv_blocking", ("chan",)),
    "molt_thread_spawn": BuiltinFuncSpec("molt_thread_spawn", ("payload",)),
    "molt_thread_join": BuiltinFuncSpec("molt_thread_join", ("handle", "timeout")),
    "molt_thread_is_alive": BuiltinFuncSpec("molt_thread_is_alive", ("handle",)),
    "molt_thread_ident": BuiltinFuncSpec("molt_thread_ident", ("handle",)),
    "molt_thread_native_id": BuiltinFuncSpec("molt_thread_native_id", ("handle",)),
    "molt_thread_current_ident": BuiltinFuncSpec("molt_thread_current_ident", ()),
    "molt_thread_current_native_id": BuiltinFuncSpec(
        "molt_thread_current_native_id", ()
    ),
    "molt_thread_drop": BuiltinFuncSpec("molt_thread_drop", ("handle",)),
    "molt_module_cache_set": BuiltinFuncSpec(
        "molt_module_cache_set", ("name", "module")
    ),
    "molt_lock_new": BuiltinFuncSpec("molt_lock_new", ()),
    "molt_lock_acquire": BuiltinFuncSpec(
        "molt_lock_acquire", ("handle", "blocking", "timeout")
    ),
    "molt_lock_release": BuiltinFuncSpec("molt_lock_release", ("handle",)),
    "molt_lock_locked": BuiltinFuncSpec("molt_lock_locked", ("handle",)),
    "molt_lock_drop": BuiltinFuncSpec("molt_lock_drop", ("handle",)),
    "molt_rlock_new": BuiltinFuncSpec("molt_rlock_new", ()),
    "molt_rlock_acquire": BuiltinFuncSpec(
        "molt_rlock_acquire", ("handle", "blocking", "timeout")
    ),
    "molt_rlock_release": BuiltinFuncSpec("molt_rlock_release", ("handle",)),
    "molt_rlock_locked": BuiltinFuncSpec("molt_rlock_locked", ("handle",)),
    "molt_rlock_drop": BuiltinFuncSpec("molt_rlock_drop", ("handle",)),
    "molt_weakref_register": BuiltinFuncSpec(
        "molt_weakref_register", ("weakref", "obj", "callback")
    ),
    "molt_weakref_get": BuiltinFuncSpec("molt_weakref_get", ("weakref",)),
    "molt_weakref_drop": BuiltinFuncSpec("molt_weakref_drop", ("weakref",)),
    "_molt_path_exists": BuiltinFuncSpec("molt_path_exists", ("path",)),
    "_molt_path_listdir": BuiltinFuncSpec("molt_path_listdir", ("path",)),
    "_molt_path_mkdir": BuiltinFuncSpec(
        "molt_path_mkdir", ("path", "mode"), (ast.Constant(0o777),)
    ),
    "_molt_path_unlink": BuiltinFuncSpec("molt_path_unlink", ("path",)),
    "_molt_path_rmdir": BuiltinFuncSpec("molt_path_rmdir", ("path",)),
    # CPython parity: vars() is equivalent to locals() with no arguments.
    "vars": BuiltinFuncSpec("molt_vars_builtin", ("obj",), (_MOLT_MISSING,)),
    "_molt_heapq_heapify": BuiltinFuncSpec("molt_heapq_heapify", ("list_obj",)),
    "_molt_heapq_heappush": BuiltinFuncSpec(
        "molt_heapq_heappush", ("list_obj", "item")
    ),
    "_molt_heapq_heappop": BuiltinFuncSpec("molt_heapq_heappop", ("list_obj",)),
    "_molt_heapq_heapreplace": BuiltinFuncSpec(
        "molt_heapq_heapreplace", ("list_obj", "item")
    ),
    "_molt_heapq_heappushpop": BuiltinFuncSpec(
        "molt_heapq_heappushpop", ("list_obj", "item")
    ),
}

# ── intrinsic arity lookup (compile-time optimisation) ────────────
# Build a reverse map from runtime name -> arity so that compile-time
# _require_intrinsic validation can confirm known intrinsic symbols
# before lowering them to runtime resolver calls.

_INTRINSIC_ARITY_CACHE: dict[str, int] | None = None
_INTRINSIC_SYMBOL_CACHE: dict[str, str] | None = None
_INTRINSIC_DEFAULTS_CACHE: dict[str, tuple[object, ...]] | None = None


def _intrinsic_signature_paths() -> list[Path]:
    return [
        Path(__file__).resolve().parent.parent / "_intrinsics.pyi",
        Path(__file__).resolve().parents[3]
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


def _intrinsic_param_default_expr(param: str) -> str | None:
    depth = 0
    for idx, ch in enumerate(param):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "=" and depth == 0:
            return param[idx + 1 :].strip()
    return None


def _intrinsic_literal_default(default_expr: str, intrinsic_name: str) -> object | None:
    if default_expr == "...":
        return Ellipsis
    if default_expr == "None":
        return None
    if default_expr == "True":
        return True
    if default_expr == "False":
        return False
    if default_expr.lstrip("-").isdigit():
        return int(default_expr)
    raise RuntimeError(
        f"unsupported concrete default for {intrinsic_name}: {default_expr!r}"
    )


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
        # Seed from BUILTIN_FUNC_SPECS (runtime name -> arity).
        for spec in BUILTIN_FUNC_SPECS.values():
            arity = (
                len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
            )
            if spec.vararg is not None:
                arity += 1
            cache[spec.runtime] = arity
        for name, params in _iter_intrinsic_signatures():
            cache.setdefault(name, len(params))
        _INTRINSIC_ARITY_CACHE = cache
    return _INTRINSIC_ARITY_CACHE


def _ensure_intrinsic_defaults_cache() -> dict[str, tuple[object, ...]]:
    """Return runtime-name -> trailing concrete default tuple for intrinsics."""
    global _INTRINSIC_DEFAULTS_CACHE
    if _INTRINSIC_DEFAULTS_CACHE is None:
        cache: dict[str, tuple[object, ...]] = {}
        for name, params in _iter_intrinsic_signatures():
            parsed: list[object | None] = []
            has_default: list[bool] = []
            for param in params:
                default_expr = _intrinsic_param_default_expr(param)
                if default_expr is None:
                    parsed.append(None)
                    has_default.append(False)
                    continue
                value = _intrinsic_literal_default(default_expr, name)
                if value is Ellipsis:
                    parsed.append(None)
                    has_default.append(False)
                    continue
                parsed.append(value)
                has_default.append(True)
            if not any(has_default):
                continue
            first = has_default.index(True)
            if not all(has_default[first:]):
                raise RuntimeError(
                    f"concrete defaults for {name} must form a trailing positional suffix"
                )
            cache.setdefault(name, tuple(parsed[first:]))
        _INTRINSIC_DEFAULTS_CACHE = cache
    return _INTRINSIC_DEFAULTS_CACHE


def _ensure_intrinsic_symbol_cache() -> dict[str, str]:
    """Return the cached intrinsic name -> canonical runtime symbol map."""
    global _INTRINSIC_SYMBOL_CACHE
    if _INTRINSIC_SYMBOL_CACHE is None:
        import re as _re

        cache: dict[str, str] = {}
        generated_path = (
            Path(__file__).resolve().parents[3]
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


def _intrinsic_defaults_exact(runtime_name: str) -> tuple[object, ...]:
    """Return concrete trailing defaults for *runtime_name*, if any."""
    defaults = _ensure_intrinsic_defaults_cache()
    return defaults.get(
        runtime_name,
        defaults.get(_canonical_intrinsic_runtime_name(runtime_name), ()),
    )


def _intrinsic_arity(runtime_name: str) -> int:
    """Return the parameter count for *runtime_name*.

    1. Check BUILTIN_FUNC_SPECS (keyed by Python name, value has .runtime).
    2. Fall back to parsing molt/_intrinsics.pyi for ``def <name>(...)``.
    3. Default to 0 if not found anywhere.
    """
    arity = _intrinsic_arity_exact(runtime_name)
    return 0 if arity is None else arity


MOLT_REEXPORT_FUNCTIONS = {
    "cancel_current": "molt.concurrency",
    "cancelled": "molt.concurrency",
    "CancellationToken": "molt.concurrency",
    "Channel": "molt.concurrency",
    "channel": "molt.concurrency",
    "current_token": "molt.concurrency",
    "set_current_token": "molt.concurrency",
    "spawn": "molt.concurrency",
    "Request": "molt.net",
    "Response": "molt.net",
    "Stream": "molt.net",
    "StreamSender": "molt.net",
    "WebSocket": "molt.net",
    "stream": "molt.net",
    "stream_channel": "molt.net",
    "ws_connect": "molt.net",
    "ws_pair": "molt.net",
}

MOLT_DIRECT_CALLS = {
    "molt": {
        "cancel_current",
        "cancelled",
        "channel",
        "current_token",
        "set_current_token",
        "spawn",
        "stream",
        "stream_channel",
        "ws_connect",
        "ws_pair",
    },
    "molt.concurrency": {
        "cancel_current",
        "cancelled",
        "channel",
        "current_token",
        "set_current_token",
        "spawn",
    },
    "molt.net": {
        "stream",
        "stream_channel",
        "ws_connect",
        "ws_pair",
    },
    "asyncio": {
        "create_task",
        "current_task",
        "ensure_future",
        "gather",
        "get_event_loop",
        "get_running_loop",
        "new_event_loop",
        "run",
        "set_event_loop",
        "sleep",
    },
    "contextlib": {"closing", "nullcontext"},
    "contextvars": {"copy_context"},
    "copy": {"copy", "deepcopy"},
    "dataclasses": {
        "asdict",
        "astuple",
        "dataclass",
        "field",
        "fields",
        "is_dataclass",
        "make_dataclass",
        "replace",
    },
    "email._encoded_words": {
        "decode",
        "decode_b",
        "decode_q",
        "encode",
        "encode_b",
        "encode_q",
        "len_b",
        "len_q",
    },
    "fnmatch": {"fnmatch", "fnmatchcase"},
    "functools": {"lru_cache", "partial", "reduce", "update_wrapper", "wraps"},
    "importlib": {"import_module", "invalidate_caches", "reload"},
    "inspect": {
        "cleandoc",
        "getdoc",
        "isfunction",
        "isclass",
        "ismodule",
        "iscoroutinefunction",
        "isgeneratorfunction",
        "signature",
    },
    "io": {"open", "stream"},
    "os": {"getenv", "unlink"},
    "pprint": {"pformat", "pprint"},
    "string": {"capwords"},
    "sys": {
        "exc_info",
        "getdefaultencoding",
        "getfilesystemencoding",
        "getrecursionlimit",
        "_getframe",
        "setrecursionlimit",
    },
    "itertools": {"chain", "islice", "repeat"},
    "traceback": {
        "format_exception",
        "format_exception_only",
        "format_exc",
        "format_tb",
        "print_exception",
        "print_exc",
        "print_tb",
    },
    "threading": {"Thread"},
    "tkinter._support": {
        "_has_gui_capability",
        "_has_process_spawn_capability",
        "_require_gui_capability",
        "_require_process_spawn_capability",
        "_require_tk_runtime",
        "_tk_available",
        "_tk_unavailable_message",
        "has_gui_capability",
        "has_process_spawn_capability",
        "require_gui_capability",
        "require_process_spawn_capability",
        "require_tk_runtime",
        "tk_available",
        "tk_unavailable_message",
    },
    "typing": {
        "TypeVar",
        "cast",
        "get_args",
        "get_origin",
        "overload",
        "runtime_checkable",
    },
    "warnings": {
        "catch_warnings",
        "filterwarnings",
        "formatwarning",
        "resetwarnings",
        "showwarning",
        "simplefilter",
        "warn",
        "warn_explicit",
    },
    "wsgiref.simple_server": {"make_server"},
}

MOLT_DIRECT_CALL_BIND_ALWAYS = {
    "asyncio": {"gather"},
    "functools": {"partial"},
    "molt.gpu.tensor": {"tensor_take_rows", "zeros"},
    "operator": {"attrgetter", "itemgetter", "methodcaller"},
    # itertools wrappers have vararg/default binding semantics that must go
    # through CALL_BIND unless we explicitly materialize packed args.
    "itertools": {"chain", "islice", "repeat"},
}


@dataclass(frozen=True)
class IntrinsicHandleClassConstructorSpec:
    type_hint: str
    handle_attr: str
    empty_intrinsic: str
    iterable_intrinsic: str
    iterable_types: frozenset[str]
    getitem_intrinsic: str | None = None
    len_intrinsic: str | None = None


INTRINSIC_HANDLE_CLASS_CONSTRUCTORS: dict[
    tuple[str, str], IntrinsicHandleClassConstructorSpec
] = {
    ("collections", "Counter"): IntrinsicHandleClassConstructorSpec(
        type_hint="counter",
        handle_attr="_handle",
        empty_intrinsic="molt_counter_new",
        iterable_intrinsic="molt_counter_from_iterable",
        iterable_types=frozenset({"list", "tuple"}),
        getitem_intrinsic="molt_counter_getitem",
        len_intrinsic="molt_counter_len",
    ),
}

INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE: dict[
    str, IntrinsicHandleClassConstructorSpec
] = {spec.type_hint: spec for spec in INTRINSIC_HANDLE_CLASS_CONSTRUCTORS.values()}

STDLIB_DIRECT_CALL_MODULES = {
    module for module in MOLT_DIRECT_CALLS if not module.startswith("molt.")
}


@dataclass
class TryScope:
    ctx_mark: MoltValue | None
    finalbody: list[ast.stmt] | None
    ctx_mark_offset: int | None = None
    done_label: int | None = None
    handler_label: int | None = None
    needs_context_unwind: bool = True
    try_start_has_handler_value: bool = True


class MethodInfo(TypedDict):
    func: MoltValue
    attr: MoltValue
    descriptor: Literal[
        "function",
        "classmethod",
        "staticmethod",
        "property",
        "decorated",
        "property_update",
    ]
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
    # True when ``has_closure`` is purely the implicit ``__class__`` super cell
    # (no real enclosing-local capture).  Set only by ``compile_method`` (the
    # non-generator path that computes inline metadata); the generator/async
    # method builders never populate it, so it is ``NotRequired`` and every
    # reader accesses it via ``.get("inline_closure_ok")``.
    inline_closure_ok: NotRequired[bool]
    inline_return: NotRequired[ast.expr | None]
    inline_params: NotRequired[list[str] | None]
    inline_owner_class: NotRequired[str | None]
    # Module that *defines* the inlinable method.  An inline splices the body
    # into the caller's scope; any bare-Name reference in the body that is not a
    # substituted parameter or a builtin resolves against the *caller's* module
    # globals (``visit_Name`` -> ``_emit_global_get``).  When the body reads one
    # of the defining module's globals (recorded in ``inline_free_names``), the
    # inline is therefore only sound when the call site is compiled in that same
    # module.  ``_try_inline_method_call`` consults both to refuse a cross-module
    # inline that would mis-resolve a defining-module global.
    inline_owner_module: NotRequired[str | None]
    inline_free_names: NotRequired[frozenset[str]]
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
    eq: bool
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


class FuncInfo(TypedDict):
    params: list[str]
    param_types: list[str]  # type hints from annotations ("int", "float", "Any", ...)
    return_hint: str | None
    ops: list[MoltOp]


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
    "_IC_TABLE_CAPACITY",
    "_ic_counter",
    "_STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF",
    "_next_ic_index",
    "_InlineSuperFoldRequired",
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
    "FormatLiteral",
    "FormatField",
    "FormatToken",
    "FormatParseState",
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
    "_MOLT_LOCALS_CACHE",
    "_MOLT_GLOBALS_BUILTIN",
    "_MOLT_MODULE_CHUNK_PARAM",
    "_MOLT_MODULE_CHUNK_PREFIX",
    "_BOOTSTRAP_TRACE_EXEMPT_MODULES",
    "MOLT_BIND_KIND_OPEN",
    "BUILTIN_FUNC_SPECS",
    "_INTRINSIC_ARITY_CACHE",
    "_INTRINSIC_SYMBOL_CACHE",
    "_INTRINSIC_DEFAULTS_CACHE",
    "_ensure_intrinsic_arity_cache",
    "_ensure_intrinsic_symbol_cache",
    "_ensure_intrinsic_defaults_cache",
    "_canonical_intrinsic_runtime_name",
    "_intrinsic_arity_exact",
    "_intrinsic_defaults_exact",
    "_intrinsic_arity",
    "MOLT_REEXPORT_FUNCTIONS",
    "MOLT_DIRECT_CALLS",
    "MOLT_DIRECT_CALL_BIND_ALWAYS",
    "IntrinsicHandleClassConstructorSpec",
    "INTRINSIC_HANDLE_CLASS_CONSTRUCTORS",
    "INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE",
    "STDLIB_DIRECT_CALL_MODULES",
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
