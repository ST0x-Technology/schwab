{
  description = "Flake for development workflows.";

  inputs = {
    rainix.url = "github:rainprotocol/rainix";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks-nix.url = "github:cachix/git-hooks.nix";
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems =
        [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];

      perSystem = { config, pkgs, system, ... }:
        let
          rainixPkgs = inputs.rainix.pkgs.${system};
          rainixPackages = inputs.rainix.packages.${system};
        in {
          packages = rainixPackages // {
            prepSolArtifacts = inputs.rainix.mkTask.${system} {
              name = "prep-sol-artifacts";
              additionalBuildInputs = inputs.rainix.sol-build-inputs.${system};
              body = ''
                set -euxo pipefail
                (cd lib/rain.orderbook.interface/ && forge build)
                (cd lib/forge-std/ && forge build)
              '';
            };

            checkTestCoverage = inputs.rainix.mkTask.${system} {
              name = "check-test-coverage";
              additionalBuildInputs = [ rainixPkgs.cargo-tarpaulin ];
              body = ''
                set -euxo pipefail
                cargo-tarpaulin --skip-clean --out Html
              '';
            };
          };

          checks = {
            pre-commit-check = inputs.git-hooks-nix.lib.${system}.run {
              src = ./.;
              hooks = {
                # Nix
                nil.enable = true;
                nixfmt-classic.enable = true;
                deadnix.enable = true;
                statix.enable = true;
                statix.settings.ignore = [ "lib/" ];

                # Rust
                taplo.enable = true;
                rustfmt.enable = true;

                # Markdown
                denofmt.enable = true;
                yamlfmt.enable = true;
                yamlfmt.settings.lint-only = false;
              };
            };
          };

          devShells.default = rainixPkgs.mkShell {
            inherit (config.checks.pre-commit-check) shellHook;
            inherit (inputs.rainix.devShells.${system}.default)
              nativeBuildInputs;
            buildInputs = with rainixPkgs;
              [
                sqlx-cli
                cargo-tarpaulin
                config.packages.prepSolArtifacts
                config.packages.checkTestCoverage
              ] ++ inputs.rainix.devShells.${system}.default.buildInputs;
          };
        };
    };
}
