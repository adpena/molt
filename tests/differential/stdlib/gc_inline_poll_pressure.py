"""Allocation schedules collection; generated loop/call polls service it."""

import gc

thresholds = gc.get_threshold()
enabled = gc.isenabled()
gc.disable()
gc.collect()
gc.set_threshold(16, 10, 10)
gc.enable()
before = gc.get_stats()[0]["collections"]
try:
    for i in range(10_000):
        cycle = []
        cycle.append(cycle)
        cycle = None
    print("automatic young collection", gc.get_stats()[0]["collections"] > before)
finally:
    gc.disable()
    gc.collect()
    gc.set_threshold(*thresholds)
    if enabled:
        gc.enable()
