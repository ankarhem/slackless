{
  description = "Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    fenix.inputs.nixpkgs.follows = "nixpkgs";
    fenix.url = "github:nix-community/fenix";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks.inputs.nixpkgs.follows = "nixpkgs";
    git-hooks.url = "github:cachix/git-hooks.nix";
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs =
    inputs@{ self, ... }:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.git-hooks.flakeModule
        inputs.treefmt-nix.flakeModule
      ];
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      flake.overlays.fenix = inputs.fenix.overlays.default;
      perSystem =
        {
          config,
          lib,
          pkgs,
          system,
          ...
        }:
        let
          toolchain = inputs.fenix.packages.${system}.complete.toolchain;
        in
        {
          treefmt = {
            programs.nixfmt.enable = true;
            programs.nixfmt.package = pkgs.nixfmt;
            programs.rustfmt.enable = true;
            programs.prettier.enable = true;
          };
          pre-commit.settings.hooks = {
            treefmt.enable = true;
          };
          devShells.default = pkgs.mkShell {
            inherit (config.pre-commit) shellHook;
            packages =
              with pkgs;
              [
                toolchain
                rust-analyzer-nightly
                cargo
                cargo-deny
                cargo-edit
                cargo-udeps
                gdb
                minicom
                usbutils
                cmake
                just
                pkg-config
              ]
              ++ config.pre-commit.settings.enabledPackages;

            env = {
              RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
              # Suppress benign cc-rs (`arm64-apple-macosx`) vs Nix (`arm64-apple-darwin`) target warning.
              NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING = "1";
            };
          };
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = lib.attrValues self.overlays;
          };
        };
    };
}
