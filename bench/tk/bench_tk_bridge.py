"""tkinter bridge microbenchmarks: isolate the Tcl/Tk FFI bridge from rendering.

Runs identically under CPython (`python3 bench/tk/bench_tk_bridge.py`) and under a
molt-compiled binary (`molt build` + `safe_run`). Each section reports ns/op so the
CPython baseline and molt can be compared directly.

The benches deliberately avoid `mainloop()`. The window is withdrawn so nothing
flashes, and the event-dispatch bench is driven by a bounded `update`/`dooneevent`
loop with `after(0)` callbacks — never an unbounded loop.

Sections (each isolates one layer of the `widget.tk.call(...)` path):
  expr        — `tk.call('expr', '1+1')` : pure command dispatch + STRING result
  setget      — `tk.call('set', x, v)` round-trip : arg-box + var write/read
  result_int  — `tk.call('expr', '40+2')` consumed as int : result TYPE conversion
  result_dbl  — `tk.call('expr', '...')` consumed as float : double TYPE conversion
  result_list — `tk.call('lrange', big, 0, end)` -> 100-elem tuple : list conversion
  stringvar   — StringVar.set / .get : Python-side Variable + bridge
  intvar      — IntVar.set / .get : adds getint() re-coercion on the result
  widget      — Label create + destroy : widget lifecycle
  event       — after(0) callbacks drained by update() : event-loop dispatch

Usage:
    python3 bench/tk/bench_tk_bridge.py [scale]
        scale (float, default 1.0) multiplies every iteration count, so CI can run
        a fast subset (e.g. 0.02) while measurement runs at 1.0.
"""

import sys
import time
import tkinter


def _now_ns():
    # time.perf_counter_ns is the high-resolution monotonic clock in both runtimes.
    return time.perf_counter_ns()


def _report(name, iters, elapsed_ns):
    ns_per_op = elapsed_ns / iters if iters else 0.0
    # Stable, greppable, single-line-per-section format.
    print(f"BENCH {name} iters={iters} total_ns={elapsed_ns} ns_per_op={ns_per_op:.1f}")


def bench_expr(tk, iters):
    call = tk.call
    t0 = _now_ns()
    for _ in range(iters):
        call("expr", "1+1")
    return _now_ns() - t0


def bench_setget(tk, iters):
    call = tk.call
    t0 = _now_ns()
    for _ in range(iters):
        call("set", "molt_bench_x", "hello")
        call("set", "molt_bench_x")
    return _now_ns() - t0


def bench_result_int(tk, iters):
    # Result is an integer. Under wantobjects=1 this must come back as a Python int
    # with no string round-trip. We force int() consumption to mimic real code.
    call = tk.call
    getint = tk.getint
    acc = 0
    t0 = _now_ns()
    for _ in range(iters):
        acc += getint(call("expr", "40+2"))
    elapsed = _now_ns() - t0
    assert acc == 42 * iters, acc
    return elapsed


def bench_result_double(tk, iters):
    call = tk.call
    getdouble = tk.getdouble
    acc = 0.0
    t0 = _now_ns()
    for _ in range(iters):
        acc += getdouble(call("expr", "3.5 + 1.25"))
    elapsed = _now_ns() - t0
    assert abs(acc - 4.75 * iters) < 1e-3, acc
    return elapsed


def bench_result_list(tk, iters):
    # Build a 100-element Tcl list once, then convert it on every iteration.
    # `lrange list 0 end` returns the whole list; splitlist turns it into a tuple.
    call = tk.call
    splitlist = tk.splitlist
    call("set", "molt_bench_list", " ".join(str(i) for i in range(100)))
    total = 0
    t0 = _now_ns()
    for _ in range(iters):
        parts = splitlist(call("lrange", call("set", "molt_bench_list"), 0, "end"))
        total += len(parts)
    elapsed = _now_ns() - t0
    assert total == 100 * iters, total
    return elapsed


def bench_stringvar(iters):
    var = tkinter.StringVar()
    t0 = _now_ns()
    for i in range(iters):
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
    elapsed = _now_ns() - t0
    return elapsed


def bench_widget(root, iters):
    Label = tkinter.Label
    t0 = _now_ns()
    for _ in range(iters):
        w = Label(root, text="x")
        w.destroy()
    return _now_ns() - t0


def bench_event(root, iters):
    # Schedule `iters` after(0) callbacks, then drain them with update().
    # This exercises the event-loop dispatch + bound-callback bridge path.
    counter = [0]

    def cb():
        counter[0] += 1

    t0 = _now_ns()
    for _ in range(iters):
        root.after_idle(cb)
    # Drain. update_idletasks runs the idle queue; loop until all fired.
    spins = 0
    while counter[0] < iters and spins < iters * 4 + 1000:
        root.update()
        spins += 1
    elapsed = _now_ns() - t0
    return elapsed


def main():
    scale = float(sys.argv[1]) if len(sys.argv) > 1 else 1.0

    def n(base):
        v = int(base * scale)
        return v if v > 0 else 1

    root = tkinter.Tk()
    root.withdraw()
    tk = root.tk

    print(f"wantobjects={tk.wantobjects()}")

    # Warm up the interpreter and JIT-ish caches before timing.
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
