"""Purpose: str.find/index/rfind/rindex with a non-str argument. CPython 3.13
prefixed the bare "must be str, not <t>" TypeError with "<method>() argument 1 ";
3.12 used the bare form. Each method names itself (index must not inherit find's
name despite delegating to it). 3.13+ also renders None as "None" (not
"NoneType"). Version-gated; must match 3.12/3.13/3.14.
"""


def show(label, fn):
    try:
        fn()
    except TypeError as e:
        print(label, str(e))


for m in ("find", "index", "rfind", "rindex"):
    show(m + "_int", lambda m=m: getattr("abc", m)(1))
    show(m + "_none", lambda m=m: getattr("abc", m)(None))
    show(m + "_bytes", lambda m=m: getattr("abc", m)(b"a"))
