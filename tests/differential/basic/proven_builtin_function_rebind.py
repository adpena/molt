"""A code object re-bound to other globals uses that frame's builtins (HF-226).

``types.FunctionType`` gives a function's code another globals mapping and so
another builtins mapping. The closed-world proof for the original definition
must not apply to that activation.
"""

import types


def classify(value):
    return isinstance(value, str), len([1, 2])


rebound = types.FunctionType(
    classify.__code__,
    {
        "__builtins__": {
            "isinstance": lambda obj, kind: "rebound",
            "len": lambda obj: -1,
            "str": str,
        }
    },
)

print("original", classify("a"))
print("rebound", rebound("a"))
