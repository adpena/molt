"""Purpose: differential coverage for positional-only/kw-only call errors."""


def f(a, /, b, *, c):
    return a, b, c


try:
    f(a=1, b=2, c=3)
except Exception as exc:
    print("posonly", type(exc).__name__, str(exc))

try:
    f(1, 2, 3)
except Exception as exc:
    print("kwonly", type(exc).__name__, str(exc))

print("ok", f(1, b=2, c=3))


def attempt(label, function, args=(), kwargs=None):
    try:
        function(*args, **({} if kwargs is None else kwargs))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "no error")


def zero():
    return None


def one(value):
    return value


def required(first, second, third):
    return first, second, third


def optional(first, second=2):
    return first, second


def keywords(*, first, second, third):
    return first, second, third


def mixed(value, *, first, second=2):
    return value, first, second


def positional(first, second, /):
    return first, second


class Owner:
    def method(self, value):
        return value

    @classmethod
    def build(cls, value):
        return cls, value


def enclosing():
    def nested(value):
        return value

    return nested


def direct_missing():
    return required(1)


def direct_extra():
    return one(1, 2)


# Deliberately unrelated names avoid the versioned typo suggestions in 3.13+.
for label, function, args, kwargs in (
    ("unexpected", one, (1,), {"alien_keyword_xyz": 2}),
    ("duplicate", one, (1,), {"value": 2}),
    ("missing-one", required, (1, 2), {}),
    ("missing-two", required, (1,), {}),
    ("missing-three", required, (), {}),
    ("kwmissing-one", keywords, (), {"first": 1, "second": 2}),
    ("kwmissing-two", keywords, (), {"first": 1}),
    ("kwmissing-three", keywords, (), {}),
    ("extra-zero", zero, (1,), {}),
    ("extra-one", one, (1, 2), {}),
    ("extra-range", optional, (1, 2, 3), {}),
    ("extra-kwonly-one", mixed, (1, 2), {"first": 3}),
    ("extra-kwonly-two", mixed, (1, 2), {"first": 3, "second": 4}),
    ("positional-conflicts", positional, (), {"second": 2, "first": 1}),
    ("keyword-before-arity", one, (1, 2), {"alien_keyword_xyz": 3}),
    ("positional-before-kwonly", mixed, (), {}),
    ("bound-name", Owner().method, (), {}),
    ("class-name", Owner.build, (1,), {"alien_keyword_xyz": 2}),
    ("nested-name", enclosing(), (), {}),
    ("direct-missing", direct_missing, (), {}),
    ("direct-extra", direct_extra, (), {}),
):
    attempt(label, function, args, kwargs)

# The current public qualified name and defaults, not cached declaration text,
# own diagnostics. A no-default function must still enter the Python binder.
one.__name__ = "public_name"
one.__qualname__ = "Renamed.target"
attempt("live-qualified-name", one, (1, 2))
optional.__defaults__ = ()
attempt("live-defaults", optional, (1, 2, 3))
optional.__defaults__ = (1, 2, 3)
attempt("surplus-default-range", optional, (1, 2, 3))
print("surplus-default-binding", optional())


class RenderedKeyword(str):
    def __str__(self):
        return "rendered-keyword"


class RaisingKeyword(str):
    def __str__(self):
        raise RuntimeError("keyword rendering failed")


attempt("keyword-str", one, (1,), {RenderedKeyword("alien_keyword_xyz"): 2})
attempt("duplicate-str", one, (1,), {RenderedKeyword("value"): 2})
attempt("keyword-str-error", one, (1,), {RaisingKeyword("alien_keyword_xyz"): 2})
print("successful-siblings", required(1, 2, 3), optional(1, 2), mixed(1, first=3))
