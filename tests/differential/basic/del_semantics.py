"""Purpose: differential coverage for del semantics."""

x = "root"


def del_local():
    try:
        del y
    except Exception as exc:
        print("del_local_unbound", type(exc).__name__)
    y = 10
    del y
    try:
        y
    except Exception as exc:
        print("del_local_after", type(exc).__name__)


def del_global():
    global x
    print("del_global_before", x)
    del x
    try:
        x
    except Exception as exc:
        print("del_global_after", type(exc).__name__)


class Box:
    def __init__(self):
        self.value = 1

    def __delattr__(self, name):
        print("del_attr", name)
        object.__delattr__(self, name)


class Bag:
    def __init__(self):
        self.data = {"k": 1}

    def __delitem__(self, key):
        print("del_item", key)
        del self.data[key]


box = Box()
del box.value

bag = Bag()
del bag["k"]

try:
    del box.missing
except Exception as exc:
    print("del_attr_missing", type(exc).__name__)

try:
    del bag["missing"]
except Exception as exc:
    print("del_item_missing", type(exc).__name__)


del_local()
del_global()
