"""Purpose: LLVM generator codegen P0 — bare creation, consumption, and the
drop/finalization paths. Regression for the SIGSEGV-on-generator-creation bug
(LLVM `OpCode::AllocTask` stored the frame payload through the NaN-BOXED task
handle instead of the unboxed heap pointer — `build_int_to_ptr(task_bits)` with
the QNAN|TAG_PTR top-16 bits intact → write through `0x7FFC…`-tagged garbage →
crash at `g(n)` creation, before any element is produced). Fix: unbox via
`unbox_ptr_bits` before the field GEP, mirroring native `unbox_ptr_value(obj)`.

Covers, in one program: bare creation (never consumed), create-and-drop
(generator goes out of scope unconsumed → destructor/frame-free path), full
consumption via list(), partial consumption then drop (RC-sensitive — the
generator and its in-flight yielded element must be released exactly once), and
a `yield` inside a loop. Byte-identical to CPython 3.14 on BOTH native and LLVM.
"""


def gen_ints(n):
    i = 0
    while i < n:
        yield i
        i = i + 1


def gen_strings(n):
    for i in range(n):
        yield "s" + str(i)


# 1. Bare creation — never consumed (exercises AllocTask frame-payload stores).
g_bare = gen_ints(5)
print("created", type(g_bare).__name__)


# 2. Create-and-drop — generator created in a frame, never consumed, then the
#    name is rebound so the unconsumed generator is dropped (frame free path).
def create_and_drop():
    local = gen_ints(3)
    local = None  # drop the unconsumed generator
    return "dropped"


print(create_and_drop())


# 3. Full consumption via list().
print(list(gen_strings(5)))
print(list(gen_ints(4)))


# 4. Partial consumption then drop — pull two via next(), then drop the
#    generator while it is suspended (RC-sensitive: suspended frame + the
#    most-recently-yielded element ownership).
def partial_then_drop():
    g = gen_strings(10)
    a = next(g)
    b = next(g)
    g = None  # drop while suspended at the 3rd yield
    return a, b


print(partial_then_drop())


# 5. yield inside a loop, consumed by a manual accumulation loop.
#
# NOTE (orthogonal pre-existing LLVM gap, intentionally NOT exercised here):
# `sum(<generator>)` — and `sum([list])` / `sum(range(...))` — currently prints
# `15.0` instead of `15` on the LLVM target (an int-accumulator that decays to
# float in `sum()`'s reduction codegen). It is unrelated to generator creation
# (it reproduces on `sum([0,1,2,3,4,5])` with no generator at all) and is tracked
# separately. A manual `total += v` loop and `max()`/`min()`/`tuple()` are all
# byte-identical on LLVM, so the generator-consumption-in-a-loop shape is pinned
# below without depending on the orthogonal `sum()` bug.
def manual_total(n):
    total = 0
    for v in gen_ints(n):
        total = total + v
    return total


print(manual_total(6))
print(max(gen_ints(6)))
