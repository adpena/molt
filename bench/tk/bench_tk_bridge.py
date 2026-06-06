"""tkinter bridge microbenchmarks: isolate the Tcl/Tk FFI bridge from rendering.

Runs identically under CPython (`python3 bench/tk/bench_tk_bridge.py`) and under a
molt-compiled binary (`molt build` + `safe_run`). Each section reports ns/op so the
CPython baseline and molt can be compared directly.

The benches deliberately avoid `mainloop()`. The window is withdrawn so nothing
flashes, and the event-dispatch bench is driven by a bounded `update()` loop with
`after_idle` callbacks — never an unbounded loop.

Method calls go through `root.tk.<method>(...)` directly (no binding the bound
method to a local) so the two runtimes execute the same dispatch shape; the
comparison stays apples-to-apples.

Sections (each isolates one layer of the `widget.tk.call(...)` path):
  expr        — `tk.call('expr', '1+1')` : pure command dispatch + result
  setget      — `tk.call('set', x, v)` round-trip : arg-box + var write/read
  result_int  — `tk.call('expr', '40+2')` consumed as int : result TYPE conversion
  result_dbl  — `tk.call('expr', '...')` consumed as float : double TYPE conversion
  result_list — `tk.call('lrange', big, 0, end)` -> 100-elem tuple : list conversion
  stringvar   — StringVar.set / .get : Python-side Variable + bridge
  intvar      — IntVar.set / .get : adds getint() re-coercion on the result
  widget      — Label create + destroy : widget lifecycle
  event       — after_idle callbacks drained by update() : event-loop dispatch

Usage:
    python3 bench/tk/bench_tk_bridge.py [scale]
        scale (float, default 1.0) multiplies every iteration count, so CI can run
        a fast subset (e.g. 0.02) while measurement runs at 1.0.
"""

import sys
import time
import tkinter


def _now_ns():
    return time.perf_counter_ns()


def _report(name, iters, elapsed_ns):
    ns_per_op = elapsed_ns / iters if iters else 0.0
    print(f"BENCH {name} iters={iters} total_ns={elapsed_ns} ns_per_op={ns_per_op:.1f}")


def bench_expr(tk, iters):
    t0 = _now_ns()
    for _ in range(iters):
        tk.call("expr", "1+1")
    return _now_ns() - t0


def bench_setget(tk, iters):
    t0 = _now_ns()
    for _ in range(iters):
        tk.call("set", "molt_bench_x", "hello")
        tk.call("set", "molt_bench_x")
    return _now_ns() - t0


def bench_result_int(tk, iters):
    # Result is an integer. Under wantobjects=1 this should come back as a Python
    # int with no string round-trip. We force int() consumption to mimic real code.
    acc = 0
    t0 = _now_ns()
    for _ in range(iters):
        acc += tk.getint(tk.call("expr", "40+2"))
    elapsed = _now_ns() - t0
    assert acc == 42 * iters, acc
    return elapsed


def bench_result_double(tk, iters):
    acc = 0.0
    t0 = _now_ns()
    for _ in range(iters):
        acc += tk.getdouble(tk.call("expr", "3.5 + 1.25"))
    elapsed = _now_ns() - t0
    assert abs(acc - 4.75 * iters) < 1e-3, acc
    return elapsed


def bench_result_list(tk, iters):
    # Build a 100-element Tcl list once, then convert it on every iteration.
    tk.call("set", "molt_bench_list", " ".join(str(i) for i in range(100)))
    total = 0
    t0 = _now_ns()
    for _ in range(iters):
        parts = tk.splitlist(tk.call("lrange", tk.call("set", "molt_bench_list"), 0, "end"))
        total += len(parts)
    elapsed = _now_ns() - t0
    assert total == 100 * iters, total
    return elapsed


def bench_stringvar(iters):
    var = tkinter.StringVar()
    t0 = _now_ns()
    for _ in range(iters):
        var.set("value")
        var.get()
    return _now_ns() - t0


def bench_intvar(iters):
    var = tkinter.IntVar()
    acc = 0
    t0 = _now_ns()
    for i in range(iters):
        var.set(i)
        acc += var.get()
    return _now_ns() - t0


def bench_widget(root, iters):
    t0 = _now_ns()
    for _ in range(iters):
        w = tkinter.Label(root, text="x")
        w.destroy()
    return _now_ns() - t0


def bench_event(root, iters):
    # Schedule `iters` after_idle callbacks, then drain them with update().
    # Exercises the event-loop dispatch + bound-callback bridge path.
    counter = [0]

    def cb():
        counter[0] += 1

    t0 = _now_ns()
    for _ in range(iters):
        root.after_idle(cb)
    spins = 0
    while counter[0] < iters and spins < iters * 4 + 1000:
        root.update()
        spins += 1
    elapsed = _now_ns() - t0
    assert counter[0] == iters, (counter[0], iters)
    return elapsed


def main():
    scale = float(sys.argv[1]) if len(sys.argv) > 1 else 1.0

    def n(base):
        v = int(base * scale)
        return v if v > 0 else 1

    root = tkinter.Tk()
    root.withdraw()
    tk = root.tk

    print(f"wantobjects={tkinter.wantobjects}")

    # Warm up the interpreter before timing.
    for _ in range(200):
        tk.call("expr", "1+1")

    _report("expr", n(50000), bench_expr(tk, n(50000)))
    _report("setget", n(50000) * 2, bench_setget(tk, n(50000)))
    _report("result_int", n(50000), bench_result_int(tk, n(50000)))
    _report("result_double", n(50000), bench_result_double(tk, n(50000)))
    _report("result_list", n(20000), bench_result_list(tk, n(20000)))
    _report("stringvar", n(50000) * 2, bench_stringvar(n(50000)))
    _report("intvar", n(50000) * 2, bench_intvar(n(50000)))
    _report("widget", n(2000), bench_widget(root, n(2000)))
    _report("event", n(10000), bench_event(root, n(10000)))

    root.destroy()
    print("DONE")


if __name__ == "__main__":
    main()
