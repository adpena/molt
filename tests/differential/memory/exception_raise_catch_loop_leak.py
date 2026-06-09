# RC leak regression (Tier-2 #77, exception-heavy 0.68× root cause): a raised
# AND immediately-caught exception inside a hot loop must reach refcount 0 and be
# FREED on every iteration — exactly like CPython, where the `except ... as e`
# handler owns one reference, releases it at handler exit (implicit `del e`), and
# the exception-state slots release their own references when the handler is left.
#
# molt's defect (diagnosed in project_exception_loop_leak_baton.md): a function
# that contains a try/except handler is NEVER processed by the TIR drop-insertion
# pass (drop_insertion.rs bails on `has_exception_handlers()`, because its
# dominance-based liveness is unsound over exception CFG). In such a function the
# native value-tracking RC is the only release mechanism — but the exception-
# object-producing ops (`exception_new*` for the creation reference, and
# `exception_last_pending` for the handler's matching/binding reference) register
# their OWNED results in NEITHER `block_tracked_obj` NOR `tracked_obj_vars`, so
# the existing `check_exception` diverted-control drain never releases them. Net
# per raised-and-caught exception: 3 inc_ref, only 2 dec_ref → the object ends at
# refcount 2 (the creation ref + the exception_last_pending ref) and is leaked.
# This is BOTH the bench_exception_heavy 0.68× churn (inc_ref/dec_ref ~22% of
# cycles, re-allocating a fresh ValueError every iteration while the old ones pile
# up) AND the ~70 MiB / 30-inner-iteration object leak #76 measured.
#
# At 500k raises every exception leaks (live_objects ≈ 500k); MOLT_ASSERT_NO_LEAK
# (live <= 200_000) trips. A correct implementation frees each exception per
# iteration, so live_objects stays O(1) and the assertion passes.
#
# Three handler shapes are covered so the fix cannot regress one while passing the
# others: (1) `except ... as e` with a body that reads the bound name (the
# exception_last_pending binding ref); (2) `except T:` with no `as` binding (still
# leaks via the unreleased creation + match references — proving the leak is NOT
# the `del e` lowering); (3) a re-raise/`from` chain that legitimately keeps the
# inner exception alive via __context__ and must NOT be over-freed.


def raise_catch_as(n):
    total = 0
    for i in range(n):
        try:
            raise ValueError(i)
        except ValueError as e:
            total += int(str(e))
    return total


def raise_catch_no_as(n):
    count = 0
    for i in range(n):
        try:
            raise KeyError(i)
        except KeyError:
            count += 1
    return count


def raise_catch_chain(n):
    handled = 0
    for i in range(n):
        try:
            try:
                raise ValueError(i)
            except ValueError as inner:
                raise RuntimeError("wrap") from inner
        except RuntimeError as outer:
            # The chained __cause__/__context__ must stay reachable here (CPython
            # parity) and be released when both handlers exit — not leaked, not
            # double-freed.
            if outer.__cause__ is not None:
                handled += 1
    return handled


print(raise_catch_as(200000))
print(raise_catch_no_as(200000))
print(raise_catch_chain(120000))
