"""LLVM-lane regression: a preserved-`Copy` operator inside an inlined callee.

The SimpleIR->TIR lift maps several operator kinds the frontend emits — among
them `floordiv` (`a // b`) — to `OpCode::Copy` with `_original_kind` preserved,
rather than to a dedicated opcode. The native/Cranelift and WASM lanes consume
SimpleIR (where these are real op kinds) and are unaffected; the LLVM lane lowers
the TIR directly, where the op arrives as a `Copy`. Without an explicit lowering
the generic `Copy` handler fell through to "pass through operand 0", silently
replacing `a // b` with `a` AND dropping the ZeroDivisionError the operator
raises on a zero divisor — a silent miscompile of the operator.

`div` is an observation-only callee inlined into `safe_div`, so the spliced
floordiv runs inside `safe_div`'s `try/except`; the except clause must still see
the ZeroDivisionError raised by the inlined `//`. This pins both the value path
(`safe_div(10, 2) == 5`, not `10`) and the exception path (`safe_div(10, 0)`
caught, returning `-1`, not `10`).
"""


def div(a: int, b: int) -> int:
    return a // b


def safe_div(a: int, b: int) -> int:
    try:
        return div(a, b)
    except ZeroDivisionError:
        return -1


def main() -> None:
    print(safe_div(10, 2))  # 5
    print(safe_div(10, 0))  # -1
    print(safe_div(-7, 2))  # -4  (floor division rounds toward -inf)
    print(safe_div(100, 7))  # 14


main()
