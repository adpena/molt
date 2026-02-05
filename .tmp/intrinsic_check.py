import builtins

for name in (
    "_molt_class_new",
    "molt_class_new",
    "_molt_class_set_base",
    "molt_class_set_base",
):
    val = getattr(builtins, name, None)
    print(name, val is not None, type(val).__name__)
