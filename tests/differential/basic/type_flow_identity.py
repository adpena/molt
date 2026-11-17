"""Type flow (design 78 section 4): a specialized entry is the same Python
function. Code object, qualname, name, line numbers and tracebacks stay the
original's whatever argument types reach it.
"""

import traceback


def area(w, h):
    if w < 0:
        raise ValueError("negative width")
    return w * h


class Box:
    def volume(self, d):
        return area(2, 3) * d


f = area
print(area(2, 3), area(2.5, 2), f is area)
print(area.__name__, area.__qualname__, area.__code__.co_name)
print(area.__code__.co_firstlineno, Box.volume.__qualname__)
print(Box().volume(4))
try:
    area(-1, 3)
except ValueError:
    frames = traceback.extract_tb(__import__("sys").exc_info()[2])
    print([(frame.name, frame.lineno) for frame in frames])
