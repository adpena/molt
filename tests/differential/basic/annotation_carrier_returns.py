events = []


class Owner:
    def __del__(self):
        events.append("released")


def echo_float(value) -> float:
    return value


def echo_bool(value) -> bool:
    return value


def echo_int(value) -> int:
    return value


for echo in (echo_float, echo_bool, echo_int):
    events.clear()
    owner = Owner()
    alias = echo(owner)
    print(alias is owner, type(alias).__name__)
    del owner
    print(events)
    del alias
    print(events)
