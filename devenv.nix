{ pkgs, lib, config, inputs, ... }:

let
  pkgs' = pkgs.extend inputs.rust-overlay.overlays.default;
  rustToolchain = pkgs'.rust-bin.stable.latest.default.override {
    extensions = [ "rust-src" "rustfmt" "clippy" ];
  };
in
{
  # Node.js 22
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
  };

  packages = [
    rustToolchain
    pkgs.just
    pkgs.ruby
    pkgs.git
    pkgs.jujutsu
    pkgs.pkg-config
    pkgs.openssl
    pkgs.glib
    pkgs.gtk3
    pkgs.libsoup_3
    pkgs.webkitgtk_4_1
    pkgs.libappindicator-gtk3
    pkgs.librsvg
    pkgs.patchelf
    pkgs.gdk-pixbuf
    pkgs.dbus
    pkgs.pango
    pkgs.cairo
    pkgs.atk
  ] ++ lib.optionals pkgs.stdenv.isDarwin [
    pkgs.libiconv
    pkgs.darwin.apple_sdk.frameworks.WebKit
    pkgs.darwin.apple_sdk.frameworks.Cocoa
    pkgs.darwin.apple_sdk.frameworks.CoreServices
  ];

  # Expose pnpm via corepack without writing to the read-only Nix store.
  # The packageManager field in apps/desktop/package.json pins the version.
  scripts.pnpm.exec = ''
    exec corepack pnpm "$@"
  '';

  env = {
    RUST_BACKTRACE = "1";
    CARGO_NET_OFFLINE = "false";
    WEBKIT_DISABLE_COMPOSITING_MODE = "1";
    WEBKIT_FORCE_SOFTWARE_RENDERING = "1";
  };

  enterShell = ''
    echo "Byte Agent dev shell ready (system: ${pkgs.stdenv.hostPlatform.system})"
  '';
}
