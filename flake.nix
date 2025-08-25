{
  description = "Flake for development workflows.";

  inputs = {
    rainix.url = "github:rainprotocol/rainix";
    nixpkgs.follows = "rainix/nixpkgs";

    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks-nix.url = "github:cachix/git-hooks.nix";

    # Separate nixpkgs for process-compose to avoid Go 1.23.9/1.24.3+ dlopen regression
    nixpkgs-for-process-compose.url =
      "github:NixOS/nixpkgs/nixos-24.11"; # Go 1.23.8
    process-compose-flake.url = "github:Platonic-Systems/process-compose-flake";
    services-flake.url = "github:juspay/services-flake";
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [ inputs.process-compose-flake.flakeModule ];

      systems =
        [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];

      perSystem = { config, pkgs, system, ... }:
        let
          rainixPkgs = inputs.rainix.pkgs.${system};
          rainixPackages = inputs.rainix.packages.${system};

          # Import older nixpkgs to build process-compose without Go regression
          oldPkgs =
            import inputs.nixpkgs-for-process-compose { inherit system; };
        in {
          process-compose."services" = {
            # Override process-compose package to use one built with Go 1.21.4
            package = oldPkgs.process-compose;

            imports = [ inputs.services-flake.processComposeModules.default ];

            settings.processes = {
              # Placeholder process to ensure process-compose works
              placeholder = {
                command = "echo 'Process compose is ready for services'";
                availability.restart = "no";
              };
            };

            # Services-flake configuration will be added here
            # Currently empty but ready for PostgreSQL and other services
            services = {
              # PostgreSQL and other services will be configured here
            };
          };

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
                config.packages.services
              ] ++ inputs.rainix.devShells.${system}.default.buildInputs;
          };
        };
    };
}
