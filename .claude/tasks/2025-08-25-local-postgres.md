# Local PostgreSQL Development Setup

## Overview

This plan outlines setting up a local PostgreSQL development environment using
services-flake integrated through flake-parts, without migrating the application
from SQLite. This creates the foundation for future PostgreSQL migration while
keeping the existing SQLite implementation intact.

## Phase 1: Nix Infrastructure Setup

### Task 1: Migrate to flake-parts Architecture

**Objective**: Refactor the existing flake.nix to use flake-parts for better
modularity and extensibility.

**Subtasks**:

- [x] Add flake-parts input to flake.nix
- [x] Restructure flake outputs to use flake-parts modules
- [x] Make existing packages, devShell, and checks work with flake-parts
- [x] Preserve all existing functionality (rainix integration, git-hooks, sol
      artifacts)
- [x] Test that all existing `nix develop`, `nix run` commands still work
- [x] Verify pre-commit hooks pass

**Completion Details**:

- Successfully migrated flake.nix from flake-utils to flake-parts architecture
- Replaced `flake-utils.lib.eachDefaultSystem` with `flake-parts.lib.mkFlake`
- Restructured outputs using `perSystem` pattern
- All existing functionality preserved:
  - `nix develop` works with pre-commit hooks
  - `nix run .#prepSolArtifacts` and `nix run .#checkTestCoverage` work
  - All rainix integration maintained (packages, devShell, build inputs)
- Pre-commit hooks pass successfully after fixing unused lambda parameters
- Flake check passes for all system architectures

**Design Decisions**:

- Use flake-parts to make the flake compatible with services-flake
- Maintain backward compatibility with existing development workflow

### Task 2: Add process-compose-flake Integration

**Objective**: Integrate process-compose-flake to manage local development
services.

**Problem Encountered During Implementation**:

- Process-compose fails to build on macOS (Darwin) with error:
  `link: duplicated definition of symbol dlopen, from github.com/ebitengine/purego`
- This is a Go linker regression introduced in Go 1.23.9 and 1.24.3
- Our current nixpkgs (via rainix) has Go 1.24.5 which still has this regression
- Process-compose-flake doesn't accept nixpkgs as a direct input - it gets pkgs
  from the perSystem context in flake-parts

**Solution**:

- Add a separate nixpkgs input pinned to nixos-24.11 with Go 1.23.8 (before the
  regression)
- Use the `package` option in process-compose configuration to override with
  process-compose built from older nixpkgs
- This isolates the fix to only process-compose, keeping all other dependencies
  on current nixpkgs

**Subtasks**:

- [x] Add nixpkgs-for-process-compose input pinned to nixos-24.11 (Go 1.23.8)
- [x] Add process-compose-flake input to flake inputs
- [x] Import process-compose-flake module in flake-parts configuration
- [x] Configure process-compose with package override to use older nixpkgs
- [x] Test process-compose commands work (`nix run .#services`)
- [x] Add process-compose generated package to the dev shell

**Implementation Details**:

1. Add separate nixpkgs input in flake.nix:

```nix
nixpkgs-for-process-compose.url = "github:NixOS/nixpkgs/nixos-24.11";
process-compose-flake.url = "github:Platonic-Systems/process-compose-flake";
```

2. In perSystem, build process-compose from older nixpkgs:

```nix
perSystem = { config, pkgs, system, ... }:
  let
    oldPkgs = import inputs.nixpkgs-for-process-compose { inherit system; };
  in {
    process-compose."services" = {
      package = oldPkgs.process-compose;  # Override to use Go 1.23.8 build
      settings.processes.placeholder = {
        command = "echo 'Process compose is ready for services'";
        availability.restart = "no";
      };
    };
  };
```

3. Add to dev shell: `config.packages.services`

**Design Decisions**:

- Use process-compose for local service orchestration
- Override only the process-compose package to avoid Go regression
- Maintain separate nixpkgs for build isolation
- Foundation ready for services-flake integration

### Task 3: Add services-flake Integration

**Objective**: Add services-flake for declarative service management.

**Subtasks**:

- [x] Add services-flake input to flake inputs
- [x] Import services-flake module in flake-parts configuration
- [x] Configure services-flake to work with process-compose-flake

**Completion Details**:

- Successfully added services-flake input to flake.nix
- Integrated services-flake.processComposeModules.default within the
  process-compose configuration
- Configured services block within process-compose."services" ready for
  PostgreSQL and other services
- All flake checks pass and pre-commit hooks are satisfied
- Services command is available via `nix run .#services`

**Design Decisions**:

- Use services-flake for declarative service definitions
- Import services-flake module within process-compose configuration, not at top
  level
- Integrate with process-compose for process management
- Foundation ready for PostgreSQL service configuration in Task 4

## Phase 2: PostgreSQL Service Setup

### Task 4: Configure PostgreSQL Service

**Objective**: Set up local PostgreSQL using services-flake.

**Subtasks**:

- [x] Add PostgreSQL service configuration in services-flake
- [x] Set up data directory in project root (.postgres-data/ with gitignore)
- [x] Add PostgreSQL client tools to development shell
- [x] Test PostgreSQL starts/stops correctly via nix commands
- [x] Verify database connectivity using psql

**Completion Details**:

- Successfully configured PostgreSQL service using services-flake in flake.nix
- PostgreSQL service creates and manages local database instance on port 5432
- Data directory `.postgres-data/` is created locally and added to .gitignore
- PostgreSQL client tools (psql, etc.) added to development shell via
  `postgresql` package
- Service initializes correctly with `schwab` database created automatically
- Database connectivity tested successfully using psql with basic CRUD
  operations
- Removed placeholder process from process-compose configuration
- PostgreSQL 17.5 is running and ready for application integration

**Implementation Details**:

1. Added PostgreSQL service configuration in flake.nix:

```nix
services = {
  postgres."schwab-db" = {
    enable = true;
    port = 5432;
    dataDir = "./.postgres-data";
    initialDatabases = [{ name = "schwab"; }];
  };
};
```

2. Added PostgreSQL client tools to devShell:

```nix
buildInputs = with rainixPkgs; [
  sqlx-cli
  cargo-tarpaulin
  postgresql  # Added for psql and other PostgreSQL client tools
  # ... other tools
];
```

3. Added `.postgres-data/` to .gitignore to prevent committing database files

4. Verified service functionality:
   - Database initializes on first run with initdb
   - Creates `schwab` database automatically
   - Accepts connections on localhost:5432
   - Basic SQL operations work correctly
   - Clean shutdown when process-compose stops

**Design Decisions**:

- Use default PostgreSQL settings suitable for development
- Store data locally in project directory (gitignored)
- Use standard port 5432 (no conflicts detected)
- Include psql and other tools in dev environment
- Use services-flake PostgreSQL module for declarative configuration
- Automatic database creation eliminates manual setup steps

### Task 5: Document PostgreSQL Development Environment

**Objective**: Document how to use the local PostgreSQL for development.

**Subtasks**:

- [ ] Document how to start PostgreSQL: `nix run .#services`
- [ ] Document connection details: `postgresql://localhost:5432/schwab`
- [ ] Document available psql commands for database inspection
- [ ] Add troubleshooting guide for common PostgreSQL service issues

**Design Decisions**:

- Keep documentation focused on local development setup only
- Do not modify application code or configuration
- Provide clear instructions for developers who want to experiment with
  PostgreSQL

## Success Criteria

- [ ] Local PostgreSQL development environment fully functional
- [ ] All existing tests pass with PostgreSQL
- [ ] No data precision loss in financial calculations
- [ ] Development workflow remains smooth and fast
- [ ] Documentation is complete and accurate
- [ ] Application performance is maintained or improved
