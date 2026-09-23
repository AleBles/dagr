# Homebrew formula for Dagr. The copy that users install from lives in the
# tap repository, alebles/homebrew-tap, as Formula/dagr.rb:
#
#   brew install alebles/tap/dagr
#
# After tagging a release, point `url` at its tarball and fill in `sha256`:
#
#   curl -sL https://github.com/alebles/dagr/archive/refs/tags/vX.Y.Z.tar.gz | shasum -a 256
class Dagr < Formula
  desc "Priority-ordered task list with a built-in MCP server"
  homepage "https://dagr.bles.nu"
  url "https://github.com/alebles/dagr/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"
  head "https://github.com/alebles/dagr.git", branch: "main"

  depends_on "pkgconf" => :build
  depends_on "rust" => :build
  depends_on "adwaita-icon-theme"
  depends_on "gtk4"
  depends_on "libadwaita"

  def install
    system "cargo", "install", *std_cargo_args
    (share/"icons/hicolor/scalable/apps").install "data/icons/hicolor/scalable/apps/nu.bles.dagr.svg"
    (share/"icons/hicolor/symbolic/apps").install "data/icons/hicolor/symbolic/apps/nu.bles.dagr-symbolic.svg"

    # A Dagr.app, because Spotlight and Launchpad only find app bundles. Its
    # launcher execs the `opt` binary rather than holding a copy, so the bundle
    # copied into ~/Applications (see caveats) keeps working across upgrades.
    contents = prefix/"Dagr.app/Contents"
    contents.install "data/macos/Info.plist"
    inreplace contents/"Info.plist", "@VERSION@", version.to_s
    (contents/"Resources").install "data/macos/dagr.icns"
    (contents/"MacOS/dagr").write <<~SH
      #!/bin/sh
      exec "#{opt_bin}/dagr" "$@"
    SH
    chmod 0755, contents/"MacOS/dagr"
  end

  # Homebrew may not write outside its prefix, so this one step is yours.
  def caveats
    <<~EOS
      To find Dagr in Spotlight and Launchpad, copy the app into place once:
        cp -R #{opt_prefix}/Dagr.app ~/Applications/
      It starts the Homebrew binary, so upgrades need no new copy.
    EOS
  end

  # `brew services start dagr` runs the background service at login, which is
  # what `dagr setup` suggests for a Homebrew install.
  service do
    run [opt_bin/"dagr", "serve"]
    keep_alive successful_exit: false
    log_path var/"log/dagr.log"
    error_log_path var/"log/dagr.log"
  end

  test do
    # Nothing is listening in the sandbox, so status reports that and exits 1.
    ENV["DAGR_SOCKET"] = testpath/"dagr.sock"
    assert_match "no dagr service", shell_output("#{bin}/dagr status", 1)
  end
end
