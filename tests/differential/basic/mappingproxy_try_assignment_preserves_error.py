"""Purpose: ensure mappingproxy assignment raises and surfaces the original TypeError."""


class Demo:
    value = 1


try:
    Demo.__dict__["other"] = 3
except Exception as exc:
    print(type(exc).__name__)
    print(str(exc))
else:
    print("NO_EXCEPTION")
