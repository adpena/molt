"""Purpose: differential coverage for args kwargs eval order."""

events = []


def tag(label):
    events.append(label)
    return label


def f(*args, **kwargs):
    return (args, kwargs)


f(a=tag("kw_a"), *[tag("star_a")])
print(events)

events = []
f(tag("pos"), b=tag("kw_b"), *[tag("star_b")], **{"c": tag("kw_c")}, d=tag("kw_d"))
print(events)


class PreparedValue:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append("drop " + self.label)


class BrokenArguments:
    def __iter__(self):
        events.append("iter")
        yield PreparedValue("expanded")
        events.append("star failure")
        raise ValueError("star failure")


class BrokenKeywords:
    def keys(self):
        events.append("keys")
        raise LookupError("keys failure")


def failed_keyword_value():
    events.append("value failure")
    raise RuntimeError("value failure")


def must_not_call(*args, **kwargs):
    events.append("wrong callee")


def positional_preparation_failure():
    must_not_call(PreparedValue("prefix"), *BrokenArguments(), tag("wrong later"))


def keyword_preparation_failure():
    must_not_call(PreparedValue("prefix"), **BrokenKeywords(), later=tag("wrong later"))


def duplicate_preparation_failure():
    must_not_call(PreparedValue("prefix"), key=1, **{"key": 2}, later=tag("wrong later"))


def expression_preparation_failure():
    must_not_call(PreparedValue("prefix"), value=failed_keyword_value())


for preparation in (
    positional_preparation_failure,
    keyword_preparation_failure,
    duplicate_preparation_failure,
    expression_preparation_failure,
):
    events.clear()
    try:
        preparation()
    except Exception as error:
        # Traceback ownership can defer finalization until the handler exits.
        failure_name = type(error).__name__
    else:
        raise AssertionError("argument preparation failure was lost")
    print(preparation.__name__, failure_name, events)

# Argument-vector and frame ownership across preparation and dispatch.
events = []


class Value:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(self.label)


class BrokenStar:
    def __iter__(self):
        events.append("star iter")
        raise ValueError("star failure")

    def __del__(self):
        events.append("star")


def fail():
    events.append("value failure")
    raise ValueError("value failure")


def variadic(*args, **kwargs):
    pass


def mixed(a, *rest, k, **kw):
    pass


def one(a):
    pass


def two(a, b):
    pass


def zero():
    pass


class Init:
    def __init__(self, *args, **kwargs):
        pass


def multi_mapping_failure():
    variadic(Value("p1"), Value("p2"), a=Value("k1"), **{"b": Value("k2")}, **fail())


def deferred_star_failure():
    variadic(*BrokenStar(), a=Value("k1"))


def mixed_frame_order():
    mixed(Value("a"), Value("r1"), Value("r2"), k=Value("k"), x=Value("x"))


def surplus_duplicate_failure():
    one(Value("p1"), Value("p2"), a=Value("k1"))


def remaining_keywords_failure():
    two(Value("p1"), b=Value("k1"), c=Value("k2"), d=Value("k3"))


def class_dispatch():
    Init(Value("p1"), Value("p2"), k=Value("k1"))


def builtin_dispatch():
    fmt = "".format
    fmt(Value("p1"), Value("p2"), a=Value("k1"), b=Value("k2"))


def expanded_binding_failure():
    zero(Value("p1"), Value("p2"), *(), a=Value("k1"), b=Value("k2"))


def expanded_builtin_dispatch():
    fmt = "".format
    fmt(Value("p1"), Value("p2"), *(), a=Value("k1"), b=Value("k2"))


def plain_variadic_dispatch():
    variadic(Value("p1"), Value("p2"))


def frame_with_locals(argument):
    # Declaration order deliberately differs from alphabetical order. Parameter
    # custody and local custody must meet at the same frame-exit boundary.
    z_first = Value("first local")
    a_second = Value("second local")
    events.append(argument.label + ":" + z_first.label + ":" + a_second.label)


def parameter_local_order():
    frame_with_locals(Value("parameter"))


def expanded_parameter_local_order():
    frame_with_locals(*(Value("parameter"),))


def rebind_frame(argument):
    argument = Value("replacement")
    z_local = Value("local")
    events.append(argument.label + ":" + z_local.label)


def parameter_rebind_order():
    rebind_frame(Value("original"))


def return_parameter(argument):
    return argument


def returned_parameter_alias():
    original = Value("returned")
    returned = return_parameter(original)
    assert returned is original
    del original
    events.append("caller released")
    assert returned.label == "returned"
    del returned
    events.append("result released")


def throwing_frame(argument):
    z_local = Value("traceback local")
    events.append(argument.label + ":" + z_local.label)
    raise ValueError("keep frame")


def traceback_frame_lifetime():
    held = []
    try:
        throwing_frame(Value("traceback parameter"))
    except ValueError as error:
        held.append(error)
    events.append("handler left")
    held.clear()
    events.append("traceback released")


def consume_attribute(value):
    events.append("consume " + value)


def owned_attribute_argument():
    # A normal attribute result owns its reference independently of the
    # temporary receiver. An actual borrowed handle requires a distinct fact.
    consume_attribute(Value("receiver").label)
    events.append("call returned")


class Receiver(Value):
    def replace(self):
        events.append("entered " + self.label)
        self = Receiver("replacement")
        events.append("rebound")


def temporary_method():
    Receiver("temporary").replace()
    events.append("caller resumed")


def retained_bound_method():
    method = Receiver("retained").replace
    method()
    events.append("caller resumed")
    del method


def bound_factory():
    return Receiver("factory").replace


def factory_bound_method():
    bound_factory()()
    events.append("caller resumed")


def conditional_bound_method():
    # Ordinary CALL unwraps the temporary method even without a fused lookup.
    condition = True
    (Receiver("conditional").replace if condition else bound_factory())()
    events.append("caller resumed")


def expanded_bound_method():
    # CALL_FUNCTION_EX retains the callable through the invocation. Its
    # receiver therefore survives self rebinding, unlike ordinary CALL.
    bound_factory()(*())
    events.append("caller resumed")


def expression_rebind_tuple():
    original = Value("original")
    pair = (original, (original := Value("replacement")))
    events.append(pair[0].label + ":" + pair[1].label)
    del pair
    events.append("tuple released")


def inspect_pair(first, second):
    events.append(first.label + ":" + second.label)


def expression_rebind_call():
    original = Value("original")
    inspect_pair(original, (original := Value("replacement")))
    events.append("call resumed")


def assignment_expression_result():
    pair = ((value := Value("first")), (value := Value("second")))
    events.append(pair[0].label + ":" + pair[1].label)
    del pair
    events.append("tuple released")


def return_through_finally():
    value = Value("original")
    try:
        return value
    finally:
        value = Value("replacement")
        events.append("finally rebound")


def returned_expression_capture():
    result = return_through_finally()
    events.append("returned " + result.label)
    del result


for case in (
    multi_mapping_failure,
    deferred_star_failure,
    mixed_frame_order,
    surplus_duplicate_failure,
    remaining_keywords_failure,
    class_dispatch,
    builtin_dispatch,
    expanded_binding_failure,
    expanded_builtin_dispatch,
    plain_variadic_dispatch,
    parameter_local_order,
    expanded_parameter_local_order,
    parameter_rebind_order,
    returned_parameter_alias,
    traceback_frame_lifetime,
    owned_attribute_argument,
    temporary_method,
    retained_bound_method,
    factory_bound_method,
    conditional_bound_method,
    expanded_bound_method,
    expression_rebind_tuple,
    expression_rebind_call,
    assignment_expression_result,
    returned_expression_capture,
):
    events.clear()
    result = "ok"
    try:
        case()
    except Exception as error:
        result = type(error).__name__
    print(case.__name__, result, events)
