"""Class hooks and object constructor adapters share one binding policy."""


def outcome(label, action):
    try:
        result = action()
    except Exception as error:
        print(label, type(error).__name__)
    else:
        marker = getattr(result, "marker", None)
        print(label, "ok", type(result).__name__, marker)


hook_events = []


class HookLeft:
    def __init_subclass__(cls, *, left=None, **kwargs):
        hook_events.append(("left", cls.__name__, left, tuple(kwargs.items())))
        super().__init_subclass__(**kwargs)


class HookRight:
    def __init_subclass__(cls, *, right=None, **kwargs):
        hook_events.append(("right", cls.__name__, right, tuple(kwargs.items())))
        super().__init_subclass__(**kwargs)


class HookBare(HookLeft):
    pass


class HookSubject(HookLeft, HookRight, left="L", right="R"):
    pass


print("hooks", hook_events)
outcome("hook-bound-object", lambda: object.__init_subclass__())


class Neither:
    pass


class InitOnly:
    def __init__(self, value=None, *, flag=None):
        self.marker = ("init", value, flag)


class NewOnly:
    def __new__(cls, value=None, *, flag=None):
        instance = super().__new__(cls)
        instance.marker = ("new", value, flag)
        return instance


class Both:
    def __new__(cls, value=None, *, flag=None):
        instance = super().__new__(cls)
        instance.marker = ("new", value, flag)
        return instance

    def __init__(self, value=None, *, flag=None):
        self.marker = ("init", value, flag)


for name, cls in (
    ("neither", Neither),
    ("init-only", InitOnly),
    ("new-only", NewOnly),
    ("both", Both),
):
    outcome(f"{name}:construct-pos", lambda cls=cls: cls("P"))
    outcome(f"{name}:construct-kw", lambda cls=cls: cls(flag="K"))
    outcome(f"{name}:object-new-pos", lambda cls=cls: object.__new__(cls, "P"))
    outcome(
        f"{name}:object-new-kw",
        lambda cls=cls: object.__new__(cls, flag="K"),
    )
    outcome(
        f"{name}:object-init-pos",
        lambda cls=cls: object.__init__(cls(), "P"),
    )
    outcome(
        f"{name}:object-init-kw",
        lambda cls=cls: object.__init__(cls(), flag="K"),
    )
