import molt_json


def test_json_parse():
    data = "42"
    val = molt_json.parse(data)
    assert val + 1 == 43


def test_json_parse_scalars():
    assert molt_json.parse("true") is True
    assert molt_json.parse("false") is False
    assert molt_json.parse("null") is None
    assert molt_json.parse("3.5") == 3.5
    assert molt_json.parse('"hi"') == "hi"
    assert molt_json.parse("[1, 2]") == [1, 2]
    assert molt_json.parse('{"a": 1}') == {"a": 1}
