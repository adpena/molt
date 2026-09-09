"""Module function visibility survives source pruning and name rebinding."""


def _private_helper():
    return "private-visible"


if False:
    _private_helper()
print(globals()["_private_helper"]())

view = locals()


def deleted_helper():
    return "captured-before-delete"


saved = view.copy()
del deleted_helper
print(saved["deleted_helper"]())
print("deleted_helper" in globals())


def repeated():
    return "first"


del repeated


def repeated():
    return "second"


print(repeated())
