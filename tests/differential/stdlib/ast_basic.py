"""Purpose: differential coverage for ast basics."""

import ast

source = """
def foo(x):
    \"\"\"doc\"\"\"
    return x + 1
""".strip()

tree = ast.parse(source)
func = tree.body[0]
print(ast.get_docstring(func))
node_names = {type(node).__name__ for node in ast.walk(tree)}
print("FunctionDef" in node_names, "Return" in node_names, "Name" in node_names)
print(len(node_names))
expr = ast.parse("1+2", mode="eval")
print(isinstance(expr, ast.Expression))
print(hasattr(ast, "PyCF_ALLOW_TOP_LEVEL_AWAIT"))
