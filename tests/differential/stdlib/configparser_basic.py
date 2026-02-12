"""Purpose: differential coverage for configparser basic API surface."""

import configparser

cp = configparser.ConfigParser()
cp.read_string("[main]
value=2
")
print(cp.sections())
print(cp.has_section("main"))
print(cp.getint("main", "value"))
print(cp.get("main", "value"))
