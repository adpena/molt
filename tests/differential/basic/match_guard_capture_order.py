"""Purpose: differential coverage for match guard evaluation and capture binding."""


def test_guard_binding():
    x = "outer"
    value = {"kind": "ok", "value": 1}
    match value:
        case {"kind": "ok", "value": x} if False:
            print("guard", "hit", x)
        case _:
            print("guard", "miss", x)



def test_guard_eval_order():
    events = []

    def guard(v):
        events.append(f"guard:{v}")
        return False

    value = (1, 2)
    match value:
        case (a, b) if guard(a):
            print("order", a, b)
        case _:
            print("order", "miss")

    print("events", events)



test_guard_binding()
test_guard_eval_order()
