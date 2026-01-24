"""Purpose: differential coverage for match mapping patterns with **rest."""

value = {"a": 1, "b": 2, "c": 3}
match value:
    case {"a": 1, **rest}:
        print("rest", sorted(rest.items()))
    case _:
        print("rest", "miss")

match value:
    case {"a": 2, **rest}:
        print("rest2", sorted(rest.items()))
    case _:
        print("rest2", "miss")
