"""Declarative Python builtin callable metadata, independent of generated ABI.

The frontend validates these declarations against generated runtime facts when
consuming them. Generators validate the same declarations against their raw
manifest, so missing or stale generated projections cannot block regeneration.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass


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


_MOLT_MISSING = ast.Name(id="__molt_missing__", ctx=ast.Load())


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
        ("value", "format_spec"),
        (ast.Constant(""),),
        text_signature="($module, value, format_spec='', /)",
    ),
    "callable": BuiltinFuncSpec(
        "molt_callable_builtin", ("obj",), text_signature="($module, obj, /)"
    ),
    "id": BuiltinFuncSpec("molt_id", ("obj",), text_signature="($module, obj, /)"),
    "enumerate": BuiltinFuncSpec(
        "molt_enumerate_builtin",
        (),
        (_MOLT_MISSING,),
        pos_or_kw_params=("iterable", "start"),
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
    "iter": BuiltinFuncSpec(
        "molt_iter_builtin", ("object", "sentinel"), (_MOLT_MISSING,)
    ),
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
        ("iterable",),
        kwonly_params=("key", "reverse"),
        kw_defaults=(ast.Constant(None), ast.Constant(False)),
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


def builtin_func_declared_arity(spec: BuiltinFuncSpec) -> int:
    arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
    if spec.vararg is not None:
        arity += 1
    return arity


__all__ = [
    "BuiltinFuncSpec",
    "BUILTIN_FUNC_SPECS",
    "MOLT_BIND_KIND_CLINIC_NAMED",
    "_MOLT_MISSING",
    "builtin_func_declared_arity",
]
