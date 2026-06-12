"""Purpose: finalizer-sensitive locals release at Python scope exit, not SSA last read."""

events = []


class Item:
    def __del__(self) -> None:
        events.append("del")


def run() -> None:
    bag = [Item()]
    print("inside", list(events))


run()
print("after", list(events))
