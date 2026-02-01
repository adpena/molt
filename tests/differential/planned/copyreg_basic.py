"""Purpose: differential coverage for copyreg basic API surface."""

import copyreg


def reducer(obj):
    return (str, ("reduced",))

copyreg.pickle(int, reducer)
print(callable(copyreg.dispatch_table.get(int)))
