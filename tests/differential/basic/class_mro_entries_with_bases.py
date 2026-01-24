"""Purpose: differential coverage for __mro_entries__ mixed with bases."""


class Base:
    def base(self):
        return "base"


class Adapter:
    def __mro_entries__(self, bases):
        class Extra:
            def extra(self):
                return "extra"

        return (Extra,)


class Mixed(Base, Adapter()):
    pass


m = Mixed()
print("base", m.base())
print("extra", m.extra())
