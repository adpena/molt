"""Purpose: differential coverage for del global basic."""

x = 1


def drop() -> str:
    global x
    del x
    try:
        return x
    except NameError as err:
        return type(err).__name__


print(drop())
