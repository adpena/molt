"""Purpose: tuple literal indexed loops must preserve canonical tuple layout."""


def run_with_tuple():
    for value in ("hello",):
        pass
    return "tuple-ok"


def run_with_list():
    for value in ["hello"]:
        pass
    return "list-ok"


print(run_with_list())
print(run_with_tuple())
