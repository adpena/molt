"""Purpose: differential coverage for __del__ finalizer semantics."""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    events = []

    class Demo:
        def __init__(self, value: int) -> None:
            self.value = value

        def __del__(self) -> None:
            events.append(self.value)

    def run() -> None:
        item = Demo(1)
        del item
        gc.collect()

    run()
    print(events)
