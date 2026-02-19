"""`json.__main__` compatibility shim.

In CPython, `json.__main__` exists starting in 3.14.
Version-gated absence for earlier versions is handled at importlib boundary.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

import json

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())

if __name__ == "__main__":
    import json.tool as _tool

    _tool.main()
