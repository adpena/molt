"""Purpose: verify TypeVar default support is version-gated for Python 3.13+."""

import sys
import typing


if sys.version_info >= (3, 13):
    t = typing.TypeVar("T")
    u = typing.TypeVar("U", default=int)
    print("typevar_default_supported", True)
    print("NoDefault", typing.NoDefault)
    print("T_has_default", t.has_default(), t.__default__ is typing.NoDefault)
    print("U_has_default", u.has_default(), u.__default__)
else:
    print("typevar_default_supported", False)
    try:
        typing.TypeVar("U", default=int)
    except TypeError as exc:
        print(type(exc).__name__, str(exc))
    else:
        print("unexpected_default_accept")
