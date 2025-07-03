{
  description = "Flake for development workflows.";

  inputs = {
    rainix.url = "github:rainprotocol/rainix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, flake-utils, rainix }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = rainix.pkgs.${system};
      in rec {
        packages = let rainixPkgs = rainix.packages.${system};
        in rainixPkgs // {
          prepSolArtifacts = rainix.mkTask.${system} {
            name = "prep-sol-artifacts";
            additionalBuildInputs = rainix.sol-build-inputs.${system};
            body = ''
              set -euxo pipefail
              (cd lib/rain.orderbook.interface/ && rainix-sol-prelude)
              (cd lib/forge-std/ && rainix-sol-prelude)
            '';
          };

          checkTestCoverage = rainix.mkTask.${system} {
            name = "check-test-coverage";
            additionalBuildInputs = [ pkgs.cargo-tarpaulin ];
            body = ''
              cargo-tarpaulin --skip-clean --exclude-files lib/* --out Html
            '';
          };
        };

        devShell = pkgs.mkShell {
          shellHook = rainix.devShells.${system}.default.shellHook;
          buildInputs = with pkgs;
            [
              sqlx-cli
              bacon
              cargo-tarpaulin
              packages.prepSolArtifacts
              packages.checkTestCoverage
            ] ++ rainix.devShells.${system}.default.buildInputs;
          nativeBuildInputs =
            rainix.devShells.${system}.default.nativeBuildInputs;
        };
      });
}
