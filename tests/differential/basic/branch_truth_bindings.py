"""Purpose: truth callbacks publish globals before branch consumers execute."""


def original():
    return "original"


def replacement():
    return "replacement"


def clean():
    return "clean"


class Switch:
    def __init__(self, value):
        self.value = value

    def __bool__(self):
        global target
        target = replacement
        return self.value


target = original
if Switch(False):
    print("unreachable")
else:
    print("if-false", target())

target = original
if Switch(True):
    print("if-entry", target())
    target = clean
print("if-clean", target())

target = original
selected = (target := clean) if Switch(True) else original
print("if-expression", selected(), target())

target = original
print("and", Switch(True) and target())
target = original
print("or", Switch(False) or target())
target = original
selected = Switch(True) and (target := clean)
print("and-walrus", selected(), target())

target = original
count = 0
while Switch(count < 1):
    print("while-entry", target())
    target = clean
    count += 1
print("while-exit", target())

target = original
filtered = [target() for value in (True, False, True) if Switch(value)]
print("filter", filtered)

target = original
try:
    assert Switch(False), target()
except AssertionError as error:
    print("assert-message", str(error))
