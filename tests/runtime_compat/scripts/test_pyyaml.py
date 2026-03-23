import yaml

print("pyyaml", yaml.__version__)
data = {"name": "Molt", "features": ["compile", "wasm", "native"]}
dumped = yaml.dump(data, default_flow_style=True).strip()
loaded = yaml.safe_load(dumped)
print("dumped:", dumped)
print("equal:", data == loaded)
