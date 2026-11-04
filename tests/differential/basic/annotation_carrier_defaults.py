events = []


class Owner:
    def __del__(self):
        events.append("released")


def exercise():
    for kind in range(3):
        events.clear()
        owner = Owner()
        if kind == 0:
            def echo(value: float = owner) -> float:
                return value
        elif kind == 1:
            def echo(value: bool = owner) -> bool:
                return value
        else:
            def echo(value: int = owner) -> int:
                return value
        alias = echo()
        print(alias is owner, type(alias).__name__)
        del owner
        print(events)
        del echo
        print(events)
        del alias
        print(events)


exercise()
