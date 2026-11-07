"""CPython marshal data format backed by Molt's shared runtime codec.

Code records cannot cross the AOT boundary; see docs/design/marshal.md.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_dumps = _require_intrinsic("molt_marshal_dumps")
_loads = _require_intrinsic("molt_marshal_loads")
_dump = _require_intrinsic("molt_marshal_dump")
_load = _require_intrinsic("molt_marshal_load")
version = _require_intrinsic("molt_marshal_version")()
_target = _require_intrinsic("molt_sys_version_info")()[:2]


if _target >= (3, 13):
    def dumps(value, version=version, /, *, allow_code=True):
        bool(allow_code)
        return _dumps(value, version)

    def loads(bytes, /, *, allow_code=True):
        bool(allow_code)
        return _loads(bytes)

    def dump(value, file, version=version, /, *, allow_code=True):
        bool(allow_code)
        return _dump(value, file, version)

    def load(file, /, *, allow_code=True):
        bool(allow_code)
        return _load(file)
else:
    def dumps(value, version=version, /):
        return _dumps(value, version)

    def loads(bytes, /):
        return _loads(bytes)

    def dump(value, file, version=version, /):
        return _dump(value, file, version)

    def load(file, /):
        return _load(file)


__all__ = ["dump", "load", "dumps", "loads", "version"]
del _require_intrinsic, _target
