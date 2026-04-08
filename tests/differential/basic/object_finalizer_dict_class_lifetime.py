"""Purpose: exercise __del__ with populated __dict__ and class resurrection access."""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    events = []
    keep = []

    class Demo:
        kind = "DemoKind"

        def __init__(self, value: int) -> None:
            self.value = value
            self.note = "live"

        def __del__(self) -> None:
            events.append(
                (
                    "del-enter",
                    self.__class__.__name__,
                    self.__class__.kind,
                    sorted(self.__dict__.items()),
                )
            )
            if not keep:
                keep.append(self)
                self.note = "resurrected"
                self.extra = len(events)
                events.append(
                    (
                        "del-resurrect",
                        self.__class__.__name__,
                        self.__class__.kind,
                        sorted(self.__dict__.items()),
                    )
                )

    def run() -> None:
        item = Demo(7)
        item.tag = "payload"
        del item
        gc.collect()
        print(
            "after_first",
            events,
            len(keep),
            keep[0].__class__.__name__,
            keep[0].__class__.kind,
            sorted(keep[0].__dict__.items()),
        )

        resurrected = keep.pop()
        resurrected.shadow = "final"
        del resurrected
        gc.collect()
        print("after_second", events, len(keep))

    run()
