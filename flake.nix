{
  description = "Byte Agent development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f {
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        inherit system;
      });
    in
    {
      devShells = forAllSystems ({ pkgs, system }: {
        default = pkgs.mkShell {
          name = "byte-agent";

          packages = with pkgs; [
            # Rust toolchain
            (rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rustfmt" "clippy" ];
            })

            # Node.js runtime and package manager
            nodejs_22
            corepack

            # Task runner and helpers
            just
            ruby
            git
            jujutsu

            # Tauri Linux system dependencies
            pkg-config
            openssl
            openssl.dev
            glib
            gtk3
            libsoup_3
            webkitgtk_4_1
            libappindicator-gtk3
            librsvg
            patchelf
            gdk-pixbuf
            dbus
            pango
            cairo
            atk
          ] ++ lib.optionals stdenv.isDarwin [
            libiconv
            darwin.apple_sdk.frameworks.WebKit
            darwin.apple_sdk.frameworks.Cocoa
            darwin.apple_sdk.frameworks.CoreServices
          ];

          shellHook = ''
            export RUST_BACKTRACE=1
            export CARGO_NET_OFFLINE=false

            # WebKitGTK on non-NixOS hosts cannot find the host GPU drivers under
            # /run/opengl-driver; fall back to software rendering so the dev shell works
            # across distributions without GPU-specific setup.
            export WEBKIT_DISABLE_COMPOSITING_MODE=1
            export WEBKIT_FORCE_SOFTWARE_RENDERING=1

            # Ensure pnpm is available at the exact version declared in apps/desktop/package.json.
            corepack prepare pnpm@10.33.0 --activate > /dev/null 2>&1 || true

            echo "Byte Agent dev shell ready (system: ${system})"
          '';
        };
      });
    };
}
