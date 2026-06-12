"""Purpose: differential coverage for __del__ resurrection and run-once semantics.

STATUS: must-pass. Explicit local `del` stores the missing sentinel before
releasing the old slot occupant, so `__del__` observes CPython's deleted-local
state while preserving resurrection and run-once semantics.
"""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    events = []
    keep = []

    class Demo:
        def __del__(self) -> None:
            events.append("del")
            if not keep:
                keep.append(self)

    def run() -> None:
        item = Demo()
        del item
        gc.collect()
        print("after_first", events, len(keep))

        keep.clear()
        gc.collect()
        print("after_second", events, len(keep))

    run()
