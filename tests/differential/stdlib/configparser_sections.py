"""Purpose: differential coverage for configparser section and option management."""

import configparser

cp = configparser.ConfigParser()
cp.read_string("""
[main]
host = localhost
port = 8080
debug = true
ratio = 3.14

[database]
name = mydb
user = admin
""")

# sections
print(cp.sections())

# has_section
print(cp.has_section("main"))
print(cp.has_section("nonexistent"))

# has_option
print(cp.has_option("main", "host"))
print(cp.has_option("main", "missing_key"))

# get
print(cp.get("main", "host"))
print(cp.get("database", "name"))

# getint
print(cp.getint("main", "port"))
print(type(cp.getint("main", "port")).__name__)

# getfloat
print(cp.getfloat("main", "ratio"))
print(type(cp.getfloat("main", "ratio")).__name__)

# getboolean
print(cp.getboolean("main", "debug"))
print(type(cp.getboolean("main", "debug")).__name__)

# options
opts = cp.options("main")
print(sorted(opts))

# items
items = cp.items("database")
print(sorted(items))

# set
cp.set("main", "new_key", "new_value")
print(cp.get("main", "new_key"))

# add_section / remove_section
cp.add_section("extra")
print(cp.has_section("extra"))
result = cp.remove_section("extra")
print(result)
print(cp.has_section("extra"))

# remove_option
cp.set("main", "temp", "temporary")
print(cp.has_option("main", "temp"))
result = cp.remove_option("main", "temp")
print(result)
print(cp.has_option("main", "temp"))

# fallback
print(cp.get("main", "missing", fallback="default_val"))
print(cp.getint("main", "missing", fallback=42))
print(cp.getfloat("main", "missing", fallback=2.5))
print(cp.getboolean("main", "missing", fallback=False))
