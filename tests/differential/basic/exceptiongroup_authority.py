"""Purpose: differential coverage for the one BaseExceptionGroup authority:
constructor admission, PySequence_Tuple identity, default/overridden derive,
split/subgroup partitioning, subset metadata and except* matching."""


def attempt(label, make):
    try:
        result = make()
    except Exception as exc:
        print(label, type(exc).__name__, exc)
    else:
        print(label, "ok", type(result).__name__)


def shape(exc):
    if isinstance(exc, BaseExceptionGroup):
        return [exc.message, [shape(item) for item in exc.exceptions]]
    return repr(exc)


# Constructor admission and CPython's exact messages.
attempt("set", lambda: ExceptionGroup("m", {ValueError(1)}))
attempt("dict", lambda: ExceptionGroup("m", {1: ValueError(1)}))
attempt("empty", lambda: ExceptionGroup("m", []))
attempt("item", lambda: ExceptionGroup("m", [ValueError(1), 1]))
attempt("class item", lambda: ExceptionGroup("m", [ValueError]))
attempt("message int", lambda: ExceptionGroup(1, [ValueError(1)]))
attempt("message none", lambda: ExceptionGroup(None, [ValueError(1)]))
attempt("arity", lambda: ExceptionGroup("m"))


class Spoof:
    @property
    def __class__(self):
        return ValueError


attempt("spoofed item", lambda: ExceptionGroup("m", [Spoof()]))


class StrSub(str):
    pass


eg = ExceptionGroup(StrSub("sub"), [ValueError(1)])
print("str subclass", type(eg.message).__name__, eg.message)

# PySequence_Tuple: exact tuples keep identity; everything else is copied.
items = (ValueError("a"), TypeError("b"))
eg = ExceptionGroup("m", items)
print("tuple identity", eg.exceptions is items, eg.args[1] is items)
listed = [ValueError("a")]
eg = ExceptionGroup("m", listed)
print("list copy", type(eg.exceptions).__name__, eg.exceptions is listed, eg.args[1] is listed)


class TupleSub(tuple):
    pass


subtuple = TupleSub(items)
eg = ExceptionGroup("m", subtuple)
print("tuple subclass", type(eg.exceptions).__name__, eg.exceptions is subtuple)

log = []


class LoggedList(list):
    def __iter__(self):
        log.append("iter")
        return super().__iter__()

    def __len__(self):
        log.append("len")
        return super().__len__()


class LoggedTuple(tuple):
    def __iter__(self):
        log.append("titer")
        return super().__iter__()


class Seq:
    def __init__(self, values):
        self.values = values

    def __len__(self):
        log.append("len")
        return len(self.values)

    def __getitem__(self, index):
        log.append(f"item{index}")
        return self.values[index]


for label, source in [
    ("list subclass", LoggedList([ValueError(1), TypeError(2)])),
    ("tuple subclass iter", LoggedTuple((ValueError(1), TypeError(2)))),
    ("sequence", Seq([ValueError(1), TypeError(2)])),
]:
    log.clear()
    eg = ExceptionGroup("m", source)
    print(label, log, [repr(e) for e in eg.exceptions])

# Class selection and nesting.
print("base narrows", type(BaseExceptionGroup("m", [ValueError(1)])).__name__)
print("base keeps", type(BaseExceptionGroup("m", [KeyboardInterrupt()])).__name__)
attempt("exception group nests", lambda: ExceptionGroup("m", [KeyboardInterrupt()]))


class MyEG(ExceptionGroup):
    pass


class MyBEG(BaseExceptionGroup):
    pass


class MixedGroup(BaseExceptionGroup, Exception):
    pass


attempt("subclass nests", lambda: MyEG("m", [KeyboardInterrupt()]))
attempt("mixed nests", lambda: MixedGroup("m", [KeyboardInterrupt()]))
print("base subclass keeps", type(MyBEG("m", [ValueError(1)])).__name__)

# The default derive is BaseExceptionGroup(self.message, excs).
eg = MyEG("mine", [ValueError(1), TypeError(2)])
match, rest = eg.split(ValueError)
print("default derive", type(match).__name__, type(rest).__name__, match.message, rest.message)
print("derive base", type(eg.derive([KeyboardInterrupt()])).__name__)
print("derive narrow", type(eg.derive([ValueError(3)])).__name__)

# split/subgroup call the visible derive once per non-empty part.
calls = []


class Tracked(ExceptionGroup):
    def derive(self, excs):
        calls.append([type(e).__name__ for e in excs])
        return Tracked(self.message, excs)


eg = Tracked("t", [ValueError(1), TypeError(2), ValueError(3)])
match, rest = eg.split(ValueError)
print("split derive", calls, type(match).__name__, type(rest).__name__)
calls.clear()
outer = Tracked("outer", [Tracked("inner", [ValueError(1), TypeError(2)]), KeyError(3)])
match, rest = outer.split(ValueError)
print("nested derive", calls, shape(match), shape(rest))
calls.clear()
print("subgroup derive", shape(outer.subgroup(ValueError)), calls)


class BadDerive(ExceptionGroup):
    def derive(self, excs):
        return ValueError("not a group")


attempt("bad derive", lambda: BadDerive("b", [ValueError(1), TypeError(2)]).split(ValueError))


class FakeGroup(Exception):
    @property
    def __class__(self):
        return ExceptionGroup


class SpoofDerive(ExceptionGroup):
    def derive(self, excs):
        return FakeGroup("fake")


attempt("spoofed derive", lambda: SpoofDerive("s", [ValueError(1), TypeError(2)]).split(ValueError))

# The matcher runs exactly once per node, in CPython order.
seen = []


def predicate(exc):
    seen.append(type(exc).__name__)
    return isinstance(exc, ValueError)


eg = ExceptionGroup("p", [ValueError(1), ExceptionGroup("q", [TypeError(2), ValueError(3)])])
match, rest = eg.split(predicate)
print("predicate split", seen, shape(match), shape(rest))
seen.clear()
print("predicate subgroup", shape(eg.subgroup(predicate)), seen)
print("full match", eg.subgroup(BaseExceptionGroup) is eg, eg.split(Exception)[0] is eg)
match, rest = eg.split(KeyError)
print("no match", match, rest is eg, shape(rest))

# Subset metadata: traceback, context, cause, suppression and notes.
try:
    try:
        raise RuntimeError("cause")
    except RuntimeError as cause:
        raise ExceptionGroup("meta", [ValueError(1), TypeError(2)]) from cause
except ExceptionGroup as group:
    group.add_note("n1")
    match, rest = group.split(ValueError)
    print("meta traceback", match.__traceback__ is group.__traceback__, rest.__traceback__ is group.__traceback__)
    print("meta chain", match.__cause__ is group.__cause__, match.__context__ is group.__context__)
    print("meta suppress", group.__suppress_context__, match.__suppress_context__, rest.__suppress_context__)
    print("meta notes", match.__notes__, match.__notes__ is group.__notes__, match.__notes__ is rest.__notes__)

eg = ExceptionGroup("plain", [ValueError(1), TypeError(2)])
match, rest = eg.split(ValueError)
print("plain metadata", eg.__suppress_context__, match.__suppress_context__, match.__cause__, match.__context__, match.__traceback__)
eg.__notes__ = 42
match, rest = eg.split(ValueError)
print("odd notes", hasattr(match, "__notes__"))
eg.__notes__ = ("a", "b")
match, rest = eg.split(ValueError)
print("tuple notes", match.__notes__, type(match.__notes__).__name__)


class TracebackDerive(ExceptionGroup):
    def derive(self, excs):
        try:
            raise ExceptionGroup("made", list(excs))
        except ExceptionGroup as made:
            return made


match, rest = TracebackDerive("x", [ValueError(1), TypeError(2)]).split(ValueError)
print("derive traceback kept", match.__traceback__ is not None)

# except* partitions through the same split authority.
calls.clear()
try:
    raise Tracked("star", [ValueError(1), TypeError(2)])
except* ValueError as matched:
    print("star value", type(matched).__name__, shape(matched))
except* TypeError as matched:
    print("star type", type(matched).__name__, shape(matched))
print("star derive", calls)

try:
    raise ValueError("naked")
except* ValueError as wrapped:
    print("naked wrap", type(wrapped).__name__, repr(wrapped.message), shape(wrapped))

# Matcher admission is target-version specific (3.12: functions only).


class CallableMatcher:
    def __call__(self, exc):
        return isinstance(exc, ValueError)


eg = ExceptionGroup("v", [ValueError(1), TypeError(2)])
for label, matcher in [
    ("callable object", CallableMatcher()),
    ("builtin callable", callable),
    ("lambda", lambda exc: True),
    ("empty tuple", ()),
    ("type tuple", (TypeError, KeyError)),
    ("tuple subclass", TupleSub((ValueError,))),
    ("non-exception class", int),
]:
    try:
        result = eg.subgroup(matcher)
    except TypeError as exc:
        print(label, "TypeError", exc)
    else:
        print(label, "ok", None if result is None else shape(result))
