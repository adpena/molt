"""Microbench: typed builtin-exception construction and member access.

The layout schema makes every operation below O(1): construction allocates the
exception itself and its required args, while typed members live inline.  In
particular, AttributeError must not allocate a `(name, obj)` tuple and OSError /
UnicodeError must not allocate a typed-field dictionary.  Run with the runtime
profile enabled so `alloc_tuple`, `alloc_dict`, total allocated bytes, and wall
time can be compared across commits; wall time alone cannot prove the structural
allocation removal.
"""


class Owner:
    pass


if __import__("sys").implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _profile_epoch_reset = load_intrinsic("molt_profile_epoch_reset")
    _profile_epoch_dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _profile_epoch_reset = None
    _profile_epoch_dump = None


def main() -> None:
    owner = Owner()
    checksum = 0
    if _profile_epoch_reset is not None:
        _profile_epoch_reset("exception_typed_fields")
    for index in range(200_000):
        attribute = AttributeError("missing", name="field", obj=owner)
        checksum += attribute.obj is owner
        checksum += attribute.name == "field"

        os_error = OSError(2, "missing", "input.txt")
        checksum += os_error.errno == 2
        checksum += os_error.filename == "input.txt"

        unicode_error = UnicodeEncodeError("utf-8", "x", 0, 1, "reason")
        checksum += unicode_error.start
        checksum += unicode_error.end
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()
    print(checksum)


if __name__ == "__main__":
    main()
