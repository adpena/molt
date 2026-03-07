class MoltLang < Formula
  desc "AOT Python compiler targeting native binaries, WASM, and Luau"
  homepage "https://github.com/adpena/molt"
  url "https://github.com/adpena/molt/archive/refs/tags/v0.1.0.tar.gz"
  # sha256 will be filled after first release
  license "Apache-2.0"

  depends_on "rust" => :build
  depends_on "python@3.13"

  def install
    # Build the Rust backend
    system "cargo", "build", "--release",
           "--manifest-path", "runtime/molt-backend/Cargo.toml"

    # Install the backend binary
    bin.install "target/release/molt-backend"

    # Install the Python frontend
    system "python3", "-m", "pip", "install", "--prefix=#{prefix}",
           "--no-deps", "--no-build-isolation", "."

    # Create the `molt` CLI wrapper that finds the backend
    (bin/"molt").write <<~SH
      #!/bin/bash
      export MOLT_BACKEND_BIN="#{opt_bin}/molt-backend"
      exec "#{libexec}/bin/python3" -m molt.cli "$@"
    SH
  end

  test do
    # Write a minimal Python test
    (testpath/"hello.py").write('print("hello")')
    # Verify CLI starts
    assert_match "molt", shell_output("#{bin}/molt --help")
  end
end
