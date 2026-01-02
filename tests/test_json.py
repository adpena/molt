import molt_json


def test_json_parse():
    data = "42"
    val = molt_json.parse(data)
    assert val + 1 == 43
