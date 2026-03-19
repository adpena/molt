"""The Zen of Python."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_THIS_PAYLOAD = _require_intrinsic("molt_this_payload")

_payload = _MOLT_THIS_PAYLOAD()
if not isinstance(_payload, tuple) or len(_payload) != 5:
    raise RuntimeError("this intrinsic returned invalid payload")

s, d, _decoded, c, i = _payload

if not isinstance(s, str):
    raise RuntimeError("this intrinsic returned invalid payload")
if not isinstance(d, dict):
    raise RuntimeError("this intrinsic returned invalid payload")
if not isinstance(_decoded, str):
    raise RuntimeError("this intrinsic returned invalid payload")
if not isinstance(c, int) or not isinstance(i, int):
    raise RuntimeError("this intrinsic returned invalid payload")

for _k, _v in d.items():
    if not isinstance(_k, str) or not isinstance(_v, str):
        raise RuntimeError("this intrinsic returned invalid payload")

print(_decoded)

globals().pop("_require_intrinsic", None)
