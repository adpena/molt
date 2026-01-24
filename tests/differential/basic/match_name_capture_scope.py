"""Purpose: differential coverage for name capture vs value patterns across scopes."""

TARGET = 5

def run(local_value):
    match local_value:
        case TARGET:
            print("global", "hit")
        case _:
            print("global", "miss")

    TARGET = 6
    match local_value:
        case TARGET:
            print("local", "capture", TARGET)
        case _:
            print("local", "miss")

run(5)
run(6)
