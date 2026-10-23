"""Purpose: sys.is_finalizing() is False in atexit callbacks and True once
module teardown begins: CPython sets it after the atexit phase."""

import atexit
import sys


class Witness:
    def __init__(self, is_finalizing):
        self.is_finalizing = is_finalizing

    def __del__(self):
        print("teardown: finalizing", self.is_finalizing())


witness = Witness(sys.is_finalizing)
atexit.register(lambda: print("atexit: finalizing", sys.is_finalizing()))
print("main: finalizing", sys.is_finalizing())
