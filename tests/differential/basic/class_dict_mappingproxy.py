"""Purpose: verify class __dict__ returns mappingproxy view."""

if __name__ == "__main__":
    class Sample:
        value = 3

    proxy = Sample.__dict__
    print(type(proxy).__name__)
    print("value" in proxy)
    try:
        proxy["other"] = 1
        print("set_ok")
    except TypeError:
        print("TypeError")
