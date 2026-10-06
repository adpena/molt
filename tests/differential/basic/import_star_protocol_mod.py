"""Fixture for compiled IMPORT_STAR indexed protocol differential tests."""
_mode = ""

class IndexedOnly:
    def __getitem__(self, index):
        if index == 0:
            return "x"
        raise IndexError

    def __iter__(self):
        raise RuntimeError("iteration must not be used")

class AfterFirst:
    def __getitem__(self, index):
        if index == 0:
            return "x"
        raise ValueError("after first")

class StopItem:
    def __getitem__(self, index):
        raise StopIteration("not exhaustion")

class End(IndexError):
    pass

class EndSubclass:
    def __getitem__(self, index):
        raise End("exhausted")

def __getattr__(name):
    if _mode == "all_lookup_nonattribute_error":
        raise ValueError("all lookup error")
    if name == "__path__":
        if _mode == "path_lookup_nonattribute_error":
            raise ValueError("package path lookup error")
        if _mode == "dynamic_package_indexed_only":
            return None
    if _mode == "dynamic_all" and name == "__all__":
        return ["_hidden"]
    if _mode == "getattr_value" and name == "dynamic":
        return 99
    raise AttributeError(name)

def configure(mode):
    global _mode, alpha, x, _hidden
    _mode = mode
    alpha = 1
    x = 7
    _hidden = 11
    namespace = globals()
    namespace["__name__"] = "import_star_protocol_mod"
    namespace.pop("__all__", None)
    namespace.pop("__path__", None)
    namespace.pop(42, None)
    if mode == "generator_all":
        namespace["__all__"] = (name for name in ["x"])
    elif mode == "set_all":
        namespace["__all__"] = {"x"}
    elif mode in ("indexed_only", "package_indexed_only", "dynamic_package_indexed_only"):
        namespace["__all__"] = IndexedOnly()
        if mode == "package_indexed_only":
            namespace["__path__"] = None
    elif mode == "invalid_all_partial":
        namespace["__all__"] = ["x", 3]
    elif mode == "getattr_value":
        namespace["__all__"] = ["dynamic"]
    elif mode == "nonstring_dict_key":
        namespace[42] = 8
    elif mode == "index_error_propagation":
        namespace["__all__"] = AfterFirst()
    elif mode == "stop_iteration_propagation":
        namespace["__all__"] = StopItem()
    elif mode == "index_error_subclass":
        namespace["__all__"] = EndSubclass()
    elif mode == "mapping_all":
        namespace["__all__"] = {0: "x"}
    elif mode == "deleted_name_invalid_all":
        namespace["__all__"] = [1]
        namespace.pop("__name__")
    elif mode == "integer_name_invalid_all":
        namespace["__all__"] = [1]
        namespace["__name__"] = 42
