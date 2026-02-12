"""Purpose: differential coverage for gc collect."""

import gc


def show(label, value):
    print(label, value)


show("gc_collect_type", type(gc.collect()).__name__)
gc.disable()
show("gc_enabled_after_disable", gc.isenabled())
gc.enable()
show("gc_enabled_after_enable", gc.isenabled())
