"""Purpose: verify TypeVar default support is version-gated for Python 3.13+."""

import sys
import typing


if sys.version_info >= (3, 13):
    typing_mod = __import__("typing")
    typevar = getattr(typing_mod, "TypeVar")
    t = typevar("T")
    u = typevar("U", default=int)
    no_default = getattr(typing_mod, "NoDefault")
    print("typevar_default_supported", True)
    print("NoDefault", no_default)
    print(
        "T_has_default",
        getattr(t, "has_default")(),
        getattr(t, "__default__") is no_default,
    )
    print("U_has_default", getattr(u, "has_default")(), getattr(u, "__default__"))
else:
    print("typevar_default_supported", False)
    try:
        typing.TypeVar("U", default=int)
    except TypeError as exc:
        print(type(exc).__name__, str(exc))
    else:
        print("unexpected_default_accept")
