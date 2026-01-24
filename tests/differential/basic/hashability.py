"""Purpose: differential coverage for hashability."""


def report(label, fn):
    try:
        fn()
        print(label, "ok")
    except TypeError:
        print(label, "typeerror")


def dict_list_key():
    return {[]: 1}


def dict_dict_key():
    return {{}: 1}


def dict_set_key():
    return {{1, 2}: 1}


def dict_bytearray_key():
    return {bytearray(b"a"): 1}


report("dict-list-key", dict_list_key)
report("dict-dict-key", dict_dict_key)
report("dict-set-key", dict_set_key)
report("dict-bytearray-key", dict_bytearray_key)


def set_with_list():
    s = set()
    s.add([])
    return s


report("set-list-key", set_with_list)
