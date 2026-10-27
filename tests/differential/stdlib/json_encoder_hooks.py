"""Purpose: differential coverage for json encoder/decoder hooks and options."""

import json


def default(obj):
    return {"__obj__": str(obj)}


payload = {"a": 1, "b": [2, 3], "c": "\u2603"}
text = json.dumps(
    payload,
    default=default,
    ensure_ascii=False,
    sort_keys=True,
    separators=(",", ":"),
    indent=2,
)
print("text", text)

restored = json.loads(
    text,
    object_hook=lambda d: {"hook": True, **d},
    parse_int=lambda s: int(s) + 1,
)
print("restored", restored["a"], restored["hook"])


def expect_callback_exception(label, marker, invoke, calls, expected):
    try:
        invoke()
    except BaseException as error:
        assert error is marker, (label, type(error).__name__)
    else:
        raise AssertionError((label, "callback exception was swallowed"))
    assert calls == expected, (label, calls)
    print("callback-exception", label, type(marker).__name__, calls)


def callback_exception_controls():
    # The second token and outer object must remain unvisited after a hook
    # fails. The original exception is a user value, not a decoder failure to
    # translate or an instruction to retry through another implementation.
    for error_type in (ImportError, ValueError, TypeError):
        for name, payload, token in (
            ("parse_int", "[1,2]", "1"),
            ("parse_float", "[1.25,2.5]", "1.25"),
            ("parse_constant", "[NaN,Infinity,-Infinity]", "NaN"),
        ):
            calls = []
            marker = error_type(name)

            def parse(value):
                calls.append(value)
                raise marker

            expect_callback_exception(
                name,
                marker,
                lambda: json.loads(payload, **{name: parse}),
                calls,
                [token],
            )

        calls = []
        marker = error_type("object_hook")

        def parse_number(value):
            calls.append(("number", value))
            return int(value)

        def object_hook(value):
            calls.append(("object", tuple(value)))
            raise marker

        expect_callback_exception(
            "object_hook",
            marker,
            lambda: json.loads(
                '{"outer":{"inner":1},"tail":2}',
                parse_int=parse_number,
                object_hook=object_hook,
            ),
            calls,
            [("number", "1"), ("object", ("inner",))],
        )

        calls = []
        marker = error_type("object_pairs_hook")

        def object_pairs_hook(value):
            calls.append(("pairs", tuple(value)))
            raise marker

        def unused_object_hook(value):
            calls.append(("unexpected-object-hook",))
            return value

        expect_callback_exception(
            "object_pairs_hook",
            marker,
            lambda: json.loads(
                '{"outer":{"x":1,"x":2},"tail":3}',
                parse_int=parse_number,
                object_pairs_hook=object_pairs_hook,
                object_hook=unused_object_hook,
            ),
            calls,
            [("number", "1"), ("number", "2"), ("pairs", (("x", 1), ("x", 2)))],
        )

        # Exercise an injected callable object as well as Python functions.
        # The encoder must not visit the later object or call default twice.
        for indent in (None, 0):
            calls = []
            marker = error_type("default")
            first, second = object(), object()

            class Default:
                def __call__(self, value):
                    calls.append("first" if value is first else "unexpected-later")
                    raise marker

            expect_callback_exception(
                "default-compact" if indent is None else "default-indented",
                marker,
                lambda: json.dumps([first, second], default=Default(), indent=indent),
                calls,
                ["first"],
            )


callback_exception_controls()


def original_item_sort_controls():
    import io
    import _json

    def render(method, value, indent, skipkeys):
        options = {"sort_keys": True, "skipkeys": skipkeys, "indent": indent}
        if method == "dumps":
            return json.dumps(value, **options)
        if method == "encode":
            return json.JSONEncoder(**options).encode(value)
        if method == "iterencode":
            return "".join(json.JSONEncoder(**options).iterencode(value))
        stream = io.StringIO()
        json.dump(value, stream, **options)
        return stream.getvalue()

    for indent in (None, 0, 2):
        for method in ("dumps", "encode", "iterencode", "dump"):
            for label, value in (
                ("numeric", {10: "ten", 2: "two", -1: "minus"}),
                ("floating", {10.5: "ten", 2.5: "two", -1.5: "minus"}),
                ("mixed", {1: "one", (1, 2): "tuple"}),
                ("numeric-string", {1: "integer", "1": "string"}),
                ("nested", {"root": {10: "ten", 2: "two"}}),
                ("all-skipped", {(1,): "one", (2,): "two"}),
                ("empty", {}),
            ):
                try:
                    result = render(method, value, indent, True)
                except TypeError as error:
                    print("item-sort", method, indent, label, type(error).__name__)
                else:
                    print("item-sort", method, indent, label, repr(result))

    # The accelerator and facade share original-item ordering. This factory
    # receives its normal compact-mode inputs, independently of JSONEncoder.
    compact = _json.make_encoder(
        {}, None, _json.encode_basestring_ascii, None, ": ", ", ", True, True, True
    )
    print("item-sort", "accelerator", "".join(compact({10: "ten", 2: "two"}, 0)))
    try:
        compact({1: "one", (1, 2): "tuple"}, 0)
    except TypeError as error:
        print("item-sort", "accelerator-mixed", type(error).__name__)
    else:
        raise AssertionError("accelerator filtered original keys before sorting")

    # Unsupported keys must still be compared before skipkeys applies. The
    # exact comparison algorithm/count is deliberately not the oracle.
    for skipkeys in (False, True):
        calls = []
        marker = LookupError("key comparison")

        class Key:
            def __init__(self, number):
                self.number = number

            def __hash__(self):
                return self.number

            def __eq__(self, other):
                return self is other

            def __lt__(self, other):
                calls.append("comparison")
                raise marker

        value = {Key(1): "one", Key(2): "two"}
        try:
            json.dumps(value, sort_keys=True, skipkeys=skipkeys)
        except LookupError as error:
            assert error is marker
            assert calls
            print("item-sort-callback", skipkeys, type(error).__name__)
        else:
            raise AssertionError("key comparison was skipped")
        calls.clear()
        assert json.dumps(value, skipkeys=True) == "{}"
        assert not calls

    # CPython sorts item tuples, so equal key comparisons can reach values.
    # Distinct hashes preserve both original entries even for these unusual
    # user keys; a key-only comparator would miss the value exception.
    calls = []
    marker = ArithmeticError("item value comparison")

    class EqualKey:
        def __init__(self, number):
            self.number = number

        def __hash__(self):
            return self.number

        def __eq__(self, other):
            return True

    class Value:
        def __lt__(self, other):
            calls.append("value comparison")
            raise marker

    value = {EqualKey(1): Value(), EqualKey(2): Value()}
    assert len(value) == 2
    try:
        json.dumps(value, sort_keys=True, skipkeys=True)
    except ArithmeticError as error:
        assert error is marker
        assert calls
        print("item-sort-value-callback", type(error).__name__)
    else:
        raise AssertionError("original item tuple comparison was skipped")

    # Sorting errors precede even key conversion's non-finite-number policy.
    try:
        json.dumps(
            {float("nan"): "nan", "x": "string"}, sort_keys=True, allow_nan=False
        )
    except TypeError as error:
        print("item-sort-before-float-policy", type(error).__name__)
    else:
        raise AssertionError("key conversion preceded original item sorting")

    # A later key failure must not run ahead of an earlier value callback.
    for sort_keys in (False, True):
        calls = []
        marker = LookupError("earlier value")

        def early_default(value):
            calls.append("default")
            raise marker

        try:
            json.dumps(
                {1.0: object(), float("inf"): 0},
                sort_keys=sort_keys,
                allow_nan=False,
                default=early_default,
            )
        except LookupError as error:
            assert error is marker
            assert calls == ["default"]
            print("item-value-before-later-key-error", sort_keys, type(error).__name__)
        else:
            raise AssertionError("later key conversion preceded first value")

    deaths = []
    mutations = []

    class HeldValue:
        def __init__(self, name):
            self.name = name

        def __del__(self):
            deaths.append(self.name)

    class ClearingKey:
        def __init__(self, number):
            self.number = number

        def __hash__(self):
            return self.number

        def __eq__(self, other):
            return self is other

        def __lt__(self, other):
            value.clear()
            assert not deaths, "original item values were released during comparison"
            mutations.append("clear")
            return self.number < other.number

    value = {ClearingKey(2): HeldValue("two"), ClearingKey(1): HeldValue("one")}
    assert json.dumps(value, sort_keys=True, skipkeys=True) == "{}"
    assert not value
    assert mutations
    print("item-sort-snapshot-survives-source-clear", True)


original_item_sort_controls()
