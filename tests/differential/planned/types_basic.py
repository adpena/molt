"""Purpose: differential coverage for types basic."""

import types


ns = types.SimpleNamespace(a=1)
print(ns.a)

mp = types.MappingProxyType({"a": 1})
print(mp["a"])
