def passthrough(fn):
    def wrapper(*args, **kwargs):
        return fn(*args, **kwargs)

    return wrapper


@passthrough
def handle(request, *, user="anon"):
    return (request, user)


print(handle("req-1"))
print(handle("req-2", user="alice"))


class View:
    def __init__(self, name):
        self.name = name

    def __call__(self, request, *, user=None):
        return (self.name, request, user)


view = View("main")
print(view("req-3", user="bob"))


class Base:
    def dispatch(self, request, *, user=None):
        return ("base", request, user)


class Child(Base):
    def dispatch(self, request, *, user=None):
        return ("child", super().dispatch(request, user=user))


child = Child()
print(child.dispatch("req-4", user="carol"))
