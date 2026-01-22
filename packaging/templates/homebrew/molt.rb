class Molt < Formula
  desc "Verified subset Python to native/WASM compiler"
  homepage "https://github.com/adpena/molt"
  version "{{VERSION}}"
  license "Apache-2.0"

  depends_on "python@3.12"

  on_macos do
    if Hardware::CPU.arm?
      url "{{MAC_ARM_URL}}"
      sha256 "{{MAC_ARM_SHA256}}"
    else
      url "{{MAC_X86_URL}}"
      sha256 "{{MAC_X86_SHA256}}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "{{LINUX_ARM_URL}}"
      sha256 "{{LINUX_ARM_SHA256}}"
    else
      url "{{LINUX_X86_URL}}"
      sha256 "{{LINUX_X86_SHA256}}"
    end
  end

  def install
    bin.install Dir["bin/*"]
    lib.install Dir["lib/*"] if Dir.exist?("lib")
    share.install Dir["share/*"] if Dir.exist?("share")
  end

  def caveats
    <<~EOS
      Molt stores build artifacts in ~/.molt by default.

      For local development, prefer running the CLI from the repo:
        PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli build examples/hello.py
    EOS
  end

  test do
    system bin/"molt", "doctor", "--json"
  end
end
