import toml

print("toml", toml.__version__)
data = {"project": {"name": "molt", "version": "1.0"}}
dumped = toml.dumps(data)
loaded = toml.loads(dumped)
print("dumped:", repr(dumped.strip()))
print("equal:", data == loaded)
