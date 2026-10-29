"""Purpose: sysconfig install schemes, path names and path expansion.

Prints only what every host and target shares with CPython: the scheme and
path names, the unexpanded templates, and an expansion whose variables the
program names. It does not print the prefixes, which name the running binary.
"""

import sysconfig

BASE_SCHEMES = ("nt", "nt_venv", "posix_home", "posix_prefix", "posix_venv", "venv")
USER_SCHEMES = ("nt_user", "osx_framework_user", "posix_user")

names = sysconfig.get_scheme_names()
print("scheme_names_type", type(names).__name__)
print("scheme_names_sorted", list(names) == sorted(names))
print("base_schemes", [name in names for name in BASE_SCHEMES])
# A host with no home directory (WASI, Emscripten) has no user schemes.
has_user_schemes = all(name in names for name in USER_SCHEMES)
print(
    "user_schemes_all_or_none",
    has_user_schemes or not any(name in names for name in USER_SCHEMES),
)

path_names = sysconfig.get_path_names()
print("path_names", type(path_names).__name__, path_names)

default = sysconfig.get_default_scheme()
print("default_scheme_known", default in names)
for key in ("prefix", "home", "user"):
    try:
        preferred = sysconfig.get_preferred_scheme(key)
    except ValueError as exc:
        # Only the user scheme may be missing, and only without user schemes.
        print("preferred", key, key == "user" and not has_user_schemes, "'user'" in str(exc))
    else:
        print("preferred", key, preferred in names, True)
try:
    sysconfig.get_preferred_scheme("bogus")
except KeyError as exc:
    print("preferred bogus KeyError", exc)

for scheme in ("posix_prefix", "posix_home", "posix_venv", "nt", "nt_venv"):
    templates = sysconfig.get_paths(scheme, expand=False)
    print("templates", scheme, sorted(templates.items()))

for scheme in ("posix_prefix", "posix_home", "posix_venv"):
    expanded = sysconfig.get_paths(scheme)
    print(
        "expanded",
        scheme,
        sorted(expanded),
        all(isinstance(value, str) and value for value in expanded.values()),
    )
print("default_paths_keys", sorted(sysconfig.get_paths()))

named = {
    "base": "/opt/m",
    "platbase": "/opt/p",
    "installed_base": "/opt/i",
    "installed_platbase": "/opt/ip",
    "platlibdir": "lib64",
    "py_version_short": "3.99",
    "abiflags": "",
}
print("named_purelib", sysconfig.get_path("purelib", "posix_prefix", named))
print("named_platstdlib", sysconfig.get_path("platstdlib", "posix_prefix", named))
print("named_home_purelib", sysconfig.get_path("purelib", "posix_home", named))
print("named_scripts", sysconfig.get_path("scripts", "posix_prefix", named))
try:
    sysconfig.get_path("nonexistent", "posix_prefix", named)
except KeyError as exc:
    print("unknown path KeyError", exc)
try:
    sysconfig.get_paths("no_such_scheme")
except KeyError as exc:
    print("unknown scheme KeyError", exc)
