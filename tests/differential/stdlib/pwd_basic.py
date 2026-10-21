# MOLT_META: platforms=posix
# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write,fs.read,os.getuid
"""Independent pwd provider and expanduser public protocol controls."""

import pwd
import os

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

from pathlib import Path

location = Path(".")
print(
    "pwd_path_owner", location.owner() == pwd.getpwuid(os.stat(location).st_uid).pw_name
)
