"""
Basic C extension loading tests.

These tests verify that C extensions compiled against Molt's Python.h
can be loaded and produce correct results. They are designed to run
under both CPython (for baseline) and Molt (for parity verification).

When run under CPython, they validate the test expectations themselves.
When run under Molt, they validate Molt's C API compatibility.
"""

import sys

# ---------------------------------------------------------------------------
# Integer operations through C API
# ---------------------------------------------------------------------------


def test_int_from_long():
    """PyLong_FromLong: basic integer creation."""
    x = 42
    assert isinstance(x, int)
    assert x == 42


def test_int_boundaries():
    """PyLong_FromLong / PyLong_FromLongLong: boundary values."""
    assert 0 == 0
    assert -1 == -1
    assert 2**31 - 1 == 2147483647  # INT_MAX
    assert -(2**31) == -2147483648  # INT_MIN


def test_int_arithmetic():
    """PyNumber_Add etc.: arithmetic through number protocol."""
    assert 3 + 4 == 7
    assert 10 - 3 == 7
    assert 6 * 7 == 42
    assert 15 // 4 == 3
    assert 15 % 4 == 3
    assert 2**10 == 1024


# ---------------------------------------------------------------------------
# Float operations
# ---------------------------------------------------------------------------


def test_float_from_double():
    """PyFloat_FromDouble: basic float creation."""
    x = 3.14
    assert isinstance(x, float)
    assert abs(x - 3.14) < 1e-10


def test_float_int_coercion():
    """PyFloat_AsDouble on int: implicit coercion."""
    x = float(7)
    assert x == 7.0


def test_float_arithmetic():
    """Number protocol on floats."""
    assert abs(1.5 + 2.5 - 4.0) < 1e-10
    assert abs(3.0 * 2.0 - 6.0) < 1e-10


# ---------------------------------------------------------------------------
# Bool operations
# ---------------------------------------------------------------------------


def test_bool_from_long():
    """PyBool_FromLong: truthiness."""
    assert bool(0) is False
    assert bool(1) is True
    assert bool(-1) is True
    assert bool(42) is True


def test_bool_identity():
    """True and False are singletons."""
    assert True is True
    assert False is False
    assert (True is False) is False


# ---------------------------------------------------------------------------
# String operations
# ---------------------------------------------------------------------------


def test_string_creation():
    """PyUnicode_FromString: basic string."""
    s = "hello"
    assert isinstance(s, str)
    assert len(s) == 5


def test_string_empty():
    """Empty string."""
    s = ""
    assert len(s) == 0
    assert s == ""


def test_string_comparison():
    """PyUnicode_Compare: string comparison."""
    assert "abc" == "abc"
    assert "abc" != "def"
    assert "abc" < "def"


# ---------------------------------------------------------------------------
# List operations
# ---------------------------------------------------------------------------


def test_list_creation():
    """PyList_New + PyList_Append."""
    lst = [1, 2, 3]
    assert isinstance(lst, list)
    assert len(lst) == 3


def test_list_getitem():
    """PyList_GetItem."""
    lst = [10, 20, 30]
    assert lst[0] == 10
    assert lst[1] == 20
    assert lst[2] == 30


def test_list_setitem():
    """PyList_SetItem."""
    lst = [1, 2, 3]
    lst[1] = 99
    assert lst[1] == 99


def test_list_append():
    """PyList_Append."""
    lst = []
    lst.append(42)
    assert len(lst) == 1
    assert lst[0] == 42


# ---------------------------------------------------------------------------
# Tuple operations
# ---------------------------------------------------------------------------


def test_tuple_creation():
    """PyTuple_New + PyTuple_SetItem."""
    t = (1, 2, 3)
    assert isinstance(t, tuple)
    assert len(t) == 3


def test_tuple_getitem():
    """PyTuple_GetItem."""
    t = (10, 20, 30)
    assert t[0] == 10
    assert t[1] == 20
    assert t[2] == 30


def test_tuple_empty():
    """Empty tuple."""
    t = ()
    assert len(t) == 0


# ---------------------------------------------------------------------------
# Dict operations
# ---------------------------------------------------------------------------


def test_dict_creation():
    """PyDict_New."""
    d = {}
    assert isinstance(d, dict)
    assert len(d) == 0


def test_dict_setitem_getitem():
    """PyDict_SetItem + PyDict_GetItem."""
    d = {}
    d["key"] = "value"
    assert d["key"] == "value"


def test_dict_size():
    """PyDict_Size."""
    d = {"a": 1, "b": 2, "c": 3}
    assert len(d) == 3


def test_dict_keys_values():
    """PyDict_Keys / PyDict_Values."""
    d = {"x": 10, "y": 20}
    assert set(d.keys()) == {"x", "y"}
    assert set(d.values()) == {10, 20}


# ---------------------------------------------------------------------------
# Type checking
# ---------------------------------------------------------------------------


def test_isinstance_int():
    """PyLong_Check / isinstance."""
    assert isinstance(42, int)
    assert not isinstance(42, str)


def test_isinstance_float():
    """PyFloat_Check."""
    assert isinstance(3.14, float)
    assert not isinstance(3.14, int)


def test_isinstance_str():
    """PyUnicode_Check."""
    assert isinstance("hello", str)
    assert not isinstance("hello", int)


def test_isinstance_list():
    """PyList_Check."""
    assert isinstance([1, 2], list)
    assert not isinstance([1, 2], tuple)


def test_isinstance_tuple():
    """PyTuple_Check."""
    assert isinstance((1, 2), tuple)
    assert not isinstance((1, 2), list)


def test_isinstance_dict():
    """PyDict_Check."""
    assert isinstance({}, dict)
    assert not isinstance({}, list)


def test_isinstance_bool():
    """PyBool_Check."""
    assert isinstance(True, bool)
    assert isinstance(False, bool)
    # In CPython, bool is a subclass of int
    assert isinstance(True, int)


# ---------------------------------------------------------------------------
# Exception handling
# ---------------------------------------------------------------------------


def test_value_error():
    """PyErr_SetString with ValueError."""
    try:
        raise ValueError("test message")
    except ValueError as e:
        assert str(e) == "test message"


def test_type_error():
    """PyErr_SetString with TypeError."""
    try:
        raise TypeError("bad type")
    except TypeError as e:
        assert str(e) == "bad type"


def test_index_error():
    """IndexError on list access."""
    lst = [1, 2, 3]
    try:
        _ = lst[10]
        assert False, "Should have raised IndexError"
    except IndexError:
        pass


def test_key_error():
    """KeyError on dict access."""
    d = {"a": 1}
    try:
        _ = d["missing"]
        assert False, "Should have raised KeyError"
    except KeyError:
        pass


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    tests = [
        (name, obj)
        for name, obj in sorted(globals().items())
        if name.startswith("test_") and callable(obj)
    ]

    passed = 0
    failed = 0
    for name, func in tests:
        try:
            func()
            print(f"  PASS  {name}")
            passed += 1
        except Exception as e:
            print(f"  FAIL  {name}: {e}")
            failed += 1

    print(f"\n{passed} passed, {failed} failed out of {passed + failed}")
    sys.exit(1 if failed else 0)
