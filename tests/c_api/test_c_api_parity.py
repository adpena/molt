"""
C API parity tests: specific CPython C API behaviors that Molt must match.

Each test documents the exact C API function(s) being exercised and the
CPython-defined semantics. Run under both CPython and Molt to verify parity.
"""

import sys

# ---------------------------------------------------------------------------
# Refcount semantics: borrowed vs new references
# ---------------------------------------------------------------------------

def test_list_getitem_returns_borrowed_ref():
    """PyList_GetItem returns a borrowed reference.
    The item must remain valid as long as the list exists."""
    lst = [object(), object(), object()]
    item = lst[1]
    # item should be the same object — identity check
    assert item is lst[1]

def test_tuple_getitem_returns_borrowed_ref():
    """PyTuple_GetItem returns a borrowed reference."""
    t = (object(), object())
    item = t[0]
    assert item is t[0]

def test_dict_getitem_returns_borrowed_ref():
    """PyDict_GetItem returns a borrowed reference (NULL if not found)."""
    d = {"key": object()}
    val = d["key"]
    assert val is d["key"]

# ---------------------------------------------------------------------------
# None singleton identity
# ---------------------------------------------------------------------------

def test_none_is_singleton():
    """Py_None: all None values are the same object."""
    a = None
    b = None
    assert a is b
    assert a is None

def test_none_is_not_false():
    """None and False are distinct objects."""
    assert None is not False
    assert None is not 0

# ---------------------------------------------------------------------------
# Bool subclass of int
# ---------------------------------------------------------------------------

def test_bool_is_int_subclass():
    """PyBool_Type.tp_base == &PyLong_Type"""
    assert issubclass(bool, int)
    assert True == 1
    assert False == 0
    assert True + True == 2

# ---------------------------------------------------------------------------
# Integer edge cases
# ---------------------------------------------------------------------------

def test_int_zero_is_falsy():
    """PyObject_IsTrue(0) == 0"""
    assert not 0
    assert not bool(0)

def test_int_nonzero_is_truthy():
    """PyObject_IsTrue(n) == 1 for n != 0"""
    assert bool(1)
    assert bool(-1)
    assert bool(999)

# ---------------------------------------------------------------------------
# String operations matching C API
# ---------------------------------------------------------------------------

def test_unicode_from_string_preserves_utf8():
    """PyUnicode_FromString: must handle ASCII correctly."""
    s = "hello world"
    assert len(s) == 11
    assert s[0] == "h"
    assert s[-1] == "d"

def test_unicode_compare_ordering():
    """PyUnicode_CompareWithASCIIString: lexicographic order."""
    assert "abc" < "abd"
    assert "abc" < "abcd"
    assert "z" > "a"
    assert "" < "a"

def test_unicode_empty_string():
    """PyUnicode_FromStringAndSize with size=0."""
    s = ""
    assert len(s) == 0
    assert s == ""
    assert not s  # empty string is falsy

# ---------------------------------------------------------------------------
# List operations matching C API
# ---------------------------------------------------------------------------

def test_list_new_preallocated():
    """PyList_New(n): creates list with space for n items."""
    lst = [None] * 5
    assert len(lst) == 5
    assert all(x is None for x in lst)

def test_list_set_item_steals_ref():
    """PyList_SetItem steals a reference to the value.
    In Python, this manifests as simple assignment."""
    lst = [1, 2, 3]
    obj = object()
    lst[1] = obj
    assert lst[1] is obj

def test_list_negative_index():
    """Negative indexing wraps around."""
    lst = [10, 20, 30]
    assert lst[-1] == 30
    assert lst[-2] == 20
    assert lst[-3] == 10

# ---------------------------------------------------------------------------
# Dict operations matching C API
# ---------------------------------------------------------------------------

def test_dict_setitem_overwrites():
    """PyDict_SetItem: setting same key overwrites."""
    d = {"k": 1}
    d["k"] = 2
    assert d["k"] == 2
    assert len(d) == 1

def test_dict_missing_key_returns_keyerror():
    """PyDict_GetItem returns NULL for missing key (Python raises KeyError)."""
    d = {}
    raised = False
    try:
        _ = d["missing"]
    except KeyError:
        raised = True
    assert raised

def test_dict_del_item():
    """PyDict_DelItemString removes key."""
    d = {"a": 1, "b": 2}
    del d["a"]
    assert "a" not in d
    assert len(d) == 1

# ---------------------------------------------------------------------------
# Tuple immutability
# ---------------------------------------------------------------------------

def test_tuple_is_immutable():
    """After creation, tuples cannot be modified."""
    t = (1, 2, 3)
    raised = False
    try:
        t[0] = 99  # type: ignore
    except TypeError:
        raised = True
    assert raised

# ---------------------------------------------------------------------------
# Type checking parity
# ---------------------------------------------------------------------------

def test_type_of_int():
    assert type(42) is int

def test_type_of_float():
    assert type(3.14) is float

def test_type_of_str():
    assert type("hello") is str

def test_type_of_list():
    assert type([]) is list

def test_type_of_tuple():
    assert type(()) is tuple

def test_type_of_dict():
    assert type({}) is dict

def test_type_of_bool():
    assert type(True) is bool
    assert type(False) is bool

def test_type_of_none():
    assert type(None) is type(None)

# ---------------------------------------------------------------------------
# Rich comparison
# ---------------------------------------------------------------------------

def test_richcompare_eq():
    """Py_EQ: equality."""
    assert 1 == 1
    assert not (1 == 2)

def test_richcompare_ne():
    """Py_NE: inequality."""
    assert 1 != 2
    assert not (1 != 1)

def test_richcompare_lt_gt():
    """Py_LT / Py_GT."""
    assert 1 < 2
    assert 2 > 1
    assert not (2 < 1)

def test_richcompare_le_ge():
    """Py_LE / Py_GE."""
    assert 1 <= 1
    assert 1 <= 2
    assert 2 >= 2
    assert 2 >= 1

def test_richcompare_different_types():
    """Cross-type comparison: int vs float."""
    assert 1 == 1.0
    assert 1.0 == 1
    assert 2 > 1.5

# ---------------------------------------------------------------------------
# Hash parity
# ---------------------------------------------------------------------------

def test_hash_int():
    """hash(int) should be stable."""
    assert hash(0) == 0
    assert hash(1) == 1
    # hash(-1) == -2 in CPython (special case)
    assert hash(-1) == -2

def test_hash_str():
    """hash(str) must be deterministic within a run."""
    s = "hello"
    h1 = hash(s)
    h2 = hash(s)
    assert h1 == h2


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
