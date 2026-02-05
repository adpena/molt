"""Purpose: ensure constant defaults like ellipsis/big-int are lowered."""

BIG = 1 << 80
ELL = Ellipsis


def use_ellipsis(value=...):
    return value


def use_ellipsis_named(value=ELL):
    return value


def use_big_int(value=BIG):
    return value


def use_ellipsis_shadowed() -> object:
    Ellipsis = "shadowed"

    def inner(value=Ellipsis):
        return value

    return inner()


print(use_ellipsis() is Ellipsis)
print(use_ellipsis() == ...)
print(use_ellipsis_named() is Ellipsis)
print(use_ellipsis_named() == ...)
print(use_big_int() == BIG)
print(use_big_int())
print(use_ellipsis_shadowed() == "shadowed")
