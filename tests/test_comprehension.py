def test_list_comp_basic(capsys):
    # Basic list comprehension
    nums = [1, 2, 3]
    squares = [x * x for x in nums]
    print(squares)

    # Verify output
    assert squares == [1, 4, 9]


def test_list_comp_filter(capsys):
    # List comprehension with if
    nums = [1, 2, 3, 4]
    evens = [x for x in nums if x % 2 == 0]
    print(evens)

    # Verify output
    assert evens == [2, 4]
