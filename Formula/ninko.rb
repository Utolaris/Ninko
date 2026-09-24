class Ninko < Formula
  desc "Lightweight Clash Verge CLI for humans and AI agents"
  homepage "https://github.com/Utolaris/Ninko"
  url "https://github.com/Utolaris/Ninko/archive/refs/tags/v1.0.0.tar.gz"
  sha256 "ccb0f49e6e550a0bfb0689830648c6b2a4165a1ac1cb92ec15248b4908eb95fd"
  license "MIT"
  head "https://github.com/Utolaris/Ninko.git", branch: "main"

  bottle do
    root_url "https://github.com/Utolaris/Ninko/releases/download/v1.0.0"
    sha256 cellar: any, arm64_sequoia: "5d673f835986c5017c028bb25de81e03c6dc281d3990f27e27deb51c3f185a8c"
    sha256 cellar: any, x86_64_linux:  "1997051548e7346e778e78ceea47afce5b2af123020e82cf52659df2632fe308"
  end

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ninko --version")
  end
end
