"""Purpose: differential coverage for __notes__ propagation through split/subgroup."""

# __notes__ should be copied to subgroups created by split() and subgroup()
eg = ExceptionGroup("eg", [ValueError("a"), TypeError("b")])
eg.add_note("note on group")

match_part, rest_part = eg.split(ValueError)
print("match notes:", getattr(match_part, '__notes__', None))
print("rest notes:", getattr(rest_part, '__notes__', None))

# Verify notes are shallow-copied (not aliased)
if match_part is not None and hasattr(match_part, '__notes__'):
    match_part.__notes__.append("extra")
    print("match extra:", match_part.__notes__)
    print("rest unchanged:", rest_part.__notes__)
    print("orig unchanged:", eg.__notes__)

# subgroup should also propagate notes
sub = eg.subgroup(TypeError)
print("subgroup notes:", getattr(sub, '__notes__', None))

# __cause__ and __context__ should also propagate through split
try:
    try:
        raise RuntimeError("original")
    except RuntimeError as orig:
        eg2 = ExceptionGroup("eg2", [ValueError("x"), TypeError("y")])
        eg2.__cause__ = orig
        eg2.__context__ = orig
        m, r = eg2.split(ValueError)
        print("match cause:", type(m.__cause__).__name__ if m.__cause__ else None)
        print("match context:", type(m.__context__).__name__ if m.__context__ else None)
        print("rest cause:", type(r.__cause__).__name__ if r.__cause__ else None)
        print("rest context:", type(r.__context__).__name__ if r.__context__ else None)
except Exception as e:
    print("error:", e)

# __traceback__ should propagate through split
try:
    raise ExceptionGroup("eg3", [ValueError("t")])
except ExceptionGroup as eg3:
    m3, r3 = eg3.split(ValueError)
    print("match has traceback:", m3.__traceback__ is not None)
