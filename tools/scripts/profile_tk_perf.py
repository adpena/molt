#!/usr/bin/env python3
"""Tkinter performance profiling script for molt vs CPython comparison.

Measures key tkinter operations and prints a results table.
Self-contained — does not require the mawn codebase.

Usage:
    # With CPython:
    python3 tools/scripts/profile_tk_perf.py

    # With molt:
    molt run tools/scripts/profile_tk_perf.py

    # Headless mode (skip tests that need a visible window, just validate import):
    MOLT_TK_PROFILE_HEADLESS=1 python3 tools/scripts/profile_tk_perf.py
"""

from __future__ import annotations

import os
import sys
import time
import threading
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import tkinter as tk

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

NUM_BUTTONS = 100
NUM_LABELS = 100
NUM_CANVAS_ITEMS = 1000
NUM_AFTER_CALLBACKS = 100
BACKGROUND_WORK_SECONDS = 0.5
UI_POLL_INTERVAL_MS = 10
WARMUP_PUMP_CYCLES = 50


def _is_headless() -> bool:
    return os.environ.get("MOLT_TK_PROFILE_HEADLESS", "").strip().lower() in {
        "1",
        "true",
        "yes",
    }


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def pump_events(root: tk.Tk, cycles: int = 20) -> None:
    """Process pending Tk events without entering mainloop."""
    import tkinter as tk

    for _ in range(cycles):
        try:
            root.update_idletasks()
            root.update()
        except tk.TclError:
            break


def timed(label: str, fn: object, results: list) -> None:
    """Run *fn*, record wall-clock time in *results*."""
    t0 = time.monotonic()
    fn()
    elapsed = time.monotonic() - t0
    results.append((label, elapsed))


# ---------------------------------------------------------------------------
# Benchmark functions
# ---------------------------------------------------------------------------


def bench_widget_creation(root: tk.Tk) -> tuple[list, list]:
    """Create NUM_BUTTONS + NUM_LABELS widgets, return (buttons, labels)."""
    import tkinter as tk

    frame = tk.Frame(root)
    frame.pack(fill="both", expand=True)
    buttons = []
    for i in range(NUM_BUTTONS):
        b = tk.Button(frame, text=f"Button {i}", width=10)
        b.pack()
        buttons.append(b)
    labels = []
    for i in range(NUM_LABELS):
        lbl = tk.Label(frame, text=f"Label {i}", width=20)
        lbl.pack()
        labels.append(lbl)
    pump_events(root)
    return buttons, labels


def bench_widget_config(root: tk.Tk, buttons: list, labels: list) -> None:
    """Reconfigure every widget (fg, text, relief)."""
    for i, b in enumerate(buttons):
        b.config(text=f"Btn-{i}", fg="#333333", relief="raised")
    for i, lbl in enumerate(labels):
        lbl.config(text=f"Lbl-{i}", fg="#555555", bg="#eeeeee")
    pump_events(root)


def bench_canvas_ops(root: tk.Tk) -> tk.Canvas:
    """Draw NUM_CANVAS_ITEMS rectangles + ovals on a canvas."""
    import tkinter as tk

    canvas = tk.Canvas(root, width=600, height=400, bg="white")
    canvas.pack()
    for i in range(NUM_CANVAS_ITEMS):
        x = (i * 7) % 580
        y = (i * 11) % 380
        if i % 2 == 0:
            canvas.create_rectangle(x, y, x + 15, y + 15, fill="#aabbcc")
        else:
            canvas.create_oval(x, y, x + 12, y + 12, fill="#ccbbaa")
    pump_events(root)
    return canvas


def bench_canvas_move(root: tk.Tk, canvas: tk.Canvas) -> None:
    """Move every canvas item by (1, 1)."""
    for item_id in canvas.find_all():
        canvas.move(item_id, 1, 1)
    pump_events(root)


def bench_after_callbacks(root: tk.Tk) -> float:
    """Schedule NUM_AFTER_CALLBACKS `after` callbacks and measure jitter.

    Returns the max deviation from the expected 1 ms interval.
    """
    results_ns: list[float] = []
    remaining = {"count": NUM_AFTER_CALLBACKS}
    done_event = threading.Event()

    def _tick() -> None:
        results_ns.append(time.monotonic())
        remaining["count"] -= 1
        if remaining["count"] > 0:
            root.after(1, _tick)
        else:
            done_event.set()

    start = time.monotonic()
    root.after(1, _tick)

    # Pump the event loop until all callbacks fire (with a timeout).
    deadline = time.monotonic() + 5.0
    while not done_event.is_set() and time.monotonic() < deadline:
        pump_events(root, cycles=5)
        # Tiny sleep to avoid busy-spinning when there are no events yet.
        time.sleep(0.001)

    total_time = time.monotonic() - start

    return total_time


def bench_background_thread(root: tk.Tk) -> tuple[float, float]:
    """Start a CPU-bound background thread, measure UI responsiveness.

    Returns (bg_thread_time, ui_responsiveness_time).
    - bg_thread_time: wall clock for the background work.
    - ui_responsiveness_time: total time the UI spent pumping events while
      the background thread was running.
    """
    bg_result: dict[str, float] = {}
    bg_done = threading.Event()

    def _cpu_work() -> None:
        t0 = time.monotonic()
        # Deliberately CPU-bound: compute sum of squares.
        total = 0
        for i in range(2_000_000):
            total += i * i
        bg_result["elapsed"] = time.monotonic() - t0
        bg_result["total"] = total
        bg_done.set()

    thread = threading.Thread(target=_cpu_work, daemon=True)

    ui_timestamps: list[float] = []
    t_start = time.monotonic()
    thread.start()

    deadline = t_start + 10.0
    while not bg_done.is_set() and time.monotonic() < deadline:
        t_pump_start = time.monotonic()
        pump_events(root, cycles=3)
        ui_timestamps.append(time.monotonic() - t_pump_start)
        time.sleep(0.005)

    thread.join(timeout=5.0)

    bg_elapsed = bg_result.get("elapsed", 0.0)
    ui_total = sum(ui_timestamps) if ui_timestamps else 0.0

    return bg_elapsed, ui_total


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def run_profile() -> list[tuple[str, float]]:
    import tkinter as tk

    results: list[tuple[str, float]] = []

    root = tk.Tk()
    root.title("molt tk profiler")
    root.geometry("800x600")
    root.withdraw()  # hidden — we don't need a visible window
    pump_events(root, cycles=WARMUP_PUMP_CYCLES)

    # 1. Widget creation
    buttons: list = []
    labels: list = []

    def _create() -> None:
        nonlocal buttons, labels
        buttons, labels = bench_widget_creation(root)

    timed("widget_creation (200 widgets)", _create, results)

    # 2. Widget configuration
    timed(
        "widget_config (200 widgets)",
        lambda: bench_widget_config(root, buttons, labels),
        results,
    )

    # 3. Canvas draw
    canvas_ref: list = []

    def _canvas_draw() -> None:
        canvas_ref.append(bench_canvas_ops(root))

    timed(f"canvas_draw ({NUM_CANVAS_ITEMS} items)", _canvas_draw, results)

    # 4. Canvas move
    timed(
        f"canvas_move ({NUM_CANVAS_ITEMS} items)",
        lambda: bench_canvas_move(root, canvas_ref[0]),
        results,
    )

    # 5. After callbacks
    def _after() -> None:
        bench_after_callbacks(root)

    timed(f"after_callbacks ({NUM_AFTER_CALLBACKS}x)", _after, results)

    # 6. Background thread + UI responsiveness
    bg_elapsed = 0.0
    ui_elapsed = 0.0

    def _bg_thread() -> None:
        nonlocal bg_elapsed, ui_elapsed
        bg_elapsed, ui_elapsed = bench_background_thread(root)

    timed("bg_thread + UI pump", _bg_thread, results)
    results.append(("  bg_thread_cpu_work", bg_elapsed))
    results.append(("  ui_pump_during_bg", ui_elapsed))

    # Cleanup
    try:
        root.destroy()
    except Exception:
        pass

    return results


def print_results(results: list[tuple[str, float]]) -> None:
    runtime = (
        "molt" if hasattr(sys, "_molt_version") else f"CPython {sys.version.split()[0]}"
    )
    print()
    print(f"=== Tkinter Performance Profile ({runtime}) ===")
    print()
    print(f"  {'Benchmark':<40s}  {'Time (ms)':>12s}")
    print(f"  {'-' * 40}  {'-' * 12}")
    for label, elapsed in results:
        print(f"  {label:<40s}  {elapsed * 1000:>12.2f}")
    print()
    print(f"  Runtime: {runtime}")
    print(f"  Platform: {sys.platform}")
    print()


def main() -> int:
    if _is_headless():
        print("Headless mode: validating tkinter import only.")
        import tkinter as tk

        root = tk.Tk()
        root.withdraw()
        root.destroy()
        print("tkinter import OK.")
        return 0

    # Detect molt runtime.
    is_molt = hasattr(sys, "_molt_version")
    if is_molt:
        print("Detected molt runtime.")
    else:
        print(f"Detected CPython {sys.version.split()[0]}.")

    results = run_profile()
    print_results(results)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
