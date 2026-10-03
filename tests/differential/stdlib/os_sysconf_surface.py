"""Target capability controls the sysconf API and its shared name table."""
import os

available = hasattr(os, "sysconf")
assert hasattr(os, "sysconf_names") == available
assert ("sysconf" in os.__all__) == available
assert ("sysconf_names" in os.__all__) == available
if available:
    assert isinstance(os.sysconf_names, dict)
    key = os.sysconf_names["SC_IOV_MAX"]
    assert isinstance(key, int)
    assert os.sysconf(key) == os.sysconf("SC_IOV_MAX")
    assert os.sysconf(key) > 0
    try:
        os.sysconf("SC_MOLT_UNKNOWN")
    except ValueError:
        pass
    else:
        raise AssertionError("unknown names must fail")
else:
    assert os.name == "nt"
print("sysconf-target-contract", True)
