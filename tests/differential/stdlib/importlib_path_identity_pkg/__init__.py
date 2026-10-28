"""Package whose body observes its own __path__ binding."""

try:
    body_path_is_spec_locations = __path__ is __spec__.submodule_search_locations
except NameError:
    # Run directly as a script, this file has no package __path__.
    body_path_is_spec_locations = None
