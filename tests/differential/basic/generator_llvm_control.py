"""Purpose: LLVM generator codegen P0 — control-flow-heavy generator shapes that
share the AllocTask frame-allocation machinery fixed for the creation SIGSEGV:
generators with try/finally (finalization on exhaustion AND on close()), a
generator consuming another generator (nested), and the send() protocol.
Byte-identical to CPython 3.14 on BOTH native and LLVM.

This complements generator_llvm_lifecycle.py: that one pins creation/drop, this
one pins the suspend/resume state machine and the explicit generator protocol
once a frame has been correctly allocated.

NOTE (orthogonal pre-existing gap, intentionally NOT exercised here): gen.throw()
that resumes into a `try/except` *inside the generator body* is currently broken
INDEPENDENTLY OF THIS FIX and on BOTH backends — native miscompiles it (the
injected exception is dropped: a re-`next()`-style `tick` is produced instead of
the `except` branch's `caught:…`), and LLVM fails module verification with an
SSA "Instruction does not dominate all uses!" on the in-handler closure-load phi
(the generator exception-resumption state machine, not AllocTask). It is a
distinct, backend-independent generator-throw-resumption bug; tracked separately.
The send()/close()/try-finally protocol below does NOT touch that path.
"""


def with_finally(n):
    try:
        i = 0
        while i < n:
            yield i
            i = i + 1
    finally:
        print("finally-ran")


# try/finally — finalization on normal exhaustion.
print(list(with_finally(3)))


# try/finally — finalization triggered by an early close().
def close_triggers_finally():
    g = with_finally(10)
    print(next(g))
    print(next(g))
    g.close()
    return "closed"


print(close_triggers_finally())


def producer(n):
    for i in range(n):
        yield i * i


def relay(g):
    # Nested: a generator consuming another generator.
    for v in g:
        yield v + 1


print(list(relay(producer(5))))


# send(): a generator that receives values.
def echo():
    received = yield "ready"
    while True:
        received = yield ("got:" + str(received))


def drive_send():
    g = echo()
    first = next(g)            # prime → "ready"
    a = g.send(10)             # → "got:10"
    b = g.send("x")            # → "got:x"
    g.close()
    return first, a, b


print(drive_send())
