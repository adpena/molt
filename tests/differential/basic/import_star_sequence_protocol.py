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
print(json.dumps({"cases": _rows}, sort_keys=True))
