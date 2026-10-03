"""Every attribute lookup result carries exactly one owned reference."""

import gc
import weakref


class Box:
    pass


class Holder:
    def __init__(self, value):
        self.value = value

    @property
    def via_property(self):
        return self.value

    def via_method(self):
        return self.value


def attribute_lifetime_refs():
    value = Box()
    holder = Holder(value)
    value_ref = weakref.ref(value)
    holder_ref = weakref.ref(holder)

    direct = holder.value
    named = getattr(holder, "value")
    fallback_default = holder.value
    fallback = getattr(holder, "missing", fallback_default)
    fallback_inline = getattr(holder, "also_missing", holder.value)
    property_value = holder.via_property
    method = holder.via_method
    method_value = method()
    if not (
        direct is value
        and named is value
        and fallback is value
        and fallback_default is value
        and fallback_inline is value
        and property_value is value
        and method_value is value
    ):
        raise AssertionError("attribute lookup changed object identity")
    return value_ref, holder_ref


value_ref, holder_ref = attribute_lifetime_refs()
gc.collect()
print("attribute-released", value_ref() is None, holder_ref() is None)


events = []


class Observed:
    def __init__(self, value):
        self.value = value

    def __del__(self):
        events.append("receiver released")

    @property
    def property_value(self):
        events.append("property read")
        return self.value

    def __getitem__(self, key):
        events.append("item read")
        return self.value


def consume(value):
    events.append("consume " + value)


def owned_reads():
    consume(Observed("attribute").value)
    consume(Observed("property").property_value)
    consume(Observed("item")[0])
    consume([Observed("list item")][0].value)
    consume((Observed("tuple item"),)[0].value)
    consume({"key": Observed("dict item")}["key"].value)


owned_reads()
print("owned-read-order", events)
