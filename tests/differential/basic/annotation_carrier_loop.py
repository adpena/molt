events = []


class Owner:
    def __del__(self):
        events.append("released")


def echo_float(value: float):
    result = value
    for i in range(4):
        previous = result
        result = value if i % 2 else previous
    return result


def echo_bool(value: bool):
    result = value
    for i in range(4):
        previous = result
        result = value if i % 2 else previous
    return result


def echo_int(value: int):
    result = value
    for i in range(4):
        previous = result
        result = value if i % 2 else previous
    return result


for echo in (echo_float, echo_bool, echo_int):
    events.clear()
    owner = Owner()
    alias = echo(owner)
    print(alias is owner, type(alias).__name__)
    del owner
    print(events)
    del alias
    print(events)
