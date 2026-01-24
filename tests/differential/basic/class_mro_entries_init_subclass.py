"""Purpose: differential coverage for __mro_entries__ with __init_subclass__."""


events = []


class Base:
    def __init_subclass__(cls, **kwargs):
        events.append(f"init_subclass:{cls.__name__}")
        super().__init_subclass__(**kwargs)


class Adapter:
    def __mro_entries__(self, bases):
        class Extra(Base):
            pass

        return (Extra,)


class Child(Adapter()):
    pass


print("events", events)
