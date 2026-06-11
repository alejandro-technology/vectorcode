class Vectorcode < Formula
  desc "Semantic code search MCP server — find code by meaning, not by name"
  homepage "https://github.com/alejandro-technology/vectorcode"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/alejandro-technology/vectorcode/releases/download/v#{version}/vectorcode-darwin-arm64.tar.gz"
      sha256 "PLACEHOLDER_ARM64_SHA256"
    else
      url "https://github.com/alejandro-technology/vectorcode/releases/download/v#{version}/vectorcode-darwin-x86_64.tar.gz"
      sha256 "PLACEHOLDER_X86_64_SHA256"
    end
  end

  def install
    bin.install "vectorcode"
  end

  test do
    assert_match "vectorcode", shell_output("#{bin}/vectorcode --version")
  end
end
