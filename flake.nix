{
  nixConfig.extra-substituters = [
    "https://bytecodealliance.cachix.org"
    "https://wasmcloud.cachix.org"
    "https://nixify.cachix.org"
    "https://crane.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-substituters = [
    "https://bytecodealliance.cachix.org"
    "https://wasmcloud.cachix.org"
    "https://nixify.cachix.org"
    "https://crane.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-public-keys = [
    "bytecodealliance.cachix.org-1:0SBgh//n2n0heh0sDFhTm+ZKBRy2sInakzFGfzN531Y="
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nixify.cachix.org-1:95SiUQuf8Ij0hwDweALJsLtnMyv/otZamWNRp1Q1pXw="
    "crane.cachix.org-1:8Scfpmn9w+hGdXH/Q9tTLiYAE/2dnJYRJP7kl80GuRk="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
  ];

  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = "github:rvolosatovs/nixify/feat/25.05";
  inputs.nixlib.url = "github:nix-community/nixpkgs.lib";
  inputs.nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.wit-deps.inputs.nixify.follows = "nixify";
  inputs.wit-deps.inputs.nixlib.follows = "nixlib";
  inputs.wit-deps.url = "github:bytecodealliance/wit-deps/v0.5.0";

  outputs = {
    nixify,
    nixlib,
    nixpkgs-unstable,
    wit-deps,
    ...
  }:
    with builtins;
    with nixlib.lib;
    with nixify.lib;
      rust.mkFlake {
        name = "wasmlet";
        src = ./.;

        overlays = [
          wit-deps.overlays.default
          (
            final: prev: {
              pkgsUnstable = import nixpkgs-unstable {
                inherit
                  (final.stdenv.hostPlatform)
                  system
                  ;

                inherit
                  (final)
                  config
                  ;
              };
            }
          )
        ];

        excludePaths = [
          ".envrc"
          ".github"
          ".gitignore"
          "ADOPTERS.md"
          "CODE_OF_CONDUCT.md"
          "CONTRIBUTING.md"
          "flake.nix"
          "LICENSE"
          "README.md"
          "SECURITY.md"
          "SPEC.md"
        ];

        doCheck = false; # testing is performed in checks via `nextest`

        targets.arm-unknown-linux-gnueabihf = false;
        targets.arm-unknown-linux-musleabihf = false;
        targets.armv7-unknown-linux-gnueabihf = false;
        targets.armv7-unknown-linux-musleabihf = false;
        targets.powerpc64le-unknown-linux-gnu = false;
        targets.s390x-unknown-linux-gnu = false;
        targets.wasm32-unknown-unknown = false;
        targets.wasm32-wasip1 = false;
        targets.wasm32-wasip2 = false;

        clippy.deny = ["warnings"];
        clippy.workspace = true;

        test.allTargets = true;
        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        }: {
          buildInputs ? [],
          depsBuildBuild ? [],
          ...
        } @ args:
          with pkgs.lib; {
            depsBuildBuild =
              depsBuildBuild
              ++ optional pkgs.stdenv.hostPlatform.isDarwin pkgs.darwin.apple_sdk.frameworks.SystemConfiguration;
          };

        withPackages = {
          hostRustToolchain,
          packages,
          pkgs,
          ...
        }: let
          interpreters.aarch64-unknown-linux-gnu = "/lib/ld-linux-aarch64.so.1";
          interpreters.riscv64gc-unknown-linux-gnu = "/lib/ld-linux-riscv64-lp64d.so.1";
          interpreters.x86_64-unknown-linux-gnu = "/lib64/ld-linux-x86-64.so.2";

          mkFHS = {
            name,
            src,
            interpreter,
          }:
            pkgs.stdenv.mkDerivation {
              inherit
                name
                src
                ;

              buildInputs = [
                pkgs.patchelf
              ];

              dontBuild = true;
              dontFixup = true;

              installPhase = ''
                runHook preInstall

                for p in $(find . -type f); do
                  # https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
                  if head -c 4 $p | grep $'\x7FELF' > /dev/null; then
                    patchelf --set-rpath /lib $p || true
                    patchelf --set-interpreter ${interpreter} $p || true
                  fi
                done

                mkdir -p $out
                cp -R * $out

                runHook postInstall
              '';
            };

          wasmlet-aarch64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmlet-aarch64-unknown-linux-gnu-fhs";
            src = packages.wasmlet-aarch64-unknown-linux-gnu;
            interpreter = interpreters.aarch64-unknown-linux-gnu;
          };

          wasmlet-riscv64gc-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmlet-riscv64gc-unknown-linux-gnu-fhs";
            src = packages.wasmlet-riscv64gc-unknown-linux-gnu;
            interpreter = interpreters.riscv64gc-unknown-linux-gnu;
          };

          wasmlet-x86_64-unknown-linux-gnu-fhs = mkFHS {
            name = "wasmlet-x86_64-unknown-linux-gnu-fhs";
            src = packages.wasmlet-x86_64-unknown-linux-gnu;
            interpreter = interpreters.x86_64-unknown-linux-gnu;
          };
        in
          packages
          // {
            inherit
              wasmlet-aarch64-unknown-linux-gnu-fhs
              wasmlet-riscv64gc-unknown-linux-gnu-fhs
              wasmlet-x86_64-unknown-linux-gnu-fhs
              ;

            rust = hostRustToolchain;
          };

        withDevShells = {
          devShells,
          pkgs,
          ...
        }:
          extendDerivations {
            buildInputs = [
              pkgs.cargo-audit
              pkgs.cargo-nextest
              pkgs.curl
              pkgs.websocat
              pkgs.wit-deps

              pkgs.pkgsUnstable.go
              pkgs.pkgsUnstable.kubectl
              pkgs.pkgsUnstable.minikube
            ];
          }
          devShells;
      };
}
