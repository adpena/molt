def show(label, func):
    try:
        func()
    except Exception as exc:
        print(f"{label}:{type(exc).__name__}:{exc}")


def dict_len3():
    dict([(1, 2, 3)])


def dict_len1():
    dict([(1,)])


def dict_not_iter():
    dict([1])


def update_len3():
    {}.update([(1, 2, 3)])


def update_len1():
    {}.update([(1,)])


def update_not_iter():
    {}.update([1])


show("dict-len3", dict_len3)
show("dict-len1", dict_len1)
show("dict-not-iter", dict_not_iter)

show("update-len3", update_len3)
show("update-len1", update_len1)
show("update-not-iter", update_not_iter)
