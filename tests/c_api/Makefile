CC = clang
CFLAGS = -I../../include/molt -I../../include -Wall -Wextra -Wpedantic
SANITIZERS = -fsanitize=address,undefined -fno-omit-frame-pointer

.PHONY: all test lint sanitize clean

all: test lint

test: test_python_h
	./test_python_h

test_python_h: test_python_h.c ../../include/molt/Python.h
	$(CC) $(CFLAGS) -o $@ $<

# Build and run with AddressSanitizer + UBSan
sanitize: test_python_h.c ../../include/molt/Python.h
	$(CC) $(CFLAGS) $(SANITIZERS) -o test_python_h_asan $<
	./test_python_h_asan

# Validate the full header for compilation warnings and lint issues
lint:
	bash lint_python_h.sh

# Verify the header parses as valid C with only syntax checking
syntax-check:
	$(CC) -I../../include/molt -I../../include -fsyntax-only -Wall -Wextra ../../include/molt/Python.h 2>&1 || true

clean:
	rm -f test_python_h test_python_h_asan
