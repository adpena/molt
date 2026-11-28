"""Purpose: differential coverage for __mro_entries__ with metaclass __prepare__."""

events = []


class Adapter:
    def __mro_entries__(self, bases):
        class Extra:
            def extra(self):
                return "extra"

        return (Extra,)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        events.append("prepare")
        return TrackingNamespace()


class TrackingNamespace(dict):
    def __setitem__(self, key, value):
        if key == "__orig_bases__":
            events.append(("original bases stored", value[0] is adapter))
        return super().__setitem__(key, value)


adapter = Adapter()


class Mixed(adapter, metaclass=Meta):
    events.append(("body before original bases", "__orig_bases__" not in locals()))


print("original base identity", Mixed.__orig_bases__[0] is adapter)


print("events", events)
print("extra", Mixed().extra())
