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
        return {}


class Mixed(Adapter(), metaclass=Meta):
    pass


print("events", events)
print("extra", Mixed().extra())
