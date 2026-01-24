"""Purpose: differential coverage for __classcell__ missing errors."""


class Meta(type):
    def __new__(mcls, name, bases, ns):
        # Drop __classcell__ to emulate misbehaving metaclass.
        ns.pop("__classcell__", None)
        return super().__new__(mcls, name, bases, ns)


try:
    class Base:
        def hello(self):
            return "base"

    class Broken(Base, metaclass=Meta):
        def hello(self):
            return super().hello() + "+broken"
except Exception as exc:
    print("classcell", type(exc).__name__)
