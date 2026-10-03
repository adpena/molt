"""Purpose: live class namespaces retain the full read-only mapping protocol."""

from types import MappingProxyType


class Sample:
    value = 3


proxy = Sample.__dict__
print(type(proxy).__name__)
print(proxy.get("value"), proxy.get("missing", 17))
print("value" in proxy.keys(), ("value", 3) in proxy.items(), 3 in proxy.values())
print("value" in list(proxy), len(proxy) == len(list(proxy)))
Sample.value = 9
Sample.other = 12
print(proxy["value"], proxy["other"])
del Sample.other
print("other" in proxy)
print(proxy.copy()["value"])
print(set(dir(Sample)) >= {"value", "__class__", "__dict__"})
print(vars(Sample)["value"])
for action in ("set", "delete"):
    try:
        if action == "set":
            proxy["other"] = 1
        else:
            del proxy["value"]
    except TypeError:
        print(action, "TypeError")

mapping = {"first": 1, "second": 2}
view = MappingProxyType(mapping)
print(list(reversed(view)))
print(view == mapping, view != mapping)
print(str(view) == str(mapping))
try:
    hash(view)
except TypeError:
    print("unhashable backing mapping")
try:
    view |= {"forbidden": 0}
except TypeError:
    print("read-only in-place union")
print(view | {"third": 3})
print({"zero": 0} | view)
mapping["third"] = 3
print(view.get("third"), list(view.keys()))


class Custom:
    def __getitem__(self, key):
        return mapping[key]

    def get(self, key, default):
        if key == "error":
            raise RuntimeError("mapping-get-failed")
        return ("custom", key, default)


custom = MappingProxyType(Custom())
print(custom.get("first"), custom.get("missing", 8))
try:
    custom.get("error")
except RuntimeError as error:
    print(str(error))
