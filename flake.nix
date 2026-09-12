{
  description = "Fast fuzzy and marked jumps for Zellij sessions, tabs, and panes.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {nixpkgs, ...}: let
    systems = [
      "aarch64-darwin"
      "x86_64-linux"
    ];
    forAllSystems = nixpkgs.lib.genAttrs systems;
  in {
    packages = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
      wasmCross = pkgs.pkgsCross.wasm32-wasip1;
    in {
      default = wasmCross.rustPlatform.buildRustPackage {
        pname = "zjump";
        version = "0.1.0";
        src = pkgs.lib.cleanSource ./.;
        cargoLock.lockFile = ./Cargo.lock;

        nativeBuildInputs = [wasmCross.lld];
        env.RUSTFLAGS = "-C linker=wasm-ld";

        buildPhase = ''
          runHook preBuild
          cargo build --release --target wasm32-wasip1
          runHook postBuild
        '';

        installPhase = ''
          runHook preInstall
          install -Dm0644 target/wasm32-wasip1/release/zjump.wasm $out/bin/zjump.wasm
          runHook postInstall
        '';
      };
    });

    devShells = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
      wasmCross = pkgs.pkgsCross.wasm32-wasip1;
    in {
      default = pkgs.mkShell {
        packages = [
          wasmCross.buildPackages.rustc
          wasmCross.buildPackages.cargo
          wasmCross.lld
          pkgs.pkg-config
          pkgs.rustfmt
        ];
        buildInputs = [pkgs.openssl pkgs.curl];
        env.CARGO_TARGET_WASM32_WASIP1_LINKER = "wasm-ld";
      };
    });

    formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.alejandra);
  };
}
