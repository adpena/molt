import pydantic

print("pydantic", pydantic.__version__)
print("BaseModel exists:", hasattr(pydantic, "BaseModel"))
print("Field exists:", hasattr(pydantic, "Field"))
