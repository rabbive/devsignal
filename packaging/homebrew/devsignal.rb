# Homebrew formula template — copy into a tap repository (e.g. homebrew-tap).
# After publishing a GitHub Release, replace OWNER, VERSION, and sha256 with:
#   shasum -a 256 devsignal-VERSION-macos-universal.tar.gz
#
#   brew install OWNER/tap/devsignal
#
class Devsignal < Formula
  desc "Unified Discord Rich Presence for AI coding CLIs on macOS"
  homepage "https://github.com/OWNER/devsignal"
  url "https://github.com/OWNER/devsignal/releases/download/vVERSION/devsignal-VERSION-macos-universal.tar.gz"
  sha256 "REPLACE_WITH_SHA256_OF_TARBALL"
  license "MIT"

  depends_on macos: :mojave

  def install
    bin.install "devsignal"
  end

  test do
    assert_path_exists bin/"devsignal"
  end
end
