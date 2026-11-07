"""Replacing the builtin `print` reaches every call of it (HF-776).

Output goes through `sys.stdout.write`, which no replacement touches.
"""

import builtins
import sys

ORIGINAL = builtins.print


class Fake:
    def __init__(self, name):
        self.name = name

    def __call__(self, *args, **kwargs):
        sys.stdout.write("called-" + self.name + " " + " ".join(map(str, args)) + "\n")
        return "result-" + self.name


def use(text):
    return print(text, end="!\n")


def report(label, value):
    sys.stdout.write(label + " " + str(value) + "\n")


report("original", use("one"))
print = Fake("print-module")
report("module binding", use("two"))
report("module binding direct", print("three"))
del print
report("module binding deleted", use("four"))
builtins.print = Fake("print-member")
report("member", use("five"))
report("member direct", print("six"))
builtins.print = ORIGINAL
report("member restored", use("seven"))
custom = dict(builtins.__dict__)
custom["print"] = Fake("print-mapping")
__builtins__ = custom


def use_after(text):
    return print(text)


report("mapping before", use("eight"))
report("mapping after", use_after("nine"))
