"""Purpose: differential coverage for pathlib basic surface (string/parts/parents)."""

import sys

# Installed CPython source does not change when public version metadata is
# reassigned. Exercise the import boundary before restoring that metadata.
_runtime_version = sys.version_info
sys.version_info = (3, 99, 0, "final", 0)
try:
    from pathlib import Path, PurePosixPath, PureWindowsPath
finally:
    sys.version_info = _runtime_version


p = Path("a") / "b" / "c.txt"
print(str(p))
print(p.parts)
print(p.name)
print(p.suffix)
print(p.stem)
print(str(p.parent))
print([str(x) for x in p.parents])

q = Path("/a/b/c.txt")
print(str(q.relative_to("/a")))
print(q.is_relative_to("/a"))
print(q.is_relative_to("/x"))

# Lexical canonicalization keeps '..' and shares the value used by comparison,
# hashing, parents and all derived constructors.
for raw in ("", ".", "./a//b/./", "a/../b", "//a///b/", "///a/b", "a/pkg.tar.gz"):
    value = PurePosixPath(raw)
    print("canonical", raw, str(value), value.parts, value.name, value.suffix, value.suffixes, value.stem)
    print("canonical-parents", [str(parent) for parent in value.parents])
print("dotdot-kept", str(Path("a/../b").absolute()).endswith("a/../b"))

factory_events = []


class Tagged(PurePosixPath):
    def __init__(self, *segments, token="state"):
        super().__init__(*segments)
        self.token = token

    def with_segments(self, *segments):
        factory_events.append((len(segments), bool(segments) and segments[0] is self))
        return type(self)(*segments, token=self.token)


value = Tagged("a/./b/file.tar.gz")
for label, operation in (
    ("join", lambda: value.joinpath("..", "next")),
    ("divide", lambda: value / "next"),
    ("reverse-divide", lambda: "prefix" / value),
    ("parent", lambda: value.parent),
    ("name", lambda: value.with_name("other.txt")),
    ("suffix", lambda: value.with_suffix(".xz")),
    ("stem", lambda: value.with_stem("other")),
    ("relative", lambda: value.relative_to("a")),
    ("walk-up", lambda: value.relative_to("a/c", walk_up=True)),
):
    factory_events.clear()
    result = operation()
    print("factory", label, str(result), type(result) is Tagged, result.token, factory_events)
factory_events.clear()
parents = value.parents
print("parents-lazy", factory_events)
print("parents-first", str(parents[0]), factory_events)
print("parents-last", str(parents[-1]), type(parents[-1]) is Tagged)
root = Tagged("/")
print("root-parent-identity", root.parent is root)


class OverrideJoin(Tagged):
    def joinpath(self, *parts):
        raise LookupError("join override")


try:
    print("divide-dispatch", str(OverrideJoin("a") / "b"))
except LookupError as error:
    print("divide-dispatch", type(error).__name__)


class OverrideAbsolute(Path):
    def absolute(self):
        return "absolute override"


print("cwd-factory-dispatch", isinstance(OverrideAbsolute.cwd(), str))

# Constructor callbacks occur when arguments are accepted; parsing is lazy.
constructor_events = []


class Source:
    def __fspath__(self):
        constructor_events.append("fspath")
        return "./a//b"


value = Tagged(Source(), "..", "c")
print("constructor-events", constructor_events)
print("constructor-value", str(value), constructor_events)
for invalid in (None, b"path", 1):
    try:
        Tagged(invalid)
    except TypeError as error:
        print("constructor-refusal", type(invalid).__name__, type(error).__name__)

# Replacement validation and callback order are versioned public behavior.
for raw in ("/", "file.txt", "file.", "..hidden", "a/pkg.tar.gz"):
    value = Tagged(raw)
    for operation, argument in (("with_name", "."), ("with_suffix", "."), ("with_stem", "")):
        factory_events.clear()
        try:
            result = str(getattr(value, operation)(argument))
        except (TypeError, ValueError) as error:
            result = type(error).__name__
        print("replacement", raw, operation, result, factory_events)


# Reassigning public metadata after import must not switch only the method-time
# half of the facade while leaving class-time read_text/rglob definitions fixed.
def selected_version_behavior():
    value = PurePosixPath("file.")
    outcomes = [value.suffix, value.suffixes, value.stem]
    for operation, argument in (("with_suffix", "."), ("with_stem", "")):
        try:
            outcomes.append(str(getattr(PurePosixPath("file.txt"), operation)(argument)))
        except ValueError as error:
            outcomes.append(type(error).__name__)
    try:
        outcomes.append(str(OverrideJoin("a") / "b"))
    except LookupError as error:
        outcomes.append(type(error).__name__)
    outcomes.append(isinstance(OverrideAbsolute.cwd(), str))
    outcomes.append(PureWindowsPath("😀:leaf").drive)
    return outcomes


baseline = selected_version_behavior()
print("selected-before-public-reassignment", baseline)
try:
    for public_version in ((3, 12, 0, "final", 0), (3, 14, 0, "final", 0)):
        sys.version_info = public_version
        actual = selected_version_behavior()
        assert actual == baseline
        print("selected-after-public-reassignment", actual == baseline)
finally:
    sys.version_info = _runtime_version
