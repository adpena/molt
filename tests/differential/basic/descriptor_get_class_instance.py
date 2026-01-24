"""Purpose: differential coverage for descriptor __get__ on class vs instance."""

class Marker:
    def __get__(self, obj, owner):
        return (obj is None, owner.__name__)


class Demo:
    marker = Marker()


if __name__ == "__main__":
    print("class", Demo.marker)
    print("instance", Demo().marker)
