class MoltWorker < Formula
  desc "Molt worker helper for offload/DB IPC"
  homepage "https://github.com/adpena/molt"
  version "{{VERSION}}"
  license "Apache-2.0"

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
  end

  test do
    system bin/"molt-worker", "--help"
  end
end
