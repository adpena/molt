"""Microbench: a hot method call against a method with a POSITIONAL DEFAULT.

This is the exact shape the asyncio-P0 adversarial reviewer used to catch a
~13x regression: a hot `obj.m(i)` call site against `def m(self, x, bump=1)`,
where the call supplies one positional short and relies on the default.

What this bench guards: `o.m(i)` lowers to `load_method` + `call_loaded_method`
(HF-216). The load's per-site inline cache selects the plain function without a
bound method, a name intern or an MRO walk. The call's direct path
(`call_owned_function`) passes `[self, i]` from a stack vector and fills the
trailing `bump` from the LIVE `__defaults__`, so a runtime
`Class.method.__defaults__ = (...)` reassignment applies at the next call.
Keyword-only, `*args` and `**kwargs` methods take the binder.

PERF (Apple M-series, native release, process wall time, median of 5, 5M
calls, 2026-10-14): 3.77s before HF-216 (a bound-method object per call);
2.51s with the binder filling the default (a heap argument vector per call);
0.90s with the direct path filling it (a stack argument vector). The last two
create no object per call (`MOLT_DEBUG_ALLOC_OBJECT=1`).

Faster still is DEVIRTUALIZING the default-bearing method call to a direct
compiled CALL. The frontend `_try_emit_user_method_static_call` bails on
`method_info["defaults"]`; inlining LITERAL defaults would break a runtime
`Class.method.__defaults__ = (...)` reassignment, so the sound form needs a
function-`__defaults__`-mutation deopt guard (a separate frontend/deopt arc).

Regression guard: if the time returns toward ~2.5s the call fell back to the
binder; toward ~3.8s, to a bound method.
"""


class Obj:
    def m(self, x, bump=1):
        return x + bump


def main() -> None:
    o = Obj()
    total = 0
    for i in range(5_000_000):
        total += o.m(i)
    print(total)


if __name__ == "__main__":
    main()
