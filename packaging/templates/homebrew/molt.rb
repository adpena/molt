class Molt < Formula
  desc "Verified subset Python to native/WASM compiler"
  homepage "https://github.com/adpena/molt"
  version "{{VERSION}}"
  license "Apache-2.0"

  depends_on "python@3.12"
  depends_on "uv"
  # Source and native identities are sealed by the release manifest.
  skip_clean :all

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
    share.install Dir["share/*"] if Dir.exist?("share")
    prefix.install "source"
    libexec.install_symlink Formula["python@3.12"].opt_bin/"python3.12" => "python"
  end

  def caveats
    <<~EOS
      Run `molt setup --install-cli-dependencies` to authorize private CLI dependencies.
      Molt keeps mutable data outside this installation; MOLT_HOME overrides that root.

      For local development, prefer running the CLI from the repo:
        PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli build examples/hello.py
    EOS
  end

  test do
    system bin/"molt", "setup", "--install-cli-dependencies"
    system bin/"molt", "doctor", "--json"
  end
end
