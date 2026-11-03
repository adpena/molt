seen = []


class Owner:
    def __del__(self):
        seen.append(1)


def echo_float(value: float):
    return value


def echo_bool(value: bool):
    return value


for echo in (echo_float, echo_bool):
    seen.clear()
    owner = Owner()
    alias = echo(owner)
    print(alias is owner, type(alias).__name__)
    del owner
    print(seen)
    del alias
    print(seen)
