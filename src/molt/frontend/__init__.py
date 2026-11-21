"""Frontend public API: Python-to-TIR lowering types and the generator.

Each public name resolves on first use from its owning module (PEP 562), so
importing ``molt.frontend`` or a leaf module such as ``molt.frontend._types``
does not load the generator. ``molt.frontend.generator`` assembles
``SimpleTIRGenerator`` from the visitor/lowering mixins; the shared data types
live in the ``molt.frontend._types`` leaf.
"""

from __future__ import annotations

from collections.abc import Callable
from types import ModuleType
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.frontend._types import (
        BoundMethodHint as BoundMethodHint,
        BuiltinClass as BuiltinClass,
        ClassId as ClassId,
        DeclaredClass as DeclaredClass,
        ReceiverKind as ReceiverKind,
        MoltValue as MoltValue,
        MoltOp as MoltOp,
        _INLINE_INT_MIN as _INLINE_INT_MIN,
        _INLINE_INT_MAX as _INLINE_INT_MAX,
        ActiveException as ActiveException,
        GEN_SEND_OFFSET as GEN_SEND_OFFSET,
        GEN_THROW_OFFSET as GEN_THROW_OFFSET,
        GEN_RESUME_RETURNED as GEN_RESUME_RETURNED,
        GEN_YIELD_FROM_OFFSET as GEN_YIELD_FROM_OFFSET,
        GEN_CONTROL_SIZE as GEN_CONTROL_SIZE,
        BUILTIN_TYPE_TAGS as BUILTIN_TYPE_TAGS,
        BUILTIN_LAYOUT_MIN as BUILTIN_LAYOUT_MIN,
        IMPLICIT_CLASSMETHOD_NAMES as IMPLICIT_CLASSMETHOD_NAMES,
        IMPLICIT_STATICMETHOD_NAMES as IMPLICIT_STATICMETHOD_NAMES,
        _function_is_instance_method as _function_is_instance_method,
        _BUILTIN_FAST_METHODS as _BUILTIN_FAST_METHODS,
        BUILTIN_EXCEPTION_NAMES as BUILTIN_EXCEPTION_NAMES,
        _MOLT_CLOSURE_PARAM as _MOLT_CLOSURE_PARAM,
        _MOLT_MODULE_CHUNK_PARAM as _MOLT_MODULE_CHUNK_PARAM,
        _MOLT_MODULE_CHUNK_PREFIX as _MOLT_MODULE_CHUNK_PREFIX,
        _builtin_func_abi_arity as _builtin_func_abi_arity,
        _intrinsic_arity_exact as _intrinsic_arity_exact,
        TryScope as TryScope,
        MethodInfo as MethodInfo,
        layout_definition_symbol as layout_definition_symbol,
        FuncInfo as FuncInfo,
        CompatibilityError as CompatibilityError,
        CompatibilityReporter as CompatibilityReporter,
        CFGGraph as CFGGraph,
        ControlMaps as ControlMaps,
        build_cfg as build_cfg,
        normalize_type_hint as normalize_type_hint,
    )
    from molt.frontend.class_model import (
        ClassModel as ClassModel,
    )
    from molt.frontend.generator import (
        SimpleTIRGenerator as SimpleTIRGenerator,
        compile_to_tir as compile_to_tir,
    )
    from molt.frontend.sema import (
        SemaResult as SemaResult,
        analyze_module as analyze_module,
    )


# Each owner loads on first use through a static import, so the import graph
# and the lowering cache key see every edge (no computed-name import).
def _types_module() -> ModuleType:
    from molt.frontend import _types as owner

    return owner


def _class_model_module() -> ModuleType:
    from molt.frontend import class_model as owner

    return owner


def _generator_module() -> ModuleType:
    from molt.frontend import generator as owner

    return owner


def _sema_module() -> ModuleType:
    from molt.frontend import sema as owner

    return owner


# Each public name and the loader of the module that owns it.
_EXPORTS: dict[str, Callable[[], ModuleType]] = {
    "BoundMethodHint": _types_module,
    "BuiltinClass": _types_module,
    "ClassId": _types_module,
    "DeclaredClass": _types_module,
    "ReceiverKind": _types_module,
    "MoltValue": _types_module,
    "MoltOp": _types_module,
    "_INLINE_INT_MIN": _types_module,
    "_INLINE_INT_MAX": _types_module,
    "ActiveException": _types_module,
    "GEN_SEND_OFFSET": _types_module,
    "GEN_THROW_OFFSET": _types_module,
    "GEN_RESUME_RETURNED": _types_module,
    "GEN_YIELD_FROM_OFFSET": _types_module,
    "GEN_CONTROL_SIZE": _types_module,
    "BUILTIN_TYPE_TAGS": _types_module,
    "BUILTIN_LAYOUT_MIN": _types_module,
    "IMPLICIT_CLASSMETHOD_NAMES": _types_module,
    "IMPLICIT_STATICMETHOD_NAMES": _types_module,
    "_function_is_instance_method": _types_module,
    "_BUILTIN_FAST_METHODS": _types_module,
    "BUILTIN_EXCEPTION_NAMES": _types_module,
    "_MOLT_CLOSURE_PARAM": _types_module,
    "_MOLT_MODULE_CHUNK_PARAM": _types_module,
    "_MOLT_MODULE_CHUNK_PREFIX": _types_module,
    "_builtin_func_abi_arity": _types_module,
    "_intrinsic_arity_exact": _types_module,
    "TryScope": _types_module,
    "MethodInfo": _types_module,
    "ClassModel": _class_model_module,
    "layout_definition_symbol": _types_module,
    "FuncInfo": _types_module,
    "CompatibilityError": _types_module,
    "CompatibilityReporter": _types_module,
    "CFGGraph": _types_module,
    "ControlMaps": _types_module,
    "build_cfg": _types_module,
    "normalize_type_hint": _types_module,
    "SimpleTIRGenerator": _generator_module,
    "compile_to_tir": _generator_module,
    "SemaResult": _sema_module,
    "analyze_module": _sema_module,
}

__all__ = list(_EXPORTS)


def __getattr__(name: str) -> object:
    loader = _EXPORTS.get(name)
    if loader is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    value = getattr(loader(), name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_EXPORTS))
