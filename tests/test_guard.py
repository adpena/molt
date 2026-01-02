def test_guarded_attribute_access():
    class User:
        id: int

    user = User()
    user.id = 123
    # Simulate a typed attribute access path.
    val = user.id
    assert val == 123
