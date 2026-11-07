"""Declarative Python builtin callable metadata, independent of generated ABI.

The frontend validates these declarations against generated runtime facts when
consuming them. Generators validate the same declarations against their raw
manifest, so missing or stale generated projections cannot block regeneration.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from enum import IntEnum
from molt._public_objects_generated import PUBLIC_CALL_SIGNATURES, PUBLIC_OBJECTS


class BuiltinBindingPolicy(IntEnum):
    # Integer transport shares the existing binding-tag ABI with C methods and
    # constructor protocols (tags 2 through 6); identity is generated for Rust.
    CLINIC_NAMED = 1
    POSITIONAL_RANGE = 7

    @property
    def requires_custom_binding(self) -> bool:
        match self:
            case BuiltinBindingPolicy.CLINIC_NAMED:
                return True
            case BuiltinBindingPolicy.POSITIONAL_RANGE:
                return False
        raise AssertionError(f"unhandled builtin binding policy {self!r}")


@dataclass(frozen=True)
class BuiltinFuncSpec:
    runtime: str
    params: tuple[str, ...]
    defaults: tuple[ast.expr, ...] = ()
    vararg: str | None = None
    varkw: str | None = None
    pos_or_kw_params: tuple[str, ...] = ()
    kwonly_params: tuple[str, ...] = ()
    kw_defaults: tuple[ast.expr | None, ...] = ()
    bind_kind: BuiltinBindingPolicy | None = None
    text_signature: str | None = None

    @property
    def module(self) -> str:
        return PUBLIC_OBJECTS[self.runtime].module


_MOLT_MISSING = ast.Name(id="__molt_missing__", ctx=ast.Load())


_BUILTIN_FUNC_SIGNATURES: tuple[BuiltinFuncSpec, ...] = (
    BuiltinFuncSpec(
        "molt_isinstance",
        ("obj", "classinfo"),
        text_signature="($module, obj, class_or_tuple, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_issubclass",
        ("sub", "classinfo"),
        text_signature="($module, cls, class_or_tuple, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec("molt_len", ("obj",), text_signature="($module, obj, /)"),
    BuiltinFuncSpec("molt_hash_builtin", ("obj",), text_signature="($module, obj, /)"),
    BuiltinFuncSpec("molt_ord", ("obj",), text_signature="($module, c, /)"),
    BuiltinFuncSpec("molt_chr", ("obj",), text_signature="($module, i, /)"),
    BuiltinFuncSpec("molt_abs_builtin", ("obj",), text_signature="($module, x, /)"),
    BuiltinFuncSpec(
        "molt_ascii_from_obj", ("obj",), text_signature="($module, obj, /)"
    ),
    BuiltinFuncSpec(
        "molt_bin_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    BuiltinFuncSpec(
        "molt_oct_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    BuiltinFuncSpec(
        "molt_hex_builtin", ("obj",), text_signature="($module, number, /)"
    ),
    BuiltinFuncSpec(
        "molt_divmod_builtin",
        ("a", "b"),
        text_signature="($module, x, y, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec("molt_repr_builtin", ("obj",), text_signature="($module, obj, /)"),
    BuiltinFuncSpec(
        "molt_format_builtin",
        ("value", "format_spec"),
        (ast.Constant(""),),
        text_signature="($module, value, format_spec='', /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_callable_builtin", ("obj",), text_signature="($module, obj, /)"
    ),
    BuiltinFuncSpec("molt_id", ("obj",), text_signature="($module, obj, /)"),
    BuiltinFuncSpec(
        "molt_enumerate_builtin",
        (),
        (_MOLT_MISSING,),
        pos_or_kw_params=("iterable", "start"),
        text_signature="(iterable, start=0)",
    ),
    BuiltinFuncSpec(
        "molt_pow_mod",
        (),
        (ast.Constant(None),),
        pos_or_kw_params=("base", "exp", "mod"),
        bind_kind=BuiltinBindingPolicy.CLINIC_NAMED,
        text_signature="($module, /, base, exp, mod=None)",
    ),
    BuiltinFuncSpec(
        "molt_round_builtin",
        (),
        (_MOLT_MISSING,),
        pos_or_kw_params=("number", "ndigits"),
        bind_kind=BuiltinBindingPolicy.CLINIC_NAMED,
        text_signature="($module, /, number, ndigits=None)",
    ),
    BuiltinFuncSpec(
        "molt_iter_builtin",
        ("obj", "sentinel"),
        (_MOLT_MISSING,),
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec("molt_map_builtin", ("func",), vararg="iterables"),
    BuiltinFuncSpec("molt_filter_builtin", ("func", "iterable")),
    BuiltinFuncSpec(
        "molt_zip_builtin",
        (),
        vararg="iterables",
        kwonly_params=("strict",),
        kw_defaults=(ast.Constant(False),),
    ),
    BuiltinFuncSpec("molt_reversed_builtin", ("seq",), text_signature="(sequence, /)"),
    BuiltinFuncSpec(
        "molt_any_builtin", ("iterable",), text_signature="($module, iterable, /)"
    ),
    BuiltinFuncSpec(
        "molt_all_builtin", ("iterable",), text_signature="($module, iterable, /)"
    ),
    BuiltinFuncSpec(
        "molt_sum_builtin",
        ("iterable",),
        (ast.Constant(0),),
        pos_or_kw_params=("start",),
        text_signature="($module, iterable, /, start=0)",
    ),
    BuiltinFuncSpec(
        "molt_min_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    BuiltinFuncSpec(
        "molt_max_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    BuiltinFuncSpec(
        "molt_sorted_builtin",
        ("iterable",),
        kwonly_params=("key", "reverse"),
        kw_defaults=(ast.Constant(None), ast.Constant(False)),
        text_signature="($module, iterable, /, *, key=None, reverse=False)",
    ),
    # CPython: dir([object]) uses the caller's locals() when called with no args.
    # Lower as a single-arg runtime call with an explicit MOLT_MISSING sentinel
    # default so the runtime can detect the no-arg case cheaply.
    BuiltinFuncSpec(
        "molt_dir_builtin",
        ("obj",),
        (_MOLT_MISSING,),
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
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
        bind_kind=BuiltinBindingPolicy.CLINIC_NAMED,
        text_signature="($module, /, file, mode='r', buffering=-1, encoding=None,\n     errors=None, newline=None, closefd=True, opener=None)",
    ),
    BuiltinFuncSpec(
        "molt_next_builtin",
        ("iterator", "default"),
        (_MOLT_MISSING,),
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_aiter", ("obj",), text_signature="($module, async_iterable, /)"
    ),
    BuiltinFuncSpec(
        "molt_anext_builtin",
        ("aiter", "default"),
        (_MOLT_MISSING,),
        text_signature="($module, aiterator, default=<unrepresentable>, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_getattr_builtin",
        ("obj", "name", "default"),
        (_MOLT_MISSING,),
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_set_attr_name",
        ("obj", "name", "value"),
        text_signature="($module, obj, name, value, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_del_attr_name",
        ("obj", "name"),
        text_signature="($module, obj, name, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_has_attr_name",
        ("obj", "name"),
        text_signature="($module, obj, name, /)",
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec(
        "molt_compile_builtin",
        (),
        (ast.Constant(0), ast.Constant(False), ast.Constant(-1)),
        pos_or_kw_params=(
            "source",
            "filename",
            "mode",
            "flags",
            "dont_inherit",
            "optimize",
        ),
        text_signature="($module, /, source, filename, mode, flags=0,\n        dont_inherit=False, optimize=-1, *, _feature_version=-1)",
    ),
    BuiltinFuncSpec(
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
    BuiltinFuncSpec(
        "molt_vars_builtin",
        ("obj",),
        (_MOLT_MISSING,),
        bind_kind=BuiltinBindingPolicy.POSITIONAL_RANGE,
    ),
    BuiltinFuncSpec("molt_globals_builtin", (), text_signature="($module, /)"),
    BuiltinFuncSpec("molt_locals_builtin", (), text_signature="($module, /)"),
    BuiltinFuncSpec(
        "molt_importlib_import_transaction",
        (),
        (
            _MOLT_MISSING,
            ast.Constant(None),
            ast.Tuple(elts=[], ctx=ast.Load()),
            ast.Constant(0),
        ),
        pos_or_kw_params=("name", "globals", "locals", "fromlist", "level"),
        bind_kind=BuiltinBindingPolicy.CLINIC_NAMED,
        text_signature="($module, /, name, globals=None, locals=None, fromlist=(),\n           level=0)",
    ),
)

BUILTIN_FUNC_SPECS: dict[str, BuiltinFuncSpec] = {
    PUBLIC_OBJECTS[spec.runtime].name: spec for spec in _BUILTIN_FUNC_SIGNATURES
}

# Public intrinsic signatures own their Python call shape once. Publication,
# intrinsic materialization and named lowering all consume that declaration.
for _runtime, _call in PUBLIC_CALL_SIGNATURES.items():
    _identity = PUBLIC_OBJECTS[_runtime]
    if _identity.module != "builtins":
        continue
    if _identity.name in BUILTIN_FUNC_SPECS:
        raise ValueError(f"duplicate builtin signature authority: {_identity.name}")
    BUILTIN_FUNC_SPECS[_identity.name] = BuiltinFuncSpec(
        _runtime,
        _call.posonly,
        tuple(ast.parse(value, mode="eval").body for value in _call.defaults),
        vararg=_call.vararg,
        varkw=_call.varkw,
        pos_or_kw_params=_call.positional,
        kwonly_params=_call.kwonly,
        kw_defaults=tuple(
            ast.parse(value, mode="eval").body for value in _call.kwdefaults
        ),
    )


def builtin_func_declared_arity(spec: BuiltinFuncSpec) -> int:
    arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
    if spec.vararg is not None:
        arity += 1
    if spec.varkw is not None:
        arity += 1
    return arity


__all__ = [
    "BuiltinFuncSpec",
    "BUILTIN_FUNC_SPECS",
    "BuiltinBindingPolicy",
    "_MOLT_MISSING",
    "builtin_func_declared_arity",
]
