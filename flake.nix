{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };
  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };
          rustToolchain = pkgs.rust-bin.stable.latest.default;
          nativeBuildInputs = with pkgs; [ rustToolchain pkg-config cmake ];
          buildInputs = with pkgs; [
            alsa-lib
            libopus
            xorg.libX11
            xorg.libXext
            xorg.libXinerama
            xorg.libXcursor
            xorg.libXrender
            xorg.libXfixes
            xorg.libXft
            pango
          ];
        in
        with pkgs;
        {
          devShells.default = mkShell {
            inherit nativeBuildInputs buildInputs;
          };
        }
      );
}
