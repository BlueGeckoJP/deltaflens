{
  description = "Development environment for deltaflens";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    nixgl = {
      url = "github:nix-community/nixGL";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      nixgl,
      ...
    }:
    let
      system = "x86_64-linux";

      pkgs = import nixpkgs {
        inherit system;
        overlays = [ nixgl.overlay ];
      };

      libraries = with pkgs; [
        dbus
        expat
        fontconfig
        freetype
        libGL
        libxkbcommon
        openssl
        vulkan-loader
        wayland
        wayland-protocols
        zlib

        libx11
        libxcursor
        libxi
        libxrandr
        libxcb
      ];

      runApp = pkgs.writeShellApplication {
        name = "deltaflens";

        runtimeInputs = [ pkgs.nix ];

        text = ''
          exec nix develop "path:${self.outPath}" --command nixVulkanIntel cargo run -- "$@"
        '';
      };

      releaseRunApp = pkgs.writeShellApplication {
        name = "deltaflens-release";

        runtimeInputs = [ pkgs.nix ];

        text = ''
          exec nix develop "path:${self.outPath}" --command nixVulkanIntel cargo run --release -- "$@"
        '';
      };

      buildApp = pkgs.writeShellApplication {
        name = "deltaflens-build";

        runtimeInputs = [ pkgs.nix ];

        text = ''
          exec nix develop "path:${self.outPath}" --command cargo build "$@"
        '';
      };

      releaseBuildApp = pkgs.writeShellApplication {
        name = "deltaflens-build-release";

        runtimeInputs = [ pkgs.nix ];

        text = ''
          exec nix develop "path:${self.outPath}" --command cargo build --release "$@"
        '';
      };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          clippy
          rust-analyzer
          rustc
          rustfmt

          pkgs.nixgl.nixGLIntel
          pkgs.nixgl.nixVulkanIntel
        ];

        nativeBuildInputs = with pkgs; [
          cmake
          pkg-config
          rustPlatform.bindgenHook
        ];

        buildInputs = libraries;

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libraries;

        VK_LOADER_LAYERS_DISABLE = "VK_LAYER_MESA_device_select";

        RUST_BACKTRACE = "1";

        shellHook = ''
          echo "Welcome to deltaflens development environment"
        '';
      };

      apps.${system} = {
        default = {
          type = "app";
          program = "${releaseRunApp}/bin/deltaflens-release";
        };

        run = {
          type = "app";
          program = "${runApp}/bin/deltaflens";
        };

        "run-release" = {
          type = "app";
          program = "${releaseRunApp}/bin/deltaflens-release";
        };

        build = {
          type = "app";
          program = "${buildApp}/bin/deltaflens-build";
        };

        "build-release" = {
          type = "app";
          program = "${releaseBuildApp}/bin/deltaflens-build-release";
        };
      };
    };
}
