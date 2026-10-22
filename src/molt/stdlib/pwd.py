"""This module provides access to the Unix password database.
It is available on all Unix versions.

Password database entries are reported as 7-tuples containing the following
items from the password database (see `<pwd.h>'), in order:
pw_name, pw_passwd, pw_uid, pw_gid, pw_gecos, pw_dir, pw_shell.
The uid and gid items are integers, all others are strings. An
exception is raised if the entry asked for cannot be found."""

from _intrinsics import require_intrinsic as _require_intrinsic
from sys import _struct_sequence_type

_require_intrinsic("molt_pwd_module_init")()
_GETPWUID = _require_intrinsic("molt_pwd_getpwuid")
_GETPWNAM = _require_intrinsic("molt_pwd_getpwnam")
_GETPWALL = _require_intrinsic("molt_pwd_getpwall")

struct_passwd = _struct_sequence_type(
    "struct_passwd",
    ("pw_name", "pw_passwd", "pw_uid", "pw_gid", "pw_gecos", "pw_dir", "pw_shell"),
    module="pwd",
    static_origin=False,
    docs=(
        "user name",
        "password",
        "user id",
        "group id",
        "real name",
        "home directory",
        "shell program",
    ),
    doc=(
        "pwd.struct_passwd: Results from getpw*() routines.\n\n"
        "This object may be accessed either as a tuple of\n"
        "  (pw_name,pw_passwd,pw_uid,pw_gid,pw_gecos,pw_dir,pw_shell)\n"
        "or via the object attributes as named in the above tuple."
    ),
)
_PASSWD_TYPE = struct_passwd

__all__ = ["getpwuid", "getpwnam", "getpwall", "struct_passwd"]


def getpwuid(uidobj, /):
    "Return the password database entry for the given numeric user ID.\n\nSee `help(pwd)` for more on password database entries."
    return _GETPWUID(uidobj, _PASSWD_TYPE)


def getpwnam(name, /):
    "Return the password database entry for the given user name.\n\nSee `help(pwd)` for more on password database entries."
    return _GETPWNAM(name, _PASSWD_TYPE)


def getpwall():
    "Return a list of all available password database entries, in arbitrary order.\n\nSee help(pwd) for more on password database entries."
    return _GETPWALL(_PASSWD_TYPE)


getpwuid.__text_signature__ = "($module, uidobj, /)"
getpwnam.__text_signature__ = "($module, name, /)"
getpwall.__text_signature__ = "($module, /)"
_FUNCTION_SET_FIXED_BUILTIN = _require_intrinsic("molt_function_set_fixed_builtin")
_FUNCTION_SET_FIXED_BUILTIN(getpwuid, 1)
_FUNCTION_SET_FIXED_BUILTIN(getpwnam, 1)
_FUNCTION_SET_FIXED_BUILTIN(getpwall, 0)
