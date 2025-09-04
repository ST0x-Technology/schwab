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

        packages = let
          rainixPkgs = rainix.packages.${system};

          # Common database path resolution - handles missing .env, missing DATABASE_URL, and non-export declarations
          setDbPath = ''
            if [ -f .env ]; then
              source .env 2>/dev/null || true
            fi
            DB_PATH=$(echo "''${DATABASE_URL:-sqlite:schwab.db}" | sed 's/sqlite://')
          '';
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

          # Database helper commands (read-only)

          viewAccumulators = rainix.mkTask.${system} {
            name = "view-accumulators";
            additionalBuildInputs = [ pkgs.sqlite ];
            body = ''
              set -euxo pipefail
              ${setDbPath}
              echo "=== Current Accumulator Positions ==="
              sqlite3 "$DB_PATH" "
                SELECT 
                  symbol,
                  printf('%.6f', accumulated_long - accumulated_short) as net_position,
                  printf('%.6f', accumulated_long) as accumulated_long,
                  printf('%.6f', accumulated_short) as accumulated_short,
                  pending_execution_id,
                  last_updated
                FROM trade_accumulators
                ORDER BY symbol;
              "
            '';
          };

          viewTrades = rainix.mkTask.${system} {
            name = "view-trades";
            additionalBuildInputs = [ pkgs.sqlite ];
            body = ''
              set -euxo pipefail
              ${setDbPath}
              echo "=== Recent Onchain Trades (Last 10) ==="
              sqlite3 "$DB_PATH" "
                SELECT 
                  id,
                  symbol,
                  printf('%.6f', amount) as amount,
                  direction,
                  substr(tx_hash, 1, 10) || '...' as tx_hash,
                  log_index,
                  created_at
                FROM onchain_trades
                ORDER BY id DESC
                LIMIT 10;
              "
            '';
          };

          viewExecutions = rainix.mkTask.${system} {
            name = "view-executions";
            additionalBuildInputs = [ pkgs.sqlite ];
            body = ''
              set -euxo pipefail
              ${setDbPath}
              echo "=== Schwab Executions ==="
              sqlite3 "$DB_PATH" "
                SELECT 
                  id,
                  symbol,
                  shares,
                  direction,
                  order_id,
                  price_cents,
                  status,
                  executed_at
                FROM schwab_executions
                ORDER BY id DESC;
              "
            '';
          };

          viewEventQueue = rainix.mkTask.${system} {
            name = "view-event-queue";
            additionalBuildInputs = [ pkgs.sqlite ];
            body = ''
              set -euxo pipefail
              ${setDbPath}
              echo "=== Event Queue Status ==="
              echo "Unprocessed events:"
              sqlite3 "$DB_PATH" "
                SELECT COUNT(*) as unprocessed_count
                FROM event_queue 
                WHERE processed = 0;
              "
              echo ""
              echo "Recent events (last 5):"
              sqlite3 "$DB_PATH" "
                SELECT 
                  id,
                  substr(tx_hash, 1, 10) || '...' as tx_hash,
                  log_index,
                  block_number,
                  processed,
                  created_at
                FROM event_queue
                ORDER BY id DESC
                LIMIT 5;
              "
            '';
          };

          viewDatabaseState = rainix.mkTask.${system} {
            name = "view-database-state";
            additionalBuildInputs = [ pkgs.sqlite ];
            body = ''
              set -euxo pipefail
              ${setDbPath}
              echo "=== Complete Database State Overview ==="
              echo ""
              echo "Trade counts by symbol:"
              sqlite3 "$DB_PATH" "
                SELECT 
                  symbol,
                  COUNT(*) as trade_count,
                  SUM(CASE WHEN direction = 'Buy' THEN 1 ELSE 0 END) as buy_count,
                  SUM(CASE WHEN direction = 'Sell' THEN 1 ELSE 0 END) as sell_count
                FROM onchain_trades 
                GROUP BY symbol
                ORDER BY symbol;
              "
              echo ""
              echo "Execution summary:"
              sqlite3 "$DB_PATH" "
                SELECT 
                  symbol,
                  COUNT(*) as execution_count,
                  SUM(CASE WHEN direction = 'Buy' THEN shares ELSE 0 END) as total_buy_shares,
                  SUM(CASE WHEN direction = 'Sell' THEN shares ELSE 0 END) as total_sell_shares
                FROM schwab_executions 
                GROUP BY symbol
                ORDER BY symbol;
              "
              echo ""
              echo "Current accumulator positions:"
              sqlite3 "$DB_PATH" "
                SELECT 
                  symbol,
                  printf('%.6f', accumulated_long - accumulated_short) as net_position,
                  printf('%.6f', accumulated_long) as accumulated_long,
                  printf('%.6f', accumulated_short) as accumulated_short,
                  CASE 
                    WHEN abs(accumulated_long - accumulated_short) >= 1.0 THEN 'READY FOR EXECUTION'
                    ELSE 'Below threshold'
                  END as execution_status
                FROM trade_accumulators
                ORDER BY symbol;
              "
              echo ""
              echo "Event queue status:"
              sqlite3 "$DB_PATH" "
                SELECT 
                  COUNT(*) as total_events,
                  SUM(CASE WHEN processed = 1 THEN 1 ELSE 0 END) as processed_events,
                  SUM(CASE WHEN processed = 0 THEN 1 ELSE 0 END) as unprocessed_events
                FROM event_queue;
              "
            '';
          };
        };

        devShell = pkgs.mkShell {
          inherit (self.checks.${system}.pre-commit-check) shellHook;
          inherit (rainix.devShells.${system}.default) nativeBuildInputs;
          buildInputs = with pkgs;
            [
              doctl
              sqlx-cli
              cargo-tarpaulin
              cargo-chef
              packages.prepSolArtifacts
              packages.checkTestCoverage
            ] ++ rainix.devShells.${system}.default.buildInputs ++ [
              packages.viewAccumulators
              packages.viewTrades
              packages.viewExecutions
              packages.viewEventQueue
              packages.viewDatabaseState
            ];
        };
      });
}
