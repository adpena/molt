"""Purpose: zero-argument super() decides "no arguments" from co_argcount and reads argument zero from its first slot."""


class Base:
    def name(self):
        return "base"


class Derived(Base):
    def positional(self):
        return super().name()

    def no_positional():
        return super()

    def star_only(*args):
        return super()

    def keyword_only(*, receiver):
        return super()

    def rebound(self):
        self = Derived()
        return super().name()

    def deleted(self):
        del self
        return super()

    def captured(self):
        def inner():
            return self

        return super().name(), inner() is self

    def default_value(self=None):
        return super()

    # A class body is not a function: it has no argument zero.
    try:
        super()
    except RuntimeError as exc:
        body = str(exc)


def report(label, call):
    try:
        print(label, call())
    except RuntimeError as exc:
        print(label, "RuntimeError", exc)
    except TypeError as exc:
        print(label, "TypeError", exc)


def plain():
    return super()


def plain_with_argument(value):
    return super()


if __name__ == "__main__":
    instance = Derived()
    print("class body", Derived.body)
    report("positional", instance.positional)
    report("no positional", Derived.no_positional)
    report("star only", lambda: Derived.star_only(instance))
    report("keyword only", lambda: Derived.keyword_only(receiver=instance))
    report("rebound", instance.rebound)
    report("deleted", instance.deleted)
    report("captured", instance.captured)
    report("default value", Derived.default_value)
    report("plain", plain)
    report("plain with argument", lambda: plain_with_argument(instance))
