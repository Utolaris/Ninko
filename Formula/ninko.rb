class Ninko < Formula
  desc "Lightweight Clash Verge CLI for humans and AI agents"
  homepage "https://github.com/Utolaris/Ninko"
  url "https://github.com/Utolaris/Ninko/archive/refs/tags/v1.0.0.tar.gz"
  sha256 "ccb0f49e6e550a0bfb0689830648c6b2a4165a1ac1cb92ec15248b4908eb95fd"
  license "MIT"
  head "https://github.com/Utolaris/Ninko.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ninko --version")
  end
end
