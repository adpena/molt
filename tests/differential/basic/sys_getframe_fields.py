"""Purpose: differential coverage for sys._getframe fields."""

import sys

frame = sys._getframe()
print(isinstance(frame.f_globals, dict))
print(isinstance(frame.f_locals, dict))
print(frame.f_globals.get("__name__") == __name__)

def inner():
    f = sys._getframe()
    print(f.f_back is not None)
    print(isinstance(f.f_back.f_globals, dict))

inner()
