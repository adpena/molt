"""Purpose: a failed name or attribute lookup keeps CPython's plain text and its `name`.

CPython puts any "Did you mean" hint only in the rendered traceback, computed
from the exception's `name` (and `obj`); `str(exc)` stays plain.
"""

number_of_items = 3


def show(error):
    print(type(error).__name__, repr(str(error)), getattr(error, "name", None))


try:
    number_of_itemz
except NameError as error:
    show(error)

try:
    Tru
except NameError as error:
    show(error)


def local_lookup():
    counter_total = 1
    return counter_totl


try:
    local_lookup()
except NameError as error:
    show(error)


def unbound_local():
    print(late)
    late = 1


try:
    unbound_local()
except UnboundLocalError as error:
    show(error)


class Box:
    def __init__(self):
        self.payload = 1


try:
    Box().payloda
except AttributeError as error:
    show(error)
    print(type(error.obj).__name__)

built = NameError("custom", name="value")
print(repr(str(built)), built.name)
built = AttributeError("custom", name="field", obj=3)
print(repr(str(built)), built.name, built.obj)
