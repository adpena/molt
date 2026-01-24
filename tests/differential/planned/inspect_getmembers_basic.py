"""Purpose: ensure inspect.getmembers lists attributes."""

import inspect


class C:
    def method(self):
        return 1


members = dict(inspect.getmembers(C))
print("method" in members, "__name__" in members)
