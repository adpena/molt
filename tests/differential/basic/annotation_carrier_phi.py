events = []


class Owner:
    def __del__(self):
        events.append("released")


def echo_float(value: float, choose=True):
    if choose:
        result = value
    else:
        result = 0.5
    return result


def echo_bool(value: bool, choose=True):
    if choose:
        result = value
    else:
        result = False
    return result


def echo_int(value: int, choose=True):
    if choose:
        result = value
    else:
        result = 17
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
