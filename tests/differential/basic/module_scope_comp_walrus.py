"""Regression for #45 item 3 — comprehension walrus target also bound outside the
comprehension, at MODULE scope (the sibling of the function-scope fix d19dfa588).

A walrus (``:=``) inside a comprehension leaks its binding to the enclosing scope
(PEP 572).  At module scope the single storage authority for a name is the module
dict (MODULE_SET_ATTR / MODULE_GET_ATTR / MODULE_GET_GLOBAL), not a boxed function
cell: other functions read the global through the module dict, and module-scope SSA
refs dangle across chunk boundaries (3f5aa1135).  The inline list/set/dict
comprehension lowering used to box the walrus target into a transient cell and
read/write that cell, while a *separate* binding of the same name (a ``while``/``if``
test walrus, a plain assignment, a ``for`` target) writes through the module dict.
The two storage paths diverged: the comprehension read a stale/None cell instead of
the loop-carried module value, so ``while (n := next(it, None)) is not None: [n :=
n + 1 for _ in range(3)]`` raised ``TypeError: NoneType + int`` instead of running.

Fix (src/molt/frontend/__init__.py, _emit_inline_simple_comp): at module scope route
the comprehension walrus target through the module dict — exactly as the GeneratorExp
poll-fn path already does (visit_GeneratorExp) — instead of boxing it.  Reads route
through MODULE_GET_GLOBAL (LOAD_GLOBAL / NameError semantics) so a walrus read before
its first binding raises NameError (not the AttributeError of MODULE_GET_ATTR), and
writes go straight to the module dict so every binding site shares one storage cell.
The boxed-cell post-comp sync is skipped for module targets (the per-element module
write already keeps the dict authoritative).  Function scope is unchanged (it still
pre-boxes via _collect_comp_walrus_shared_names / _prebox_scope_cell_vars).

The chunk-split section below is op-dense enough that it crosses module chunk
boundaries (native default 1400 ops, src/molt/cli.py; WASM 2000) so the comp-walrus
shared name must survive across chunks via the module dict.  To exercise the same
shapes at a tight threshold deterministically, build/run with the lever, e.g.:

    MOLT_MODULE_CHUNK_OPS=120 python3 -m molt build --target native --output /tmp/o this_file.py

Runs byte-identical to CPython 3.12/3.13/3.14.
"""


# --- while-test walrus + comp walrus on the SAME name (list comp) ------------
it_a = iter([10, 20])
seen_a = []
while (n := next(it_a, None)) is not None:
    inner = [n := n + 1 for _ in range(3)]
    seen_a.append((inner, n))
print("A", seen_a, n)


# --- for-loop outer plain assign + comp walrus on the SAME name --------------
m = 0
inner_b = []
for _ in range(2):
    m = m + 100
    inner_b = [m := m + 1 for _ in range(3)]
print("B", m, inner_b)


# --- distinct comp-walrus name must stay independent (no over-unification) ---
it_c = iter([10, 20])
last_c = None
while (p := next(it_c, None)) is not None:
    inner_c = [q := x + 1 for x in range(p % 3)]
    last_c = (p, inner_c, q)
print("C", last_c, p)


# --- comp-walrus-only target with no other writer (preceded by binding) ------
total_d = 0
for _ in range(2):
    vals_d = [y := v * v for v in range(3)]
    total_d += y
print("D", total_d, y, vals_d)


# --- nested while + comp walrus, tuple-unpack outer target ------------------
rows = iter([(1, 2), (3, 4)])
out_e = []
while (pair := next(rows, None)) is not None:
    a, b = pair
    squares = [(acc := a * k) for k in range(b)]
    out_e.append((squares, acc))
print("E", out_e, pair)


# --- interleaved reads via a helper fn reading the module global ------------
# The comp-walrus update must be visible to read_g(), and read_g()'s view must
# reflect the latest comp-walrus write (single storage authority = module dict).
g = 0
log_f = []


def read_g():
    return g


it_f = iter([5, 6])
while (g := next(it_f, None)) is not None:
    before = read_g()
    inner_f = [g := g + 10 for _ in range(2)]
    after = read_g()
    log_f.append((before, inner_f, after, g))
print("F", log_f, g)


# --- set comprehension walrus at module scope, dual-bound -------------------
it_g = iter([1, 2])
seen_g = []
while (s := next(it_g, None)) is not None:
    inner_g = {s := s + 1 for _ in range(2)}
    seen_g.append((sorted(inner_g), s))
print("G", seen_g, s)


# --- dict comprehension walrus at module scope, dual-bound ------------------
it_h = iter([1, 2])
seen_h = []
while (d := next(it_h, None)) is not None:
    inner_h = {k: (d := d + 1) for k in range(2)}
    seen_h.append((inner_h, d))
print("H", seen_h, d)


# --- comp walrus reads its own update mid-comprehension chain ---------------
base = 0
chain_i = [base := base + i for i in range(5)]
print("I", chain_i, base)


# --- genexpr walrus at module scope (already-handled path; parity baseline) --
it_j = iter([2, 3])
seen_j = []
while (w := next(it_j, None)) is not None:
    inner_j = list(w := w + 1 for _ in range(2))
    seen_j.append((inner_j, w))
print("J", seen_j, w)


# --- comp walrus inside nested module-scope control flow, dual-bound --------
it_l = iter([1, 2, 3])
out_l = []
while (vl := next(it_l, None)) is not None:
    if vl % 2 == 1:
        inner_l = [vl := vl * 10 for _ in range(2)]
        out_l.append(("odd", inner_l, vl))
    else:
        out_l.append(("even", vl))
print("L", out_l, vl)


# --- comp-walrus target that shadows a builtin name, dual-bound -------------
# A bound builtin-named global resolves via the module dict (the comp-walrus
# write); only a read *before* any binding would fall back to the builtin.
max = 0  # noqa: A001 - intentionally shadow the builtin to test name routing
out_n = []
it_n = iter([3, 4])
while (max := next(it_n, None)) is not None:
    inner_n = [max := max + 100 for _ in range(2)]
    out_n.append((inner_n, max))
print("N", out_n, max)


# --- nested function reading+writing the module global a comp-walrus binds ---
acc_m = 100
trace_m = []


def bump_m():
    global acc_m
    acc_m += 1
    return acc_m


it_m = iter([1, 2])
while (acc_m := next(it_m, None)) is not None:
    pre = bump_m()  # reads + writes module acc_m (now the loop value + 1)
    inner_m = [acc_m := acc_m + 5 for _ in range(2)]
    trace_m.append((pre, inner_m, acc_m))
print("M", trace_m, acc_m)


# --- chunk-split section: op-dense so the shared comp-walrus name crosses ----
# module chunk boundaries.  The module-dict route is the chunk-safe one; a
# boxed-cell/SSA route would dangle here (3f5aa1135).
z0 = 0
z1 = z0 + 1
z2 = z1 + 1
z3 = z2 + 1
z4 = z3 + 1
z5 = z4 + 1
z6 = z5 + 1
z7 = z6 + 1
z8 = z7 + 1
z9 = z8 + 1
z10 = z9 + 1
z11 = z10 + 1
z12 = z11 + 1
z13 = z12 + 1
z14 = z13 + 1
z15 = z14 + 1
z16 = z15 + 1
z17 = z16 + 1
z18 = z17 + 1
z19 = z18 + 1

ck = z19 * 0  # ck starts at 0; dual-bound below
trace_ck = []
it_ck = iter([1, 2, 3])
while (ck := next(it_ck, None)) is not None:
    inner_ck = [ck := ck + 7 for _ in range(2)]
    trace_ck.append((inner_ck, ck))
term_ck = ck  # loop terminator (None) — no arithmetic on it

# Many ops AFTER the loop that re-bind and read the same name, forcing a chunk
# boundary between a comp-walrus store and later reads of that name.
ck = 1000
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1
ck = ck + 1


def read_ck_from_fn():
    return ck


print("CHUNK", trace_ck, term_ck, ck, read_ck_from_fn())
