# The Homebrew cask for Lumen. The release workflow fills in the version and
# checksum and pushes it to github.com/lumenshell/homebrew-tap as Casks/lumen.rb:
#
#   brew install --cask lumenshell/tap/lumen
cask "lumen" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/lumenshell/lumen/releases/download/v#{version}/lumen-macos.zip"
  name "Lumen"
  desc "Fast, GPU-rendered terminal"
  homepage "https://lumenshell.dev"

  livecheck do
    url :url
    strategy :github_latest
  end

  app "Lumen.app"
  binary "#{appdir}/Lumen.app/Contents/MacOS/lumen"

  zap trash: [
    "~/.config/lumen",
    "~/Library/Application Support/lumen",
  ]
end
