"""Observe the integer math module and its reexported builtins."""

import importlib
import math
import pickle
import sys


if sys.version_info >= (3, 15):
    import _math_integer

    import math.integer as static_integer
    from math import integer as attribute_integer

    integer = importlib.import_module("math.integer")
    print("integer module", integer is math.integer is _math_integer, integer.__name__)
    print(
        "import forms",
        static_integer is integer,
        attribute_integer is integer,
        sys.modules["math.integer"] is integer,
    )
    for name, args in (
        ("comb", (7, 3)),
        ("factorial", (7,)),
        ("gcd", (14, 21)),
        ("isqrt", (26,)),
        ("lcm", (14, 21)),
        ("perm", (7, 3)),
    ):
        value = getattr(math, name)
        print(
            name,
            value is getattr(integer, name),
            value.__module__,
            pickle.loads(pickle.dumps(value)) is value,
            value(*args),
        )
else:
    print("integer module present", hasattr(math, "integer"))
