events = []


class Owner:
    def __del__(self):
        events.append("released")


def make_float(value: float):
    def inner() -> float:
        return value
    return inner


def make_bool(value: bool):
    def inner() -> bool:
        return value
    return inner


def make_int(value: int):
    def inner() -> int:
        return value
    return inner


def echo_float(value):
    inner = make_float(value)
    return inner()


def echo_bool(value):
    inner = make_bool(value)
    return inner()


def echo_int(value):
    inner = make_int(value)
    return inner()


for echo in (echo_float, echo_bool, echo_int):
    events.clear()
    owner = Owner()
    alias = echo(owner)
    print(alias is owner, type(alias).__name__)
    del owner
    print(events)
    del alias
    print(events)
