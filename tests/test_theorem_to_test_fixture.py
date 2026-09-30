from tools import theorem_to_test
from tools.compat import test_policy


def test_generated_package_init_is_inert_fixture_metadata(tmp_path, monkeypatch):
    monkeypatch.setattr(theorem_to_test, "OUTPUT_DIR", tmp_path)
    assert theorem_to_test.write_tests([]) == []
    assert test_policy.parse_metadata(tmp_path / "__init__.py").source_role == "fixture"
    assert theorem_to_test.write_tests([]) == []
    assert test_policy.parse_metadata(tmp_path / "__init__.py").source_role == "fixture"


def test_generator_preserves_existing_executable_package_init(tmp_path, monkeypatch):
    monkeypatch.setattr(theorem_to_test, "OUTPUT_DIR", tmp_path)
    path = tmp_path / "__init__.py"
    path.write_text("VALUE = 1\n")
    theorem_to_test.write_tests([])
    assert path.read_text() == "VALUE = 1\n"
    assert test_policy.parse_metadata(path).source_role == "program"
