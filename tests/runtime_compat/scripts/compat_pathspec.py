import pathspec

print("pathspec", pathspec.__version__)
spec = pathspec.PathSpec.from_lines("gitwildmatch", ["*.py", "!test_*"])
print("match src.py:", spec.match_file("src.py"))
print("match test_x.py:", spec.match_file("test_x.py"))
print("match readme.md:", spec.match_file("readme.md"))
