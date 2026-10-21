"""POSIX password database access through the admitted native provider."""

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


def getpwuid(uid, /):
    return _GETPWUID(uid, _PASSWD_TYPE)


def getpwnam(name, /):
    return _GETPWNAM(name, _PASSWD_TYPE)


def getpwall():
    return _GETPWALL(_PASSWD_TYPE)
