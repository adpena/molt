"""Purpose: a constant expression that raises must raise AT RUNTIME, every
call, with the exact exception — it must NEVER be silently const-folded away
or dead-code-eliminated.

This is the structural regression for the task-#42 bug class. Two compiler
sites used to drop a raising constant expression and produce a SILENT
no-raise (the worst failure mode — wrong answer, no signal):

  1. The TIR `may_throw` oracle (`op_kinds.toml`) mis-classified `Shl`/`Shr`
     as fully `pure` and `Pow` as `pure_may_throw` BUT `may_throw = false`.
     DCE preserves a dead op only when the oracle says it may throw, so a
     dead `x = 1 << -1` / `y = 0 ** -1` (result unused, NO try block) was
     deleted — the `ValueError`/`ZeroDivisionError` vanished.

  2. SCCP's `eval_binary_pow` folded `float ** float` with a bare IEEE
     `powf`, so `0.0 ** -1.0` folded to `inf` (CPython raises
     ZeroDivisionError) and `(-1.0) ** 0.5` folded to `NaN` (CPython returns
     a `complex`). Both are silent miscompiles independent of any try block.

CPython's own peephole folder REFUSES to fold a constant that raises (it
catches the error at fold time and leaves the op for the runtime); molt must
do the same. The matrix below crosses every foldable raising shape with the
scope kinds whose lowering paths differ (module / function / method /
comprehension / lambda) and exercises BOTH the value-used and value-dead
(DCE-bait) spellings. Run byte-for-byte against CPython on native + llvm.

NOTE ON EXCEPTION MESSAGE TEXT: the exception *type* is asserted everywhere
(that is the fold/DCE contract this file guards). For the integer
`//`/`%`/`divmod`-by-zero and the zero-to-negative-power cases, molt's
runtime message currently tracks the CPython 3.13 wording while the harness
diffs against 3.14; that runtime message-version gap is a SEPARATE item, so
those two shapes assert on the type only and the full message is asserted for
every shape where molt is already byte-identical to the harness CPython.
"""


def show_exc(label, thunk, *, with_message=True):
    """Call ``thunk`` (which must raise), print the exception type — and the
    message when ``with_message`` — so the line is byte-comparable to CPython.
    A non-raising call is itself a failure signal printed verbatim."""
    try:
        result = thunk()
    except BaseException as exc:  # noqa: BLE001 — we re-print the type deliberately
        if with_message:
            print(label, type(exc).__name__, "|", str(exc))
        else:
            print(label, type(exc).__name__)
        return
    print(label, "DID-NOT-RAISE ->", repr(result))


# Shapes whose type AND message molt matches CPython byte-for-byte.
# (callable, label) — each MUST raise when evaluated.
TYPED_AND_MESSAGED = [
    (lambda: 1 << -1, "lshift_neg"),
    (lambda: 1 >> -1, "rshift_neg"),
    (lambda: ord(""), "ord_empty"),
    (lambda: ord("ab"), "ord_multi"),
    (lambda: chr(0x110000), "chr_over"),
    (lambda: (1,)[2], "tuple_oob"),
    (lambda: int("x"), "int_bad"),
]

# Shapes whose exception TYPE molt matches but whose message text is on the
# CPython-3.13 wording (a separate runtime message-version item): type only.
TYPED_ONLY = [
    (lambda: 1 // 0, "floordiv_zero"),
    (lambda: 1 % 0, "mod_zero"),
    (lambda: divmod(1, 0), "divmod_zero"),
    (lambda: 0.0 ** -1.0, "fpow_zero_neg"),
    (lambda: 0 ** -1, "ipow_zero_neg"),
]


def run_in_function(thunk, label, with_message):
    """Function scope — the original task-#42 repro lives here."""
    show_exc("fn:" + label, thunk, with_message=with_message)


class Holder:
    def run_in_method(self, thunk, label, with_message):
        """Bound-method scope — a distinct lowering path (self-bound frame)."""
        show_exc("method:" + label, thunk, with_message=with_message)


def main():
    # --- module scope (top-level statements inside main) --------------------
    for thunk, label in TYPED_AND_MESSAGED:
        show_exc("module:" + label, thunk, with_message=True)
    for thunk, label in TYPED_ONLY:
        show_exc("module:" + label, thunk, with_message=False)

    # --- function scope ----------------------------------------------------
    for thunk, label in TYPED_AND_MESSAGED:
        run_in_function(thunk, label, True)
    for thunk, label in TYPED_ONLY:
        run_in_function(thunk, label, False)

    # --- method scope ------------------------------------------------------
    holder = Holder()
    for thunk, label in TYPED_AND_MESSAGED:
        holder.run_in_method(thunk, label, True)
    for thunk, label in TYPED_ONLY:
        holder.run_in_method(thunk, label, False)

    # --- comprehension scope (the raising expr is evaluated per element) ----
    # Each element evaluation must raise; the comprehension aborts on the
    # first one, so we wrap the whole comprehension and assert it raised.
    show_exc("comp:lshift_neg", lambda: [1 << -1 for _ in range(3)])
    show_exc("comp:floordiv_zero", lambda: [1 // 0 for _ in range(3)], with_message=False)
    show_exc("comp:fpow_zero_neg", lambda: [0.0 ** -1.0 for _ in range(3)], with_message=False)
    show_exc("comp:chr_over", lambda: [chr(0x110000) for _ in range(3)])

    # --- lambda scope (a fresh code object / call frame) -------------------
    show_exc("lambda:lshift_neg", (lambda: 1 << -1))
    show_exc("lambda:floordiv_zero", (lambda: 1 // 0), with_message=False)
    show_exc("lambda:fpow_zero_neg", (lambda: 0.0 ** -1.0), with_message=False)
    show_exc("lambda:tuple_oob", (lambda: (1,)[2]))

    # --- DEAD-RESULT spellings (the DCE-drop bug) --------------------------
    # The result is assigned to an unused local with NO surrounding try, so
    # nothing keeps the op alive except its observable exception. DCE must
    # NOT delete it. Each helper MUST raise rather than return its sentinel.
    def dead_lshift():
        unused = 1 << -1  # noqa: F841 — intentionally dead, must still raise
        return "dead_lshift-NO-RAISE"

    def dead_rshift():
        unused = 1 >> -1  # noqa: F841
        return "dead_rshift-NO-RAISE"

    def dead_ipow():
        unused = 0 ** -1  # noqa: F841
        return "dead_ipow-NO-RAISE"

    def dead_fpow():
        unused = 0.0 ** -1.0  # noqa: F841
        return "dead_fpow-NO-RAISE"

    def dead_floordiv():
        unused = 1 // 0  # noqa: F841
        return "dead_floordiv-NO-RAISE"

    show_exc("dead:lshift", dead_lshift)
    show_exc("dead:rshift", dead_rshift)
    show_exc("dead:ipow", dead_ipow, with_message=False)
    show_exc("dead:fpow", dead_fpow, with_message=False)
    show_exc("dead:floordiv", dead_floordiv, with_message=False)

    # --- non-raising fold REGRESSION (folding must still work where safe) ---
    # These const exprs do NOT raise; the optimizer is free to fold them and
    # MUST still produce the exact value. Guards the "don't over-refuse" edge.
    print("safe:lshift", 1 << 10)
    print("safe:rshift", 1024 >> 3)
    print("safe:ipow", 2 ** 10)
    print("safe:fpow", 2.0 ** 0.5)
    print("safe:floordiv", 7 // 2)
    print("safe:mod", 7 % 3)
    print("safe:chr", chr(65))
    print("safe:ord", ord("A"))
    print("safe:tuple", (10, 20, 30)[1])
    print("safe:bigshift", 1 << 70)  # bigint result, still exact

    # --- I64-fast-lane divide-by-zero (the LLVM/WASM raw-divide bug) --------
    # A counted loop establishes proven-i64 reprs so the divide reaches the raw
    # machine `sdiv`/`srem` (LLVM) / `i64.div_s`/`i64.rem_s` (WASM) lane rather
    # than the boxed runtime. A raw divide by zero is poison (LLVM) / a trap
    # (WASM); the divisor-zero guard must instead raise ZeroDivisionError. Type
    # only (the runtime message wording is the separate version item).
    def loop_floordiv_zero():
        acc = 0
        j = 3
        while j >= 0:
            acc += 100 // j  # j reaches 0
            j -= 1
        return acc

    def loop_mod_zero():
        acc = 0
        j = 3
        while j >= 0:
            acc += 100 % j  # j reaches 0
            j -= 1
        return acc

    show_exc("loop:floordiv_zero", loop_floordiv_zero, with_message=False)
    show_exc("loop:mod_zero", loop_mod_zero, with_message=False)

    # Non-raising loop divide REGRESSION: a proven-non-zero divisor must still
    # take the (fast) raw lane and produce the exact value.
    def loop_floordiv_ok():
        acc = 0
        for k in range(1, 6):
            acc += 100 // k
        return acc

    print("safe:loopfloordiv", loop_floordiv_ok())


if __name__ == "__main__":
    main()
