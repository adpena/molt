"""LLVM TIR inliner module-phase activation (E1 phase e) regression.

The LLVM lane runs the SAME `run_module_pipeline` (CallGraph -> ModuleSummaries
-> E1 inliner -> slot promotion) the native/wasm lanes run, on its own TIR
functions, and lowers the inlined `TirModule` directly to LLVM IR with NO
SimpleIR round-trip. Before activation the LLVM lane consumed the inliner's
output only indirectly (via the Cranelift SimpleIR module phase) and re-lifted
the merged bodies through a redundant second per-function pipeline.

This program exercises three axes that activation must preserve byte-for-byte:

1. Direct-call inlining in a hot loop: `add1` is a tiny exception-free,
   non-recursive leaf called 1,000,000 times inside `run`. The inliner splices
   its body (`x + 1`) into the loop, so the emitted LLVM `run` has no call to
   `add1` at all (verified separately via MOLT_LLVM_DUMP_IR). Output must match
   CPython exactly.

2. The BigInt boundary (the trusted-unbox non-regression): inlining must NEVER
   promote a fresh spliced value to a raw-i64 carrier without a value-range
   proof. `mul(1 << 60, 7)` and `mul(1 << 50, 1 << 50) == 2 ** 100` flow through
   an inlined `apply`/`mul` and must stay BigInt-correct (no 47-bit truncation).

3. Exception propagation through an inlined observation-only callee: `div` has
   no handler of its own (observation-only) and is inlined into `safe_div`,
   whose `try/except` must still catch the ZeroDivisionError raised inside the
   spliced body.
"""


def add1(x: int) -> int:
    return x + 1


def run() -> int:
    total = 0
    i = 0
    while i < 1_000_000:
        total = add1(total)
        i += 1
    return total


def mul(a: int, b: int) -> int:
    return a * b


def apply(f, x: int, y: int) -> int:
    return f(x, y)


def div(a: int, b: int) -> int:
    return a // b


def safe_div(a: int, b: int) -> int:
    try:
        return div(a, b)
    except ZeroDivisionError:
        return -1


def main() -> None:
    # 1) Hot direct-call leaf inlining.
    print(run())  # 1000000

    # 2) BigInt boundary through an inlined indirect-call chain.
    print(apply(mul, 1 << 60, 7))  # 8070450532247928832
    print(apply(mul, 3, 4))  # 12
    print(mul(1 << 50, 1 << 50) == 2 ** 100)  # True

    # 3) Exception propagation through an inlined observation-only callee.
    print(safe_div(10, 2))  # 5
    print(safe_div(10, 0))  # -1


main()
