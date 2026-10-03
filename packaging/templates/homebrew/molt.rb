require "shellwords"

class Molt < Formula
  desc "Verified subset Python to native/WASM compiler"
  homepage "https://github.com/adpena/molt"
  version "{{VERSION}}"
  license "Apache-2.0"

  depends_on "python@3.14"
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
    # Move whole manifest-owned directories, preserving hidden source inputs.
    prefix.install {{BUNDLE_DIRECTORIES}}
    libexec.install_symlink Formula["python@3.14"].opt_bin/"python3.14" => "python"
  end

  def caveats
    <<~EOS
      Run `molt setup --install-cli-dependencies` to authorize private CLI dependencies.
      Molt keeps mutable data outside this installation; MOLT_HOME overrides that root.

      The bundled frontend uses Python 3.14 and accepts the declared 3.12-3.14 targets.
      Set PYTHON to a CPython executable to select another frontend explicitly.
    EOS
  end

  test do
    ENV["PYTHON"] = (libexec/"python").to_s
    ENV["MOLT_HOME"] = (testpath/"molt-home").to_s
    system bin/"molt", "setup", "--install-cli-dependencies"
    system bin/"molt", "doctor", "--json"
    guest = testpath/"args.py"
    cp prefix/"source/tests/differential/basic/args_kwargs_eval_order.py", guest
    executable = testpath/"args_molt"
    system bin/"molt", "build", guest, "--python-version", "3.14",
           "--profile", "release", "--output", executable
    # CPython is an independent oracle; only the produced binary executes the guest.
    oracle = [libexec/"python", guest].map { |path| path.to_s.shellescape }.join(" ")
    assert_equal shell_output(oracle), shell_output(executable.to_s.shellescape)
  end
end
