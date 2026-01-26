"""Purpose: differential coverage for control flow complex."""


def section(name):
    print(f"--- {name} ---")


section("Break in Try/Finally")


def break_finally():
    print("start")
    for i in range(3):
        try:
            print(f"loop {i}")
            if i == 1:
                print("breaking")
                break
        finally:
            print(f"finally {i}")
    print("end")


break_finally()

section("Continue in Try/Finally")


def continue_finally():
    print("start")
    for i in range(3):
        try:
            print(f"loop {i}")
            if i == 1:
                print("continuing")
                continue
        finally:
            print(f"finally {i}")
    print("end")


continue_finally()

section("Return in Finally")


def return_finally():
    try:
        return "try-return"
    finally:
        print("executing finally")
        # In Python, a return in finally overrides the return in try
        return "finally-return"


print(return_finally())

section("Nested loops with break")


def nested_break():
    for i in range(3):
        print(f"outer {i}")
        for j in range(3):
            print(f"  inner {j}")
            if i == 1 and j == 1:
                print("  breaking inner")
                break
        else:
            print("  inner else ran")
    else:
        print("outer else ran")


nested_break()
