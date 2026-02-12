# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for trace basic."""

import trace


def f(x):
    return x + 1

tracer = trace.Trace(count=False, trace=False)
print(tracer.runfunc(f, 1))
