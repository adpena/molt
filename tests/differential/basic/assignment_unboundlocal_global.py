"""Purpose: differential coverage for UnboundLocalError/global assignment semantics."""

x = "root"


def unbound_read_then_assign():
    try:
        print("unbound", x)
    except Exception as exc:
        print("unbound", type(exc).__name__)
    x = "local"
    return x


def global_read_then_assign():
    global x
    print("global_before", x)
    x = "changed"
    print("global_after", x)


unbound_read_then_assign()
global_read_then_assign()
print("module_x", x)
