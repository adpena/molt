"""Purpose: collections.Counter must reject unhashable keys like CPython.

molt's Counter uses an obj_eq registry that never hashed keys, so it silently
accepted unhashable keys (a divergence). Now: the element-counting path
(Counter(iter)) raises the bare 'unhashable type: X'; direct key access
(c[k], c[k]=v, del c[k], k in c) raises the dict-key error (3.14 adds the
'cannot use X as a dict key' context). Hashable keys keep working.

NOTE: exercised at MODULE scope (needs_exception_stack=True), not inside lambdas.
An exception raised by an intrinsic call inside a needs_exception_stack=False
function (e.g. a bare lambda) does not propagate cleanly through the call/stdlib
boundary -- the separately-tracked needs-stack propagation gap (see the
iter-consume-hang baton). The unhashable *message* under test is identical
regardless of consumer context, so module scope validates the fix faithfully.
"""

from collections import Counter

try:
    Counter([[]])
except Exception as e:
    print("ctor_unhashable", type(e).__name__, str(e))

c = Counter()
try:
    c[[]]
except Exception as e:
    print("getitem", type(e).__name__, str(e))
try:
    c[[]] = 1
except Exception as e:
    print("setitem", type(e).__name__, str(e))
try:
    del c[[]]
except Exception as e:
    print("delitem", type(e).__name__, str(e))
try:
    [] in c
except Exception as e:
    print("contains", type(e).__name__, str(e))

# hashable keys unaffected
c2 = Counter([1, 1, 2, 3, 3, 3])
print("counts", c2[3], c2[1], c2[99], (1 in c2), (99 in c2))
c2[5] = 10
print("setget", c2[5])
del c2[1]
print("afterdel", c2[1], (1 in c2))
print("DONE")
