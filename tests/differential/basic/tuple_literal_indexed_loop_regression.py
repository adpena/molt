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


def heap_literals(stop):
    result = []
    for index in range(6):
        # Non-identifier strings are not protected by automatic name interning.
        value = ("non-interned literal", b"\x00\xff\x80", 9223372036854775807,
                 123456789012345678901234567890)
        alias = value
        result.append(alias)
        del value, alias
        if index == stop:
            break
    return result


def escaped_literal(fail):
    try:
        if fail:
            raise ValueError("literal-error")
        return "non-interned literal"
    except ValueError:
        return "non-interned literal"


for stop in (0, 3, 9):
    first = heap_literals(stop)
    second = heap_literals(stop)
    print(len(first), first == second, first[0] == second[-1])
print(escaped_literal(False), escaped_literal(True))
