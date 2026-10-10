import click

print("click", click.__version__)
print("click.echo exists:", hasattr(click, "echo"))
print("click.command exists:", hasattr(click, "command"))
print("click.option exists:", hasattr(click, "option"))
