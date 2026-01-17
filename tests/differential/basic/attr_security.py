def show_exc(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, repr(exc))


class BoomGet:
    def __get__(self, obj, owner):
        raise RuntimeError("boom-get")


class BoomSet:
    def __set__(self, obj, value):
        raise ValueError(f"boom-set:{value}")


class BoomDel:
    def __delete__(self, obj):
        raise KeyError("boom-del")


class NonCallableGet:
    __get__ = 1


class Guard:
    x = BoomGet()
    y = BoomSet()
    z = BoomDel()
    bad = NonCallableGet()

    def __getattr__(self, name):
        return f"fallback:{name}"


g = Guard()
show_exc("desc_get", lambda: g.x)
show_exc("desc_set", lambda: setattr(g, "y", 4))
show_exc("desc_del", lambda: delattr(g, "z"))
show_exc("desc_noncall", lambda: getattr(g, "bad"))
print("fallback_missing", g.missing)


class AttrGet:
    def __get__(self, obj, owner):
        raise AttributeError("attr-get")


class Fallback:
    attr = AttrGet()

    def __getattr__(self, name):
        return f"fallback:{name}"


f = Fallback()
try:
    print("attr_get_value", f.attr)
except AttributeError as exc:
    print("attr_get_error", type(exc).__name__, repr(exc))


class RecAttr:
    def __getattr__(self, name):
        return getattr(self, name)


r = RecAttr()
show_exc("getattr_rec", lambda: r.missing)


class RecGetAttribute:
    def __getattribute__(self, name):
        return getattr(self, name)


rg = RecGetAttribute()
show_exc("getattribute_rec", lambda: rg.missing)
