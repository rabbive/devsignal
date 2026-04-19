# Homebrew formula template — copy into a tap repository (e.g. homebrew-tap).
# After the matching GitHub Release exists, set sha256:
#   curl -fsSL -o t.tgz "https://github.com/rabbive/devsignal/releases/download/v0.2.0/devsignal-0.2.0-macos-universal.tar.gz"
#   shasum -a 256 t.tgz
#
#   brew install YOURTAP/devsignal
#
class Devsignal < Formula
  desc "Unified Discord Rich Presence for AI coding CLIs on macOS"
  homepage "https://github.com/rabbive/devsignal"
  url "https://github.com/rabbive/devsignal/releases/download/v0.2.0/devsignal-0.2.0-macos-universal.tar.gz"
  # Replace with output of shasum after the v0.2.0 release tarball is published (placeholder fails brew until updated).
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"

  depends_on macos: :mojave

  def install
    bin.install "devsignal"
  end

  test do
    assert_path_exists bin/"devsignal"
  end
end
