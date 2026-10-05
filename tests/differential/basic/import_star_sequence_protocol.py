"""Actual compiled IMPORT_STAR operation; no dynamic exec oracle shortcut."""
# ruff: noqa: E402, F403, F405
import json
import os
import sys
sys.path.insert(0, os.path.dirname(__file__))
import import_star_protocol_mod as _source

_rows = []
_namespace = globals()
for _case in (
    "generator_all", "set_all", "indexed_only", "invalid_all_partial",
    "getattr_value", "nonstring_dict_key", "dynamic_all",
    "index_error_propagation", "stop_iteration_propagation",
    "index_error_subclass", "all_lookup_nonattribute_error", "mapping_all",
    "deleted_name_invalid_all", "integer_name_invalid_all",
    "package_indexed_only", "dynamic_package_indexed_only",
    "path_lookup_nonattribute_error",
):
    _source.configure(_case)
    for _key in ("alpha", "x", "_hidden", "dynamic"):
        _namespace.pop(_key, None)
    try:
        from import_star_protocol_mod import *
        _error = None
    except Exception as _exc:
        _error = {"type": type(_exc).__name__, "message": str(_exc)}
    _bindings = {key: repr(_namespace[key]) for key in ("alpha", "x", "_hidden", "dynamic") if key in _namespace}
    _rows.append({"case": _case, "error": _error, "bindings": _bindings})
# __import__ prepares fromlist children only for packages. Ordinary modules
# return unchanged even when a truthy fromlist is not iterable or contains ints.
_source.configure("indexed_only")
for _label, _fromlist in (
    ("module_named_fromlist", ["x"]),
    ("module_star_fromlist", ["*"]),
    ("module_invalid_fromlist_item", [1]),
    ("module_noniterable_fromlist", 1),
    ("module_fromlist_iteration_forbidden", _source.IndexedOnly()),
):
    try:
        _imported = __import__("import_star_protocol_mod", fromlist=_fromlist)
        _error = None
        _bindings = {"same_module": repr(_imported is _source)}
    except Exception as _exc:
        _error = {"type": type(_exc).__name__, "message": str(_exc)}
        _bindings = {}
    _rows.append({"case": _label, "error": _error, "bindings": _bindings})

# Empty fromlists must not probe __path__ at all.
_source.configure("path_lookup_nonattribute_error")
try:
    _imported = __import__("import_star_protocol_mod", fromlist=())
    _error = None
    _bindings = {"same_module": repr(_imported is _source)}
except Exception as _exc:
    _error = {"type": type(_exc).__name__, "message": str(_exc)}
    _bindings = {}
_rows.append({"case": "empty_fromlist_skips_package_probe", "error": _error, "bindings": _bindings})
# Statically known bad calls must execute and remain catchable in the guest.
def _negative_level():
    return __import__("import_star_protocol_mod", level=-1)


def _missing_relative_globals():
    return __import__("import_star_protocol_mod", level=1)


def _invalid_relative_package():
    return __import__("import_star_protocol_mod", {"__package__": 42}, None, (), 1)


def _explicit_none_globals():
    return __import__("import_star_protocol_mod", globals=None, level=1)


def _positional_none_globals():
    return __import__("import_star_protocol_mod", None, None, (), 1)


def _empty_globals():
    return __import__("import_star_protocol_mod", {}, None, (), 1)


_import_alias = __import__


def _aliased_omitted_globals():
    return _import_alias("import_star_protocol_mod", level=1)


def _expanded_none_globals():
    return _import_alias("import_star_protocol_mod", **{"globals": None, "level": 1})


for _label, _callback in (
    ("negative_level", _negative_level),
    ("missing_relative_globals", _missing_relative_globals),
    ("invalid_relative_package", _invalid_relative_package),
    ("explicit_none_globals", _explicit_none_globals),
    ("positional_none_globals", _positional_none_globals),
    ("empty_relative_globals", _empty_globals),
    ("aliased_omitted_globals", _aliased_omitted_globals),
    ("expanded_none_globals", _expanded_none_globals),
):
    try:
        _callback()
        _error = None
    except Exception as _exc:
        _error = {"type": type(_exc).__name__, "message": str(_exc)}
    _rows.append({"case": _label, "error": _error, "bindings": {}})

# The child has no explicit import edge: only the package's live __all__
# names it. This exercises call-star discovery and the actual guest transaction.
try:
    _package = __import__("import_star_pkg_all_child", fromlist=("*",))
    _error = None
    _bindings = {"child_value": repr(_package.child.VALUE)}
except Exception as _exc:
    _error = {"type": type(_exc).__name__, "message": str(_exc)}
    _bindings = {}
_rows.append({"case": "package_call_star_child", "error": _error, "bindings": _bindings})

# Relative statement failures must enter the same guest exception handlers.
_saved_package = __package__
__package__ = ""
try:
    from . import never_loaded
    _error = None
except ImportError as _exc:
    _error = {"type": type(_exc).__name__, "message": str(_exc)}
_rows.append({"case": "statement_no_parent", "error": _error, "bindings": {}})
__package__ = "single_package"
try:
    from .. import never_loaded
    _error = None
except ImportError as _exc:
    _error = {"type": type(_exc).__name__, "message": str(_exc)}
_rows.append({"case": "statement_beyond_top", "error": _error, "bindings": {}})
__package__ = _saved_package

print(json.dumps({"cases": _rows}, sort_keys=True))
