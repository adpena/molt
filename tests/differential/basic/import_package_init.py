# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
import pkg_basic
from pkg_basic import submod

print("pkg", pkg_basic.VALUE, submod.SUBVALUE)
print("pkg_value", pkg_basic.pkg_value())
