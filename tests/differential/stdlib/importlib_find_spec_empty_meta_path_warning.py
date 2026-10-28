"""Purpose: an empty sys.meta_path warns through _warnings, never imported here.

CPython 3.12 importlib._bootstrap._find_spec calls
_warnings.warn("sys.meta_path is empty", ImportWarning) and then returns None.
This program never imports _warnings itself, so the compiled import protocol
must carry that support module.
"""

import importlib.util
import sys
import warnings

saved_meta_path = sys.meta_path[:]
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    sys.meta_path[:] = []
    try:
        result = importlib.util.find_spec("_molt_empty_meta_path_target")
    finally:
        sys.meta_path[:] = saved_meta_path

print("result", result)
print("warnings", [(entry.category.__name__, str(entry.message)) for entry in caught])
print("restored", sys.meta_path == saved_meta_path)
