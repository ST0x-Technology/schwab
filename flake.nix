{
  description = "Flake for development workflows.";

  inputs = {
    rainix.url = "github:rainprotocol/rainix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, flake-utils, rainix }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = rainix.pkgs.${system};
      in {
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
        };

        devShell = pkgs.mkShell {
          shellHook = rainix.devShells.${system}.default.shellHook;
          buildInputs = [ pkgs.sqlx-cli ]
            ++ rainix.devShells.${system}.default.buildInputs;
          nativeBuildInputs =
            rainix.devShells.${system}.default.nativeBuildInputs;
        };
      });
}
