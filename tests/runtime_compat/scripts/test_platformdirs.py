import platformdirs

print("platformdirs", platformdirs.__version__)
dirs = platformdirs.PlatformDirs("molt", "molt-project")
print("user_data_dir type:", type(dirs.user_data_dir).__name__)
print("user_config_dir type:", type(dirs.user_config_dir).__name__)
print("user_cache_dir type:", type(dirs.user_cache_dir).__name__)
