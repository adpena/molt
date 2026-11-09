# A failing main module stays in sys.modules (CPython keeps __main__), and its
# globals, including an object with native storage, are released at shutdown.
import array
import atexit
import sys

atexit.register(lambda: print("atexit: __main__ published", "__main__" in sys.modules))
held = array.array("b", [1, 2])
print("before failure", held.tolist())
raise ValueError("main fails")
