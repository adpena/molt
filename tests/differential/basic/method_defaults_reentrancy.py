# Pins the compiled-body liveness-retention invariant that closes the
# call_direct borrowed-__defaults__ reentrancy window (IC fix bee1d1418).
#
# call_direct pads its stack argv with BORROWED references to elements of the
# live __defaults__ tuple. A method that reassigns its own class's
# __defaults__ mid-call frees the old tuple; the in-flight call must keep the
# old element alive and intact (CPython binds defaults at call time).
#
# Round-2 adversarial review (2026-06-05) proved the window is closed ONLY
# because native codegen retains (increfs) a borrowed param that is live
# across a freeing call. Nothing in call_direct itself enforces that. If a
# future borrow/last-use optimization elides the retention for a param
# "obviously" read once, THIS test is the tripwire: the freed default gets
# recycled by the heap churn below and the output diverges (or aborts).

class C:
    def m(self, x, payload="old-" + str(11)):
        # Frees the previous __defaults__ tuple while `payload` (borrowed
        # from it) is still live in this frame.
        C.m.__defaults__ = ("new-" + str(x),)
        # Heap churn so a prematurely freed payload block gets recycled
        # (turns a silent stale read into observable corruption).
        churn = ["x" * 32 for _ in range(64)]
        return payload + ":" + str(len(churn))


class D:
    # Heap bigint payload: the shape the round-2 RC trace used (1 -> 2 incref
    # before the freeing store, 0 only after last use).
    def g(self, x, payload=(1 << 80) + 7):
        D.g.__defaults__ = ((1 << 81) + x,)
        acc = 0
        for j in range(200):
            acc += j
        return payload + acc


c = C()
outs = []
for i in range(5):
    outs.append(c.m(i))  # default engaged every call; tuple replaced every call
print(outs)
print(C.m.__defaults__)

d = D()
vals = []
for i in range(4):
    vals.append(d.g(i))
print(vals)
print(D.g.__defaults__)
