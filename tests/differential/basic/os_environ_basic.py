"""Purpose: exercise os.environ mapping methods + type checks."""

import os


KEY = "MOLT_TEST_ENV_KEY"


old_value = os.environ.get(KEY)
had_key = KEY in os.environ


def _restore():
    if had_key:
        os.environ[KEY] = old_value
    else:
        os.environ.pop(KEY, None)


try:
    os.environ[KEY] = "one"
    print(os.environ[KEY])
    print(os.environ.get(KEY))
    print(KEY in os.environ)
    print(os.environ.setdefault(KEY, "two"))
    print(os.environ.pop(KEY))
    print(KEY in os.environ)
    try:
        os.environ.pop(KEY)
        print("pop-missing-ok")
    except KeyError:
        print("pop-missing-keyerror")

    os.environ.update({KEY: "three"})
    print(os.environ[KEY])
    copied = os.environ.copy()
    print(isinstance(copied, dict))

    popped_key, popped_val = os.environ.popitem()
    os.environ[popped_key] = popped_val
    print("popitem-ok")

    os.environ.update(**{KEY: "four"})
    print(os.environ[KEY])
    os.environ.pop(KEY, None)
    print(os.environ.setdefault(KEY, "five"))

    try:
        os.environ[1] = "x"
        print("set-nonstr-key-ok")
    except TypeError:
        print("set-nonstr-key-typeerror")

    try:
        os.environ["bad"] = 1
        print("set-nonstr-val-ok")
    except TypeError:
        print("set-nonstr-val-typeerror")

    try:
        os.environ.update({1: "x"})
        print("update-nonstr-key-ok")
    except TypeError:
        print("update-nonstr-key-typeerror")

    try:
        os.environ.update({"x": 1})
        print("update-nonstr-val-ok")
    except TypeError:
        print("update-nonstr-val-typeerror")

finally:
    _restore()
