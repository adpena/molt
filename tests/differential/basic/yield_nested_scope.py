"""Purpose: differential coverage for yield in nested control flow."""


def gen(flag: bool):
    if flag:
        for i in range(2):
            if i == 1:
                yield ("loop", i)
    else:
        yield ("else", 0)
    try:
        if flag:
            yield ("try", flag)
    finally:
        yield ("finally", flag)


print(list(gen(True)))
print(list(gen(False)))
