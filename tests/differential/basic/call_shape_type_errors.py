"""Call-shape errors are TypeErrors that a program can catch, as in CPython.

Each probe calls a builtin or a method of an exactly known receiver with the
wrong number of arguments. The call compiles; running it raises TypeError.
"""


def attempt(label, probe):
    try:
        probe()
    except TypeError:
        print(label, "TypeError")
    else:
        print(label, "no error")


def len_without_argument():
    return len()


def len_with_two_arguments():
    return len([1], [2])


def isinstance_with_one_argument():
    return isinstance(1)


def getattr_with_one_argument():
    return getattr(object())


def ord_without_argument():
    return ord()


def enumerate_with_three_arguments():
    return enumerate([], 0, 1)


def enumerate_with_unknown_keyword():
    return enumerate([], step=1)


def set_add_without_argument():
    items = set()
    items.add()


def list_pop_with_two_arguments():
    items = [1]
    return items.pop(0, 1)


def list_index_without_argument():
    items = [1]
    return items.index()


def list_index_with_keyword():
    items = [1]
    return items.index(1, start=0)


def dict_get_without_argument():
    table = {}
    return table.get()


def str_lower_with_argument():
    text = "a"
    return text.lower(1)


def str_strip_with_two_arguments():
    text = "a"
    return text.strip(" ", " ")


def generator_send_without_argument():
    def numbers():
        yield 1

    return numbers().send()


for label, probe in (
    ("len()", len_without_argument),
    ("len(a, b)", len_with_two_arguments),
    ("isinstance(x)", isinstance_with_one_argument),
    ("getattr(x)", getattr_with_one_argument),
    ("ord()", ord_without_argument),
    ("enumerate(a, b, c)", enumerate_with_three_arguments),
    ("enumerate(a, step=)", enumerate_with_unknown_keyword),
    ("set.add()", set_add_without_argument),
    ("list.pop(a, b)", list_pop_with_two_arguments),
    ("list.index()", list_index_without_argument),
    ("list.index(a, start=)", list_index_with_keyword),
    ("dict.get()", dict_get_without_argument),
    ("str.lower(a)", str_lower_with_argument),
    ("str.strip(a, b)", str_strip_with_two_arguments),
    ("generator.send()", generator_send_without_argument),
):
    attempt(label, probe)
