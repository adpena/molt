"""Purpose: differential coverage for metaclass __prepare__ order."""

log = []


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        log.append(("prepare", name))
        return {}


class Demo(metaclass=Meta):
    x = 1


if __name__ == "__main__":
    print("log", log)
    print("attr", Demo.x)
