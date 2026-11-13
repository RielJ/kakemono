class KakemonoCli < Formula
  desc "Wallpaper manager CLI and headless daemon"
  homepage "https://kakemono.rielj.xyz"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/RielJ/kakemono/releases/download/v@VERSION@/kakemono-cli-macos-arm64.tar.gz"
      sha256 "@SHA:kakemono-cli-macos-arm64.tar.gz@"
    end
    on_intel do
      url "https://github.com/RielJ/kakemono/releases/download/v@VERSION@/kakemono-cli-macos-x64.tar.gz"
      sha256 "@SHA:kakemono-cli-macos-x64.tar.gz@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/RielJ/kakemono/releases/download/v@VERSION@/kakemono-cli-linux-arm64.tar.gz"
      sha256 "@SHA:kakemono-cli-linux-arm64.tar.gz@"
    end
    on_intel do
      url "https://github.com/RielJ/kakemono/releases/download/v@VERSION@/kakemono-cli-linux-x64.tar.gz"
      sha256 "@SHA:kakemono-cli-linux-x64.tar.gz@"
    end
  end

  # The kakemono cask links the same `kakemono` command. Casks can't name a conflicting
  # formula, so only this side says so.
  conflicts_with cask: "kakemono"

  def install
    bin.install "kakemono"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/kakemono --version")
  end
end
