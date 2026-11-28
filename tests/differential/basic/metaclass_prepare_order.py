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


class InheritedPrepare(type):
    def __new__(mcls, name, bases, namespace, *, flag):
        namespace["received_flag"] = flag
        return super().__new__(mcls, name, bases, namespace)


class HeaderKeywords(metaclass=InheritedPrepare, flag=42):
    pass


first = type.__prepare__("Direct", (), arbitrary=True)
first["independent"] = True
print(type.__prepare__(), type.__prepare__(0, 1, 2, any_name=None))
print(HeaderKeywords.received_flag)
