"""Purpose: a module-level function called directly with a CONSTANT positional
default must observe a runtime ``func.__defaults__`` reassignment.

CPython binds ``__defaults__`` at call time.  molt devirtualizes a direct call
``f(i)`` to a compiled CALL and (pre-fix) baked the literal default at compile
time -- so a later ``f.__defaults__ = (...)`` reassignment was SILENTLY IGNORED.
This is the pre-existing module-level divergence the method-devirt deopt guard
also heals: the baked-const path is now guarded by the function's
``__defaults__`` version stamp and falls back to a live read after any mutation.

Covers positional const default, ``__kwdefaults__`` for a kw-only const
default, a non-engaged default (explicit arg supplied), and a reset-then-mutate
sequence in a loop.

Byte-identical vs CPython 3.12 / 3.13 / 3.14.
"""


def add(x, bump=1):
    return x + bump


def scale(x, *, factor=10):
    return x * factor


def main() -> None:
    out = []

    # Baked-literal fast path before mutation.
    out.append(add(5))  # 6
    out.append(add(5, 2))  # 7 (explicit)

    # Reassign the positional default.
    add.__defaults__ = (100,)
    out.append(add(5))  # 105
    out.append(add(5, 2))  # 7 (explicit still wins)

    # Reassign kw-only default.
    out.append(scale(3))  # 30
    scale.__kwdefaults__ = {"factor": 4}
    out.append(scale(3))  # 12
    out.append(scale(3, factor=2))  # 6

    print(out)
    print("add.__defaults__:", add.__defaults__)
    print("scale.__kwdefaults__:", scale.__kwdefaults__)

    # Loop with a reset then a mid-loop mutation.
    add.__defaults__ = (1,)
    acc = []
    for i in range(8):
        if i == 4:
            add.__defaults__ = (-7,)
        acc.append(add(i))
    print("loop:", acc)


if __name__ == "__main__":
    main()
