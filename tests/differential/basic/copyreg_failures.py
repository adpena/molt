"""Purpose: differential coverage for copyreg failure semantics."""

import copyreg


class Payload:
    pass


def reducer(obj):
    return (Payload, ())


def show(label, fn):
    try:
        fn()
    except Exception as exc:
        print(label, type(exc).__name__)
    else:
        print(label, "ok")


show("pickle_bad_cls", lambda: copyreg.pickle(1, reducer))
show("pickle_bad_reducer", lambda: copyreg.pickle(Payload, 1))
show("constructor_bad", lambda: copyreg.constructor(1))
show("add_bad_code_value", lambda: copyreg.add_extension("demo.mod", "Payload", 0))
copyreg.add_extension("demo.coerce", "Payload", "17001")
show(
    "remove_coerced_code",
    lambda: copyreg.remove_extension("demo.coerce", "Payload", 17001),
)

copyreg.add_extension("demo.mod", "Payload", 12001)
show("add_dup_code", lambda: copyreg.add_extension("demo.other", "Other", 12001))
show(
    "add_conflict_code",
    lambda: copyreg.add_extension("demo.mod", "Payload", 12002),
)
show(
    "remove_mismatch",
    lambda: copyreg.remove_extension("demo.mod", "Payload", 12002),
)
copyreg.remove_extension("demo.mod", "Payload", 12001)
show(
    "remove_missing",
    lambda: copyreg.remove_extension("demo.mod", "Payload", 12001),
)
