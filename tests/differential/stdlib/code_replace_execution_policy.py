# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy
"""Changing CPython frame layout requires interpreter execution (spec 0216)."""

def sample():
    return 1

# CPython accepts a larger interpreter stack; Molt must refuse before cloning.
changed = sample.__code__.replace(co_stacksize=sample.__code__.co_stacksize + 1)
print(changed.co_stacksize)
