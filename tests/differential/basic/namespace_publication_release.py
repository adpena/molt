"""Namespace syntax/method aliases publish before releasing the old value."""

events = []


class Previous:
    def __del__(self):
        namespace = globals()
        events.append(namespace.get("__package__", "<absent>"))
        namespace["__package__"] = "callback.pkg"


for mode in ("store", "setitem", "alias", "delete", "delitem", "delete_alias"):
    globals()["__package__"] = Previous()
    if mode == "store":
        globals()["__package__"] = "claimed.pkg"
    elif mode == "setitem":
        globals().__setitem__("__package__", "claimed.pkg")
    elif mode == "alias":
        put = globals().__setitem__
        put("__package__", "claimed.pkg")
    elif mode == "delete":
        del globals()["__package__"]
    elif mode == "delitem":
        globals().__delitem__("__package__")
    else:
        remove = globals().__delitem__
        remove("__package__")
    print(mode, events[-1], globals()["__package__"])
