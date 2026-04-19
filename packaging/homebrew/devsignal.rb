# Homebrew formula template — copy into a tap repository (e.g. homebrew-tap).
# Future bumps: match url to release tag, then:
#   curl -fsSL -o t.tgz "https://github.com/rabbive/devsignal/releases/download/vVERSION/devsignal-VERSION-macos-universal.tar.gz"
#   shasum -a 256 t.tgz
#
class Devsignal < Formula
  desc "Unified Discord Rich Presence for AI coding CLIs on macOS"
  homepage "https://github.com/rabbive/devsignal"
  url "https://github.com/rabbive/devsignal/releases/download/v0.2.0/devsignal-0.2.0-macos-universal.tar.gz"
  sha256 "4c7b96fe6a1507c6bb286ba5fd4e24685de1830b4db86a126c87d4ee56461f08"
  license "MIT"

  depends_on macos: :mojave

  def install
    bin.install "devsignal"
  end

  test do
    assert_path_exists bin/"devsignal"
  end
end
