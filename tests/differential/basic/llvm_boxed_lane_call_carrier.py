# Regression: the LLVM boxed-lane dynamic-call ABI must NaN-box raw scalar
# arguments before handing them to the runtime dispatch path (`molt_call_func_
# fast{N}` / `molt_call_bind` / `molt_call_bind_ic`). The callee trampoline
# decodes each argument as a NaN-boxed `DynBox` into its parameter's raw
# representation; passing a raw `I64`/`F64` made the trampoline read the raw
# payload as a boxed tag, so an int surfaced as a denormal float / `15.0`.
#
# Bug class (molt tasks #58 / #37): the dynamic-call arg marshalling in the LLVM
# backend (`emit_call_func_runtime` / `emit_call_bind_runtime` and the
# `call_method` path) used a raw `ensure_i64` cast instead of
# `materialize_dynbox_operand`. The direct-call path and `call_builtin` already
# boxed correctly — this closes the asymmetric coverage gap on the dynamic path.
#
# Also covers #61: `frozenset([...])` construction (the LLVM-only missing
# `frozenset_new` lowering arm) returned `None` entirely.
#
# Every shape below is byte-identical across CPython 3.12 / 3.13 / 3.14.


# ── Closure called within its defining scope (non-inlined dynamic dispatch) ──
def closure_returns_arg(base):
    def ident(x):
        return x

    return ident(7)


def closure_base_plus(base):
    def add(x):
        return base + x

    return add(10)


# ── Method call carrying a raw-int argument (call_bind_ic path) ──
class Calc:
    def __init__(self, bias):
        self.bias = bias

    def add(self, value):
        return self.bias + value

    def echo(self, value):
        return value


# ── Builtins whose int result must stay int through the call boundary ──
def sum_of_list():
    return sum([0, 1, 2, 3, 4, 5])


def format_of_int():
    return format(42)


def main() -> None:
    print(closure_returns_arg(5))          # 7
    print(closure_base_plus(5))            # 15

    c = Calc(100)
    print(c.add(10))                       # 110
    print(c.echo(7))                       # 7
    print(c.add(2) + c.echo(3))            # 105

    print(sum_of_list())                   # 15
    print(format_of_int())                 # 42

    # frozenset construction must not collapse to None.
    fs = frozenset([1, 2, 3])
    print(type(fs).__name__)               # frozenset
    print(len(fs))                         # 3
    print(sorted(fs))                      # [1, 2, 3]
    print(2 in fs, 9 in fs)                # True False

    # Float carrier through the same dynamic boundary.
    def scale(x):
        return x * 2.0

    print(scale(3.5))                      # 7.0


if __name__ == "__main__":
    main()
