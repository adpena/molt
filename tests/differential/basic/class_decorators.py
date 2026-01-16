class Decorator:
    def __init__(self, tag):
        self.tag = tag

    def __call__(self, cls):
        cls.tag = self.tag
        return cls


def deco_default(cls, flag=True):
    cls.flag = flag
    return cls


def deco_tag(cls):
    cls.note = "ok"
    return cls


@deco_default
@deco_tag
class Alpha:
    pass


print(Alpha.flag, Alpha.note)


def make_local():
    @deco_default
    class Local:
        pass

    return Local.flag


print(make_local())


class Wrapper:
    def __init__(self, value):
        self.value = value

    def decorate(self, cls):
        cls.value = self.value
        return cls


wrap = Wrapper(3)


@wrap.decorate
class Beta:
    pass


print(Beta.value)

decor = Decorator("callable")


@decor
class Gamma:
    pass


print(Gamma.tag)

trace = []


def factory(tag):
    trace.append(f"factory:{tag}")

    def deco(cls):
        trace.append(f"apply:{tag}:{cls.__name__}")
        cls.tag = tag
        return cls

    return deco


def side_effect(name):
    trace.append(f"side:{name}")

    def deco(cls):
        trace.append(f"apply:{name}:{cls.__name__}")
        cls.side = name
        return cls

    return deco


@factory("A")
@side_effect("B")
class Delta:
    pass


print(trace)
print(Delta.tag, Delta.side)


def outer(flag):
    def deco(cls):
        cls.flag = flag
        return cls

    return deco


@outer(True)
@outer(False)
class Epsilon:
    pass


print(Epsilon.flag)

order = []


def stamped(tag):
    order.append(f"deco_eval:{tag}")

    def apply(cls):
        order.append(f"deco_apply:{tag}:{cls.__name__}")
        cls.tags = cls.tags + [tag]
        return cls

    return apply


@stamped("first")
@stamped("second")
class Zeta:
    tags = []
    order.append("body:Zeta")


print(order)
print(Zeta.tags)

more = []


def tagged(tag, *, suffix="x"):
    more.append(f"factory:{tag}:{suffix}")

    def deco(cls):
        more.append(f"apply:{tag}:{cls.__name__}:{suffix}")
        cls.label = f"{tag}-{suffix}"
        return cls

    return deco


class ObjDecorator:
    def __init__(self, tag):
        more.append(f"obj_init:{tag}")
        self.tag = tag

    def __call__(self, cls):
        more.append(f"obj_apply:{self.tag}:{cls.__name__}")
        cls.obj = self.tag
        return cls


@tagged("K", suffix="s")
@ObjDecorator("obj")
class Theta:
    more.append("body:Theta")


print(more)
print(Theta.label, Theta.obj)
