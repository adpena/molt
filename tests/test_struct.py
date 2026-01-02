def test_structified_class_layout():
    class Point:
        x: int
        y: int

    point = Point()
    point.x = 10
    point.y = 32
    assert point.x + point.y == 42
