"""A noncanonical class builder is excluded by the static class model."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy

import builtins


def replacement(*args, **kwargs):
    return 17


builtins.__build_class__ = replacement


class C:
    pass


print("class statement completed")
