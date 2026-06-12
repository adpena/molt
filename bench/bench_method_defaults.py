"""Benchmark: defaults-bearing method call 5M.

A method with a positional default (``bump=1``) is called with the default
engaged every iteration.  This is the workload the ``__defaults__``-mutation
deopt guard devirtualizes: the call site knows the callee and the literal
default, the function's defaults version stays 0 (never mutated), so each call
takes the baked-literal fast path (a direct compiled CALL with the inlined
default) instead of runtime IC dispatch into a dynamic defaults bind.
"""


class Obj:
    def m(self, x, bump=1):
        return x + bump


N = 5_000_000
o = Obj()
total = 0
i = 0
while i < N:
    total = o.m(total)  # total + 1, default engaged every iteration
    i = i + 1
print(total)
