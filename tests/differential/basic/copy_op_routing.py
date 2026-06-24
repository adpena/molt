"""Purpose: native-backend `copy` op must be ROUTED to its codegen handler.

`{kind:"copy"}` ops (frontend `COPY` — conditional-expression branch merges,
cached-call results, fast/slow specialization merges) reach native codegen
whenever their result or source is a reassigned-local (mutable storage), so they
survive `rewrite_copy_aliases`.

Regression for a native-backend dispatch-routing drift: the dispatch routes each
op to its family handler via `fc::native_op_family`, a kind->family map built
from every handler's `HANDLED_KINDS` authority. `copy` was present in
`fc::value_transfer::handle_value_transfer_op`'s match arm (and the dispatch
comment) but ABSENT from `fc::value_transfer::HANDLED_KINDS`, so
`native_op_family("copy")` returned `None`, copy matched no family guard, and it
fell through to the loud-panic catch-all for result-producing kinds. Every
surviving copy op was therefore unrouted (a silent-wrong-answer / abort class).
This exercises the common copy-emitting shapes against CPython so the routing
stays wired and the arm<->HANDLED_KINDS authority cannot silently diverge again.
"""


def ternary_reassigned_source(cond):
    # `r` is reassigned (mutable storage), so the conditional-expression COPY
    # that reads `r` survives `rewrite_copy_aliases` to codegen.
    r = 100
    if cond:
        r = 5
    v = r if cond else 7
    return v


def ternary_accumulate(n):
    acc = 0
    for i in range(n):
        # The conditional expression emits a COPY merging the two branch values;
        # `acc` is a reassigned-local so the copy survives.
        acc = acc + (i if i % 2 == 0 else -i)
    return acc


def nested_ternary(a, b):
    x = 1
    x = (a if a > b else b) if a + b > 0 else (a - b)
    return x


def ternary_str(flag):
    s = "init"
    s = "yes" if flag else "no"
    return s + "!"


print(ternary_reassigned_source(True))
print(ternary_reassigned_source(False))
print(ternary_accumulate(10))
print(ternary_accumulate(0))
print(nested_ternary(5, 3))
print(nested_ternary(-5, -3))
print(ternary_str(True))
print(ternary_str(False))
