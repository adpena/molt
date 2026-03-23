import wcwidth

print("wcwidth", wcwidth.__version__)
print("width A:", wcwidth.wcwidth("A"))
print("width CJK:", wcwidth.wcwidth("\u4e16"))
print("wcswidth hello:", wcwidth.wcswidth("hello"))
