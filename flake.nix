{
  description = "Flake for development workflows.";

  inputs = {
    rainix.url = "github:rainprotocol/rainix";
    flake-utils.url = "github:numtide/flake-utils";
    git-hooks-nix.url = "github:cachix/git-hooks.nix";
  };

  outputs = { self, flake-utils, rainix, git-hooks-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = rainix.pkgs.${system};

      in rec {
        checks = {
          pre-commit-check = git-hooks-nix.lib.${system}.run {
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

        packages = let rainixPkgs = rainix.packages.${system};
        in rainixPkgs // {
          prepSolArtifacts = rainix.mkTask.${system} {
            name = "prep-sol-artifacts";
            additionalBuildInputs = rainix.sol-build-inputs.${system};
            body = ''
              set -euxo pipefail
              (cd lib/rain.orderbook.interface/ && forge build)
              (cd lib/forge-std/ && forge build)
            '';
          };

          checkTestCoverage = rainix.mkTask.${system} {
            name = "check-test-coverage";
            additionalBuildInputs = [ pkgs.cargo-tarpaulin ];
            body = ''
              set -euxo pipefail
              cargo-tarpaulin --skip-clean --out Html
            '';
          };
        };

        devShell = pkgs.mkShell {
          inherit (self.checks.${system}.pre-commit-check) shellHook;
          inherit (rainix.devShells.${system}.default) nativeBuildInputs;
          buildInputs = with pkgs;
            [
              sqlx-cli
              cargo-tarpaulin
              packages.prepSolArtifacts
              packages.checkTestCoverage
            ] ++ rainix.devShells.${system}.default.buildInputs;
        };
      });
}
