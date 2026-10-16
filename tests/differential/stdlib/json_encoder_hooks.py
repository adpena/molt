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
