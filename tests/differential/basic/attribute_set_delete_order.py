"""Purpose: differential coverage for __setattr__/__delattr__ order."""

log = []


class Base:
    def __setattr__(self, name, value):
        log.append(("set", name, value))
        super().__setattr__(name, value)

    def __delattr__(self, name):
        log.append(("del", name))
        super().__delattr__(name)


if __name__ == "__main__":
    base = Base()
    base.value = 10
    del base.value
    print("log", log)
