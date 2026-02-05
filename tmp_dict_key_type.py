_copy_dispatch = {}


def _copy_frozenset(x):
    return x


_copy_dispatch[frozenset] = _copy_frozenset
print("ok")
