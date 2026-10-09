import colorama

print("colorama", colorama.__version__)
print("Fore.RED:", repr(colorama.Fore.RED))
print("Style.RESET_ALL:", repr(colorama.Style.RESET_ALL))
print("init exists:", hasattr(colorama, "init"))
