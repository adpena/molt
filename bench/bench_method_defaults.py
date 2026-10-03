"""Benchmark a method with a positional default, engaged on every call.

The default workload is five million calls. An optional iteration count permits
scaling and profiling the same executable without recompiling it. This workload
does not assume a particular dispatch optimization; inspect emitted code before
attributing its performance to a fast path.
"""

import sys


class Obj:
    def m(self, x, bump=1):
        return x + bump


N = int(sys.argv[1]) if len(sys.argv) > 1 else 5_000_000
o = Obj()
total = 0
i = 0
while i < N:
    total = o.m(total)  # total + 1, default engaged every iteration
    i = i + 1
print(total)
