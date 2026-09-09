"""Expression-result, evaluation and source-point binding parity capsule.

No host-specific values are printed. Replay unchanged against each supported
CPython version and both compiler targets; a reference run is not target proof.
"""

print("empty-expansions", bool([*()]), bool((*(),)), bool({*()}), bool({**{}}))
print("boolean-comparisons", 2 == True, [1] == True, 1 is True)  # noqa: E712, F632

events = []


def event(name):
    events.append(name)
    return 1


if [event("condition")]:
    events.append("body")
else:
    events.append("dead")
print("condition-order", events)
events.clear()


class Finalizer:
    def __del__(self):
        events.append("finalizer")


if [Finalizer()]:
    events.append("body")
print("condition-release", events)
events.clear()

try:
    if [missing_condition]:
        print("unreachable")
except NameError:
    print("condition-nameerror")


class RichComparison:
    def __eq__(self, other):
        return []


rich = RichComparison()
print("rich-result", (rich == 0 == 1) is False)


class ComparisonTruth:
    def __bool__(self):
        global comparison_marker
        events.append("truth")
        comparison_marker = True
        return True


class ComparisonMutation:
    def __eq__(self, other):
        events.append("compare")
        return ComparisonTruth()


comparison_probe = ComparisonMutation()
comparison_marker = False
print("chain-callback", comparison_probe == 0 == comparison_marker)
print("chain-callback-order", events, comparison_marker)
events.clear()

try:
    print(TYPE_CHECKING)
except NameError:
    print("type-checking-nameerror")


def parameter_gate(TYPE_CHECKING):
    if TYPE_CHECKING:
        return "parameter-true"
    return "parameter-false"


print(parameter_gate(True), parameter_gate(False))


class Shadow:
    TYPE_CHECKING = True
    if TYPE_CHECKING:
        selected = "class-true"


print(Shadow.selected)

try:
    if future_sys.platform == "win32":
        print("unreachable-future-import")
except NameError:
    print("future-import-nameerror")
import sys as future_sys  # noqa: E402, F401 - future binding is the semantic probe

import typing  # noqa: E402 - source-ordered member-owner evaluation probe

if (typing_owner := typing).TYPE_CHECKING:
    print("unreachable-typing-owner")
print("typing-owner", typing_owner is typing)


class Key:
    def __hash__(self):
        events.append("hash")
        return 7


def key():
    events.append("key")
    return Key()


def value():
    events.append("value")
    return 1


def tail():
    events.append("tail")
    return 4


built = {key(): value(), 4: tail()}
print("dict-segment", events)
events.clear()
built = {key(), tail()}
print("set-segment", events)
events.clear()
built = {key(), *(), tail()}
print("set-expansion-segment", events)
events.clear()
built = {key(): value(), **{}, 4: tail()}
print("dict-expansion-segment", events)

print("print-result", print("inner-print"))
print("empty-print-result", print())
print("multi-print-result", print("left", "right"))


class PrintSink:
    def write(self, text):
        events.append("write:" + text)


class Printed:
    def __init__(self, text, fails=False):
        self.text = text
        self.fails = fails

    def __str__(self):
        events.append("str:" + self.text)
        if self.fails:
            raise ValueError("print conversion")
        return self.text


events.clear()
try:
    print(Printed("first"), Printed("second", fails=True), file=PrintSink())
except ValueError:
    print("print-partial-effects", events)
