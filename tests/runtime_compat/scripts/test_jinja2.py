import jinja2

print("jinja2", jinja2.__version__)
template = jinja2.Template("Hello {{ name }}!")
result = template.render(name="Molt")
print("rendered:", result)
