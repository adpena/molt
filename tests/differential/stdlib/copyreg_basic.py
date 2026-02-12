"""Purpose: differential coverage for copyreg registry helpers."""

import copyreg


class Payload:
    pass


def reduce_payload(obj):
    return (Payload, ())


print("dispatch_before", Payload in copyreg.dispatch_table)
copyreg.pickle(Payload, reduce_payload)
print("dispatch_after", Payload in copyreg.dispatch_table)
try:
    copyreg.pickle(Payload, None)
except Exception as exc:
    print("pickle_none", type(exc).__name__)
print("dispatch_still", Payload in copyreg.dispatch_table)


def ctor():
    return Payload()


print("constructor_none", copyreg.constructor(ctor) is None)

copyreg.add_extension("demo.mod", "Payload", 15001)
print("add_extension_ok", True)
copyreg.add_extension("demo.mod", "Payload", 15001)
print("add_extension_redundant_ok", True)
try:
    copyreg.add_extension("demo.other", "Other", 15001)
except Exception as exc:
    print("dup_code", type(exc).__name__)

copyreg.remove_extension("demo.mod", "Payload", 15001)
print("remove_extension_ok", True)
copyreg.clear_extension_cache()
print("clear_cache_ok", True)
