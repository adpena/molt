"""LLVM-lane regression: a heap-BigInt argument to an `int`-typed parameter.

A function parameter annotated `int` is a *semantic* int, not a proof that the
value fits a raw 47-bit inline NaN-box payload. On the LLVM lane the parameter
ABI must therefore carry an unprovable-range `int` parameter BOXED (`DynBox`),
exactly as the native lane carries every `int` parameter as a boxed word — the
caller passes the boxed value (an inline int OR a heap-BigInt pointer) unchanged
and the callee uses it boxed. Declaring the parameter a raw `i64` made the LLVM
lowering decode the boxed bits as a raw integer and re-box them, truncating a
heap BigInt (e.g. `1 << 60`) to its low 47 bits — a non-deterministic garbage
value derived from the pointer (the trusted-unbox bug-class, at the parameter
ABI rather than at an explicit unbox site).

`mul` is reached three ways here — a direct call with runtime BigInt args, an
indirect call through `apply` (the trampoline arg-decode path), and a folded
constant call — so the parameter-ABI carrier must be BigInt-correct on all of
them. `apply`'s indirect call additionally exercises the dynamic-dispatch
trampoline, which loads NaN-boxed args from the call-args array and must hand a
raw-`i64`-ABI parameter the same value-range-gated decode (boxed unprovable
ints pass through unchanged).
"""


def mul(a: int, b: int) -> int:
    return a * b


def apply(f, x: int, y: int) -> int:
    return f(x, y)


def main() -> None:
    big = 1 << 60
    small = 7
    # Direct call, runtime (non-constant-foldable) BigInt argument.
    print(mul(big, small))  # 8070450532247928832
    # Indirect call through the dynamic-dispatch trampoline.
    print(apply(mul, big, small))  # 8070450532247928832
    print(apply(mul, 3, 4))  # 12
    # 2 ** 100 — both operands heap BigInts.
    print(mul(1 << 50, 1 << 50) == 2 ** 100)  # True


main()
