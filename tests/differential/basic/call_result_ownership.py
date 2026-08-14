"""Python calls publish exactly one owned result across every binding family."""

import gc
import weakref


class Box:
    def __init__(self, label):
        self.label = label


def direct_alias(value):
    return value


def direct_fresh(label):
    return {label}


def full_alias(head, *rest, value, **metadata):
    return value


def full_fresh(head, *rest, label, **metadata):
    return {label}


class Router:
    def bound_alias(self, value):
        return value

    def bound_fresh(self, label):
        return {label}

    def bound_full_alias(self, head, *rest, value, **metadata):
        return value

    def bound_full_fresh(self, head, *rest, label, **metadata):
        return {label}


def direct_fresh_case():
    result = direct_fresh("direct-fresh")
    ref = weakref.ref(result)
    observed = "direct-fresh" in result
    del result
    gc.collect()
    return (observed, ref() is None)


def full_fresh_case():
    result = full_fresh("head", "tail", label="full-fresh", marker=1)
    ref = weakref.ref(result)
    observed = "full-fresh" in result
    del result
    gc.collect()
    return (observed, ref() is None)


def bound_fresh_case(router):
    result = router.bound_fresh("bound-fresh")
    ref = weakref.ref(result)
    observed = "bound-fresh" in result
    del result
    gc.collect()
    return (observed, ref() is None)


def bound_full_fresh_case(router):
    result = router.bound_full_fresh("head", "tail", label="bound-full-fresh", marker=1)
    ref = weakref.ref(result)
    observed = "bound-full-fresh" in result
    del result
    gc.collect()
    return (observed, ref() is None)


def direct_alias_case():
    value = Box("direct-alias")
    ref = weakref.ref(value)
    result = direct_alias(value)
    observed = (result is value, result.label)
    value = None
    gc.collect()
    owned = ref() is result
    result = None
    gc.collect()
    return observed + (owned, ref() is None)


def full_alias_case():
    value = Box("full-alias")
    ref = weakref.ref(value)
    result = full_alias("head", "tail", value=value, marker=1)
    observed = (result is value, result.label)
    value = None
    gc.collect()
    owned = ref() is result
    result = None
    gc.collect()
    return observed + (owned, ref() is None)


def bound_alias_case(router):
    value = Box("bound-alias")
    ref = weakref.ref(value)
    result = router.bound_alias(value)
    observed = (result is value, result.label)
    value = None
    gc.collect()
    owned = ref() is result
    result = None
    gc.collect()
    return observed + (owned, ref() is None)


def bound_full_alias_case(router):
    value = Box("bound-full-alias")
    ref = weakref.ref(value)
    result = router.bound_full_alias("head", "tail", value=value, marker=1)
    observed = (result is value, result.label)
    value = None
    gc.collect()
    owned = ref() is result
    result = None
    gc.collect()
    return observed + (owned, ref() is None)


def main():
    router = Router()
    print("direct-alias", *direct_alias_case())
    print("direct-fresh", *direct_fresh_case())
    print("full-alias", *full_alias_case())
    print("full-fresh", *full_fresh_case())
    print("bound-alias", *bound_alias_case(router))
    print("bound-fresh", *bound_fresh_case(router))
    print("bound-full-alias", *bound_full_alias_case(router))
    print("bound-full-fresh", *bound_full_fresh_case(router))


if __name__ == "__main__":
    main()
