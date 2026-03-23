import tomli

data = tomli.loads('[project]\nname = "molt"\nversion = "1.0"')
print("tomli loaded")
print("name:", data["project"]["name"])
print("version:", data["project"]["version"])
