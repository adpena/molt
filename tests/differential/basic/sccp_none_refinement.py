# None tests that the compiler may decide from earlier tests and producers:
# a wrong nullness fold changes these results. Every tested value is computed.


def default_list(items=None):
    if items is None:
        items = []
    if items is None:
        return "unreachable: a default list is never None"
    items.append(len(items))
    return items


def default_none_again(value=None):
    if value is None:
        value = None
    return value is None


def guard_then_retest(value):
    if value is None:
        return "none"
    if value is not None:
        return "kept " + type(value).__name__
    return "unreachable"


def negated_guard(value):
    if not (value is None):
        return "set"
    if value is None:
        return "unset"
    return "unreachable"


def boolean_wrapped(value):
    if bool(value is not None):
        return value is None
    return value is not None


def merged_paths(flag, value):
    if flag:
        value = None
    elif value is None:
        value = 0
    return value is None


def falsy_but_not_none(value):
    # 0, "" and [] are falsy and not None: truth must not stand in for nullness.
    if not value:
        return value is None
    return "truthy"


def in_loop(values):
    seen = 0
    for value in values:
        if value is None:
            continue
        if value is not None:
            seen += 1
    return seen


def with_exception(value):
    try:
        if value is None:
            raise ValueError("none")
        result = value is not None
    except ValueError:
        result = value is None
    return result


for case in (None, 0, "", [], "x", 3):
    print(default_list() if case is None else default_list([case]))
    print(default_none_again(case), guard_then_retest(case), negated_guard(case))
    print(boolean_wrapped(case), merged_paths(False, case), merged_paths(True, case))
    print(falsy_but_not_none(case), with_exception(case))
print(in_loop([None, 1, None, "", 0]))
