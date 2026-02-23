"""Purpose: differential coverage for platform basic."""

import platform

# system() returns a non-empty string
s = platform.system()
print("system_is_str", isinstance(s, str) and len(s) > 0)

# machine() returns a non-empty string
m = platform.machine()
print("machine_is_str", isinstance(m, str) and len(m) > 0)

# python_version() returns a dotted version string
pv = platform.python_version()
parts = pv.split(".")
print("python_version_dotted", len(parts) == 3)

# python_version_tuple() returns a 3-element tuple of strings
pvt = platform.python_version_tuple()
print("python_version_tuple_len", len(pvt) == 3)
print("python_version_tuple_types", all(isinstance(x, str) for x in pvt))

# uname() returns a named tuple with 6 elements (includes processor)
u = platform.uname()
print("uname_len", len(u) == 6)
print("uname_system", isinstance(u.system, str) and len(u.system) > 0)
print("uname_machine", isinstance(u.machine, str) and len(u.machine) > 0)
print("uname_has_processor", hasattr(u, "processor"))

# node() returns a string
n = platform.node()
print("node_is_str", isinstance(n, str))

# release() returns a string
r = platform.release()
print("release_is_str", isinstance(r, str))

# processor() returns a string
p = platform.processor()
print("processor_is_str", isinstance(p, str))
