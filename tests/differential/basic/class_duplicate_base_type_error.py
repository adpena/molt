"""A repeated base is a TypeError when the class statement runs, as in CPython."""


def build_with_repeated_builtin_base():
    class Twice(int, int):
        pass

    return Twice


class Base:
    pass


Alias = Base


def build_with_aliased_base():
    class Twice(Base, Alias):
        pass

    return Twice


for label, build in (
    ("int, int", build_with_repeated_builtin_base),
    ("Base, Alias", build_with_aliased_base),
):
    try:
        build()
    except TypeError as error:
        print(label, "TypeError", error)
    else:
        print(label, "no error")
