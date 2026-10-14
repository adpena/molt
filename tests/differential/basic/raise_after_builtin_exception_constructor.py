"""Purpose: a pending exception from an earlier call reaches the handler before a later raise.

Guards check elision after builtin exception constructors (HF-214).
"""

def g():
    raise KeyError("from g")


def f():
    try:
        raise ValueError(g())
    except Exception as e:
        return type(e).__name__, e.args


print(f())
