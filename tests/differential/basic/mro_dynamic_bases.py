"""Purpose: differential coverage for dynamic __mro_entries__ bases."""

class Base:
    def __init_subclass__(cls, **kwargs):
        cls.tag = "base"


class Wrapper:
    def __mro_entries__(self, bases):
        return (Base,)


class Demo(Wrapper()):
    pass


if __name__ == "__main__":
    print("bases", [cls.__name__ for cls in Demo.__mro__])
    print("tag", Demo.tag)
