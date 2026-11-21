"""Frontend public API: Python-to-TIR lowering types and the generator.

Each public name resolves on first use from its owning module (PEP 562), so
importing ``molt.frontend`` or a leaf module such as ``molt.frontend._types``
does not load the generator. ``molt.frontend.generator`` assembles
``SimpleTIRGenerator`` from the visitor/lowering mixins; the shared data types
live in the ``molt.frontend._types`` leaf.
"""

from __future__ import annotations

import importlib
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

# Each public name and the module that owns it.
_EXPORTS: dict[str, str] = {
    "BoundMethodHint": "molt.frontend._types",
    "BuiltinClass": "molt.frontend._types",
    "ClassId": "molt.frontend._types",
    "DeclaredClass": "molt.frontend._types",
    "ReceiverKind": "molt.frontend._types",
    "MoltValue": "molt.frontend._types",
    "MoltOp": "molt.frontend._types",
    "_INLINE_INT_MIN": "molt.frontend._types",
    "_INLINE_INT_MAX": "molt.frontend._types",
    "ActiveException": "molt.frontend._types",
    "GEN_SEND_OFFSET": "molt.frontend._types",
    "GEN_THROW_OFFSET": "molt.frontend._types",
    "GEN_RESUME_RETURNED": "molt.frontend._types",
    "GEN_YIELD_FROM_OFFSET": "molt.frontend._types",
    "GEN_CONTROL_SIZE": "molt.frontend._types",
    "BUILTIN_TYPE_TAGS": "molt.frontend._types",
    "BUILTIN_LAYOUT_MIN": "molt.frontend._types",
    "IMPLICIT_CLASSMETHOD_NAMES": "molt.frontend._types",
    "IMPLICIT_STATICMETHOD_NAMES": "molt.frontend._types",
    "_function_is_instance_method": "molt.frontend._types",
    "_BUILTIN_FAST_METHODS": "molt.frontend._types",
    "BUILTIN_EXCEPTION_NAMES": "molt.frontend._types",
    "_MOLT_CLOSURE_PARAM": "molt.frontend._types",
    "_MOLT_MODULE_CHUNK_PARAM": "molt.frontend._types",
    "_MOLT_MODULE_CHUNK_PREFIX": "molt.frontend._types",
    "_builtin_func_abi_arity": "molt.frontend._types",
    "_intrinsic_arity_exact": "molt.frontend._types",
    "TryScope": "molt.frontend._types",
    "MethodInfo": "molt.frontend._types",
    "ClassModel": "molt.frontend.class_model",
    "layout_definition_symbol": "molt.frontend._types",
    "FuncInfo": "molt.frontend._types",
    "CompatibilityError": "molt.frontend._types",
    "CompatibilityReporter": "molt.frontend._types",
    "CFGGraph": "molt.frontend._types",
    "ControlMaps": "molt.frontend._types",
    "build_cfg": "molt.frontend._types",
    "normalize_type_hint": "molt.frontend._types",
    "SimpleTIRGenerator": "molt.frontend.generator",
    "compile_to_tir": "molt.frontend.generator",
    "SemaResult": "molt.frontend.sema",
    "analyze_module": "molt.frontend.sema",
}

__all__ = list(_EXPORTS)


def __getattr__(name: str) -> object:
    module = _EXPORTS.get(name)
    if module is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    value = getattr(importlib.import_module(module), name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_EXPORTS))
