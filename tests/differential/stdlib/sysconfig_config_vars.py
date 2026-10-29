# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-273 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: sysconfig configuration variables as CPython's test.support reads them.

Prints types, key presence and the values every interpreter derives the same
way (the target Python version). Build values that name the interpreter's own
build (compiler flags, install prefixes) legitimately differ and stay unprinted.
"""

import io
import warnings

import sysconfig

COMPUTED = (
    "abiflags",
    "base",
    "exec_prefix",
    "installed_base",
    "installed_platbase",
    "platbase",
    "platlibdir",
    "prefix",
    "projectbase",
    "py_version",
    "py_version_nodot",
    "py_version_nodot_plat",
    "py_version_short",
    "srcdir",
)

config = sysconfig.get_config_vars()
print("config_type", type(config).__name__)
print("config_cached", sysconfig.get_config_vars() is config)
print("config_keys_str", all(isinstance(key, str) for key in config))
print("computed_present", [name in config for name in COMPUTED])
print("computed_types", [type(config[name]).__name__ for name in COMPUTED])
print("py_version_short", config["py_version_short"])
print("py_version_nodot", config["py_version_nodot"])
print("py_version_prefix", config["py_version"].startswith(config["py_version_short"]))
print("python_version", sysconfig.get_python_version())

print("unknown_var", sysconfig.get_config_var("MOLT_NO_SUCH_CONFIG_VAR"))
print(
    "vars_list",
    sysconfig.get_config_vars("py_version_short", "MOLT_NO_SUCH_CONFIG_VAR"),
)
print("var_matches_dict", sysconfig.get_config_var("prefix") == config["prefix"])

# test.support.check_sanitizer() reads these two at import time.
cflags = sysconfig.get_config_var("CFLAGS") or ""
config_args = sysconfig.get_config_var("CONFIG_ARGS") or ""
print("cflags_type", type(cflags).__name__, type(config_args).__name__)
print(
    "sanitizers",
    [
        "-fsanitize=memory" in cflags or "--with-memory-sanitizer" in config_args,
        "-fsanitize=address" in cflags or "--with-address-sanitizer" in config_args,
        "-fsanitize=thread" in cflags or "--with-thread-sanitizer" in config_args,
    ],
)

print("is_python_build", sysconfig.is_python_build())
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    try:
        sysconfig.is_python_build(True)
    except TypeError:
        # Python 3.15 removed the deprecated check_home argument.
        print("check_home_removed")
print("check_home_warnings", [type(w.message).__name__ for w in caught])

config_h = sysconfig.get_config_h_filename()
print("config_h", type(config_h).__name__, config_h.endswith("pyconfig.h"))
makefile = sysconfig.get_makefile_filename()
print("makefile", type(makefile).__name__, makefile.endswith("Makefile"))

parsed = sysconfig.parse_config_h(
    io.StringIO('#define HAVE_X 1\n/* #undef HAVE_Y */\n#define NAME "v"\n')
)
print("parse_config_h", sorted(parsed.items()))
merged = sysconfig.parse_config_h(io.StringIO("#define HAVE_Z 7\n"), {"KEEP": 1})
print("parse_config_h_into", sorted(merged.items()))
print(
    "expand_makefile_vars",
    sysconfig.expand_makefile_vars("$(A)-${B}-c", {"A": "x", "B": "$(A)y"}),
)

# CPython 3.13 added _sysconfig; on 3.12 its import fails.
try:
    import _sysconfig
except ModuleNotFoundError as exc:
    print("_sysconfig", "ModuleNotFoundError", exc.name)
else:
    config_vars = _sysconfig.config_vars()
    print("_sysconfig", type(config_vars).__name__)
    print(
        "_sysconfig_ints",
        sorted(
            name
            for name, value in config_vars.items()
            if name.startswith("Py_") and isinstance(value, int)
        ),
    )
