"""Purpose: CPython parity for item assignment/deletion on immutable sequences.
`t[i]=x`, `t[i:j]=...`, `del t[i]`, `del t[i:j]` on tuple/range must raise
TypeError (previously a silent no-op in molt — a divergence). Wording is
version-stable across 3.12/3.13/3.14, with CPython's deliberate asymmetry:
assignment always "does not support item assignment"; deletion says "doesn't
support item deletion" for an index but "does not support item deletion" for a
slice. Statement forms (not __setitem__/__delitem__, which would raise
AttributeError) so the sq_ass_item / subscript-del slots are exercised.
"""

t = (1, 2, 3)
r = range(5)

try:
    t[0] = 9
except Exception as e:
    print("t_set_idx", type(e).__name__, str(e))
try:
    t[0:1] = [9]
except Exception as e:
    print("t_set_slice", type(e).__name__, str(e))
try:
    r[0] = 9
except Exception as e:
    print("r_set_idx", type(e).__name__, str(e))
try:
    r[0:1] = [9]
except Exception as e:
    print("r_set_slice", type(e).__name__, str(e))
try:
    del t[0]
except Exception as e:
    print("t_del_idx", type(e).__name__, str(e))
try:
    del t[0:1]
except Exception as e:
    print("t_del_slice", type(e).__name__, str(e))
try:
    del r[0]
except Exception as e:
    print("r_del_idx", type(e).__name__, str(e))
try:
    del r[0:1]
except Exception as e:
    print("r_del_slice", type(e).__name__, str(e))
print("DONE")
