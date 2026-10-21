# MOLT_META: platforms=posix
# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write,fs.read,os.getuid
"""Independent pwd provider and expanduser public protocol controls."""

import pwd
import os
import sys
from pathlib import Path

print(
    "pwd_module_doc",
    pwd.__doc__
    == "This module provides access to the Unix password database.\n"
    "It is available on all Unix versions.\n\n"
    "Password database entries are reported as 7-tuples containing the following\n"
    "items from the password database (see `<pwd.h>'), in order:\n"
    "pw_name, pw_passwd, pw_uid, pw_gid, pw_gecos, pw_dir, pw_shell.\n"
    "The uid and gid items are integers, all others are strings. An\n"
    "exception is raised if the entry asked for cannot be found.",
)

entry = pwd.getpwuid(os.getuid())
print(
    "pwd_identity",
    pwd.getpwnam(entry.pw_name) == entry,
    type(entry) is pwd.struct_passwd,
    isinstance(entry, tuple),
    len(entry),
    entry.n_fields,
    entry.n_sequence_fields,
    entry.n_unnamed_fields,
)
print(
    "pwd_fields",
    tuple(entry)
    == (
        entry.pw_name,
        entry.pw_passwd,
        entry.pw_uid,
        entry.pw_gid,
        entry.pw_gecos,
        entry.pw_dir,
        entry.pw_shell,
    ),
)
print(
    "pwd_enumeration",
    isinstance(pwd.getpwall(), list),
    all(type(item) is pwd.struct_passwd and len(item) == 7 for item in pwd.getpwall()),
)


class UserIndex:
    def __init__(self, value):
        self.value = value
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return self.value


uid = UserIndex(entry.pw_uid)
print("pwd_index", pwd.getpwuid(uid) == entry, uid.calls)
error = RuntimeError("uid callback")


class BrokenIndex:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        raise error


broken = BrokenIndex()
try:
    pwd.getpwuid(broken)
except TypeError as observed:
    print(
        "pwd_callback_replacement",
        type(observed).__name__,
        broken.calls,
        str(observed) == "uid should be integer, not BrokenIndex",
    )
for function, value in (
    (pwd.getpwnam, 1),
    (pwd.getpwnam, "bad\x00name"),
    (pwd.getpwuid, object()),
    (pwd.getpwuid, -1),
    (pwd.getpwuid, 1 << 100),
):
    try:
        function(value)
    except (TypeError, ValueError, KeyError, OverflowError) as observed:
        print("pwd_invalid", type(observed).__name__)
print(
    "pwd_constructor",
    pwd.struct_passwd(range(7)) == tuple(range(7)),
    pwd.struct_passwd.__bases__ == (tuple,),
)
try:

    class DerivedPasswd(pwd.struct_passwd):
        pass
except TypeError:
    print("pwd_subclass TypeError")
provider = pwd.getpwnam
home = os.environ.get("HOME")
os.environ["HOME"] = "/pwd-home-control"


def forbidden_lookup(name):
    raise error


pwd.getpwnam = forbidden_lookup
try:
    print("expanduser_HOME", os.path.expanduser("~/leaf") == "/pwd-home-control/leaf")
    try:
        os.path.expanduser("~" + entry.pw_name)
    except RuntimeError as observed:
        print("expanduser_named_lookup", observed is error)
finally:
    pwd.getpwnam = provider
    if home is None:
        del os.environ["HOME"]
    else:
        os.environ["HOME"] = home


for function, keyword, value in (
    (pwd.getpwuid, "uid", entry.pw_uid),
    (pwd.getpwnam, "name", entry.pw_name),
):
    try:
        function(**{keyword: value})
    except TypeError:
        print("pwd_positional_only TypeError")

location = Path(".")
print(
    "pwd_path_owner", location.owner() == pwd.getpwuid(os.stat(location).st_uid).pw_name
)


# CPython's two argument converters have distinct native-name precision and
# getpwnam's clinic converter spells the singleton None specially.
LongArgumentName = type("X" * 220, (), {})
qualified_type_diagnostic = (3, 13) <= sys.version_info[:2] < (3, 15)
incomplete_utf8_suffix = "\ufffd" if sys.version_info[:2] < (3, 14) else ""
for function, argument, expected in (
    (pwd.getpwuid, LongArgumentName(), "uid should be integer, not " + "X" * 200),
    (
        pwd.getpwnam,
        LongArgumentName(),
        "getpwnam() argument must be str, not "
        + "X" * (220 if qualified_type_diagnostic else 50),
    ),
    (
        pwd.getpwnam,
        None,
        "getpwnam() argument must be str, not "
        + ("NoneType" if qualified_type_diagnostic else "None"),
    ),
):
    try:
        function(argument)
    except TypeError as observed:
        print("pwd_argument_diagnostic", str(observed) == expected)

for function, width in ((pwd.getpwuid, 200), (pwd.getpwnam, 50)):
    argument = type("X" * (width - 1) + "\u03a9", (), {})()
    try:
        function(argument)
    except TypeError as observed:
        suffix = (
            "\u03a9"
            if function is pwd.getpwnam and qualified_type_diagnostic
            else incomplete_utf8_suffix
        )
        print(
            "pwd_argument_utf8_precision",
            str(observed).endswith("X" * (width - 1) + suffix),
        )


class MetadataText(str):
    def __str__(self):
        raise RuntimeError("metadata must not call str")

    def __repr__(self):
        raise RuntimeError("metadata must not call repr")


class QualifiedArgument:
    pass


QualifiedArgument.__module__ = MetadataText("m.\udcff\x00")
QualifiedArgument.__qualname__ = MetadataText("Q.\udcff\x00")
try:
    pwd.getpwnam(QualifiedArgument())
except TypeError as observed:
    print(
        "pwd_argument_qualified_unicode",
        str(observed)
        == "getpwnam() argument must be str, not "
        + (
            "m.\udcff\x00.Q.\udcff\x00"
            if qualified_type_diagnostic
            else "QualifiedArgument"
        ),
    )


class MissingName(str):
    def __repr__(self):
        return "\udcff\x00\u03a9"


missing = MissingName("molt_no_such_user_486ae10d")
try:
    pwd.getpwnam(missing)
except KeyError as observed:
    print(
        "pwd_missing_name_unicode",
        observed.args == ("getpwnam(): name not found: \udcff\x00\u03a9",),
    )


class FieldRepr:
    def __repr__(self):
        return "\udcff\x00\u03a9"


record = pwd.struct_passwd((FieldRepr(), "x", 1, 2, "g", "d", "s"))
print(
    "pwd_record_repr_unicode",
    repr(record)
    == "pwd.struct_passwd(pw_name=\udcff\x00\u03a9, pw_passwd='x', pw_uid=1, "
    "pw_gid=2, pw_gecos='g', pw_dir='d', pw_shell='s')",
)
repr_error = RuntimeError("pwd repr callback")


class FailingRepr(str):
    def __repr__(self):
        raise repr_error


for render, value in (
    (pwd.getpwnam, FailingRepr(missing)),
    (repr, pwd.struct_passwd((FailingRepr("x"),) * 7)),
):
    try:
        render(value)
    except RuntimeError as observed:
        print("pwd_repr_callback_error", observed is repr_error)

cycle = []
cyclic_record = pwd.struct_passwd((cycle, "x", 1, 2, "g", "d", "s"))
cycle.append(cyclic_record)
print(
    "pwd_record_list_cycle",
    repr(cyclic_record)
    == "pwd.struct_passwd(pw_name=[pwd.struct_passwd(pw_name=[...], pw_passwd='x', "
    "pw_uid=1, pw_gid=2, pw_gecos='g', pw_dir='d', pw_shell='s')], pw_passwd='x', "
    "pw_uid=1, pw_gid=2, pw_gecos='g', pw_dir='d', pw_shell='s')",
)
cycle.clear()


class ReentrantRepr:
    def __repr__(self):
        return repr(reentrant_record)


reentrant_record = pwd.struct_passwd((ReentrantRepr(),) * 7)
try:
    repr(reentrant_record)
except RecursionError:
    print("pwd_record_repr_recursion", True)
print(
    "pwd_record_repr_unwind",
    repr(pwd.struct_passwd((0,) * 7))
    == "pwd.struct_passwd(pw_name=0, pw_passwd=0, pw_uid=0, pw_gid=0, "
    "pw_gecos=0, pw_dir=0, pw_shell=0)",
)

# __replace__ is part of the shared record protocol from Python 3.13 onward.
# A str-subclass keyword exercises its Python-produced error text directly.
if hasattr(record, "__replace__"):
    try:
        record.__replace__(**{MissingName("unexpected"): 42})
    except TypeError as observed:
        print(
            "pwd_replace_error_unicode",
            observed.args == ("Got unexpected field name(s): [\udcff\x00\u03a9]",),
        )

for function, name, signature, doc in (
    (
        pwd.getpwuid,
        "getpwuid",
        "($module, uidobj, /)",
        "Return the password database entry for the given numeric user ID.\n\n"
        "See `help(pwd)` for more on password database entries.",
    ),
    (
        pwd.getpwnam,
        "getpwnam",
        "($module, name, /)",
        "Return the password database entry for the given user name.\n\n"
        "See `help(pwd)` for more on password database entries.",
    ),
    (
        pwd.getpwall,
        "getpwall",
        "($module, /)",
        "Return a list of all available password database entries, in arbitrary order.\n\n"
        "See help(pwd) for more on password database entries.",
    ),
):
    print(
        "pwd_callable_metadata",
        type(function).__name__ == "builtin_function_or_method",
        function.__self__ is pwd,
        function.__module__ == "pwd",
        function.__name__ == name,
        function.__qualname__ == name,
        function.__text_signature__ == signature,
        function.__doc__ == doc,
        not hasattr(function, "__dict__"),
    )

for function, args, kwargs, expected in (
    (pwd.getpwuid, (), {}, "pwd.getpwuid() takes exactly one argument (0 given)"),
    (pwd.getpwuid, (1, 2), {}, "pwd.getpwuid() takes exactly one argument (2 given)"),
    (pwd.getpwuid, (), {"uidobj": 1}, "pwd.getpwuid() takes no keyword arguments"),
    (pwd.getpwnam, (), {}, "pwd.getpwnam() takes exactly one argument (0 given)"),
    (
        pwd.getpwnam,
        ("a", "b"),
        {},
        "pwd.getpwnam() takes exactly one argument (2 given)",
    ),
    (pwd.getpwnam, (), {"name": "x"}, "pwd.getpwnam() takes no keyword arguments"),
    (pwd.getpwall, (1,), {}, "pwd.getpwall() takes no arguments (1 given)"),
    (pwd.getpwall, (), {"x": 1}, "pwd.getpwall() takes no keyword arguments"),
):
    try:
        function(*args, **kwargs)
    except TypeError as observed:
        print("pwd_callable_binding", str(observed) == expected)


# Native error labels use the module object's comparison and string protocols.
class ModuleComparisonError(Exception):
    pass


module_error = ModuleComparisonError("module comparison")


class RejectModuleComparison(str):
    def __ne__(self, other):
        raise module_error


class RenderModuleAlias(str):
    def __str__(self):
        return "alias"


original_module = pwd.getpwuid.__module__
try:
    pwd.getpwuid.__module__ = RejectModuleComparison("pwd")
    try:
        pwd.getpwuid()
    except ModuleComparisonError as observed:
        print("pwd_callable_module_callback", observed is module_error)
    pwd.getpwuid.__module__ = RenderModuleAlias("pwd")
    try:
        pwd.getpwuid()
    except TypeError as observed:
        print(
            "pwd_callable_module_alias",
            str(observed) == "alias.getpwuid() takes exactly one argument (0 given)",
        )
finally:
    pwd.getpwuid.__module__ = original_module

# The native C name is mutable on this heap type. Error precision counts bytes,
# while successful repr keeps the whole name. Run after the creation-name probes.
original_name = pwd.struct_passwd.__name__
long_name = "X" * 499 + "\u03a9"
try:
    pwd.struct_passwd.__name__ = long_name
    print("pwd_repr_full_native_name", repr(record).startswith(long_name + "("))
    for args, suffix in (
        (((),), " takes a 7-sequence (0-sequence given)"),
        (((0,) * 7, None), " takes a dict as second arg, if any"),
    ):
        try:
            pwd.struct_passwd(*args)
        except TypeError as observed:
            print(
                "pwd_constructor_utf8_precision",
                str(observed) == "X" * 499 + incomplete_utf8_suffix + "()" + suffix,
            )
    if hasattr(record, "__replace__"):
        try:
            pwd.struct_passwd((0,) * 7, {"extra": 1})
        except TypeError as observed:
            print(
                "pwd_constructor_keyword_precision",
                str(observed)
                == "X" * 499
                + incomplete_utf8_suffix
                + "() got duplicate or unexpected field name(s)",
            )
finally:
    pwd.struct_passwd.__name__ = original_name
