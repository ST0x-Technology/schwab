# SQLite to Postgres Migration Plan

## Overview

This plan outlines the complete migration from SQLite to PostgreSQL for the
Schwab arbitrage bot, including setting up a local development environment using
services-flake integrated through flake-parts.

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

**Subtasks**:

- [ ] Add process-compose-flake input to flake inputs
- [ ] Import process-compose-flake module in flake-parts configuration
- [ ] Configure basic process-compose setup without services yet
- [ ] Test process-compose commands work (`nix run .#services`)
- [ ] Add process-compose generated package to the dev shell

**Design Decisions**:

- Use process-compose for local service orchestration
- Prepare foundation for services-flake integration

### Task 3: Add services-flake Integration

**Objective**: Add services-flake for declarative service management.

**Subtasks**:

- [ ] Add services-flake input to flake inputs
- [ ] Import services-flake module in flake-parts configuration
- [ ] Configure services-flake to work with process-compose-flake
- [ ] Test basic services infrastructure without PostgreSQL
- [ ] Ensure services can be started/stopped via nix commands

**Design Decisions**:

- Use services-flake for declarative service definitions
- Integrate with process-compose for process management
- Enable easy service addition/removal in the future

## Phase 2: PostgreSQL Service Setup

### Task 4: Configure PostgreSQL Service

**Objective**: Set up local PostgreSQL using services-flake.

**Subtasks**:

- [ ] Add PostgreSQL service configuration in services-flake
- [ ] Configure database name, user, and basic security settings
- [ ] Set up data directory in project root (.postgres-data/ with gitignore)
- [ ] Configure PostgreSQL port (default 5432 or custom if needed)
- [ ] Add PostgreSQL client tools to development shell
- [ ] Test PostgreSQL starts/stops correctly via nix commands
- [ ] Verify database connectivity using psql

**Design Decisions**:

- Use default PostgreSQL settings suitable for development
- Store data locally in project directory (gitignored)
- Use standard port unless conflicts exist
- Include psql and other tools in dev environment

### Task 5: Database Connection Configuration

**Objective**: Configure application to connect to local PostgreSQL.

**Subtasks**:

- [ ] Update DATABASE_URL format for PostgreSQL in .env.example
- [ ] Document new environment variable format in README or CLAUDE.md
- [ ] Add environment variable validation for PostgreSQL URLs
- [ ] Test connection pooling works with PostgreSQL
- [ ] Verify SSL configuration is appropriate for local development

**Design Decisions**:

- Use standard PostgreSQL connection string format
- Maintain environment variable-based configuration
- Default to local development settings
- Ensure production deployment flexibility

## Phase 3: Database Schema Migration

### Task 6: Convert SQLite Schema to PostgreSQL

**Objective**: Adapt the database schema for PostgreSQL-specific features and
data types.

**Subtasks**:

- [ ] Create new migration file for PostgreSQL schema
- [ ] Convert SQLite-specific data types to PostgreSQL equivalents:
  - INTEGER PRIMARY KEY AUTOINCREMENT → SERIAL PRIMARY KEY
  - DATETIME → TIMESTAMP WITH TIME ZONE
  - REAL → NUMERIC or DOUBLE PRECISION (evaluate precision needs)
  - TEXT → VARCHAR or TEXT (evaluate length constraints)
- [ ] Update CHECK constraints to use PostgreSQL syntax
- [ ] Review and optimize indexes for PostgreSQL
- [ ] Test migration runs successfully on fresh PostgreSQL database
- [ ] Validate all constraints and relationships work correctly

**Design Decisions**:

- Use PostgreSQL native data types for better performance and precision
- Choose NUMERIC over REAL for financial calculations to avoid precision loss
- Use TIMESTAMP WITH TIME ZONE for proper timezone handling
- Maintain existing constraint logic while optimizing for PostgreSQL

### Task 7: Update Application Code for PostgreSQL

**Objective**: Modify Rust code to work with PostgreSQL instead of SQLite.

**Subtasks**:

- [ ] Update Cargo.toml dependencies:
  - Change sqlx features from "sqlite" to "postgres"
  - Keep existing features (chrono, runtime-tokio-rustls)
- [ ] Update database connection code in src/lib.rs or database module
- [ ] Review and update any SQLite-specific SQL queries
- [ ] Update any AUTOINCREMENT logic to work with SERIAL
- [ ] Test all database operations work with PostgreSQL
- [ ] Verify connection pooling and transaction handling

**Design Decisions**:

- Use sqlx PostgreSQL driver for consistency with existing code
- Maintain existing database abstraction patterns
- Preserve all existing functionality while optimizing for PostgreSQL
- Use PostgreSQL-specific features where beneficial (better JSON support, etc.)

### Task 8: Data Type Precision Improvements

**Objective**: Address the TODO in migrations about precision for financial
calculations.

**Subtasks**:

- [ ] Evaluate current REAL usage for financial data
- [ ] Replace REAL with NUMERIC(precision, scale) for monetary values:
  - onchain_input_amount: Use NUMERIC for 18-decimal precision
  - onchain_price_per_share_cents: Use NUMERIC for exact cents
  - schwab_price_per_share_cents: Use NUMERIC for exact cents
- [ ] Update Rust code to handle NUMERIC types (BigDecimal or rust_decimal)
- [ ] Add proper decimal handling dependencies if needed
- [ ] Test precision is maintained through all calculations
- [ ] Verify no precision loss in financial operations

**Design Decisions**:

- Use NUMERIC for exact financial calculations
- Choose appropriate precision/scale for each monetary field
- Consider rust_decimal crate for decimal arithmetic
- Ensure precision requirements meet tokenized stock trading needs

## Phase 4: Testing and Validation

### Task 9: Update Test Infrastructure

**Objective**: Ensure all tests work with PostgreSQL.

**Subtasks**:

- [ ] Update test database setup to use PostgreSQL instead of SQLite
- [ ] Configure test database isolation (separate test DB or transactions)
- [ ] Update any SQLite-specific test utilities
- [ ] Verify all existing tests pass with PostgreSQL
- [ ] Add tests for PostgreSQL-specific functionality if needed
- [ ] Test database migration and rollback procedures

**Design Decisions**:

- Use separate PostgreSQL database for testing
- Maintain test isolation between test runs
- Preserve existing test structure and assertions
- Add PostgreSQL-specific test cases as needed

### Task 10: Integration Testing and Validation

**Objective**: Comprehensive testing of the complete system with PostgreSQL.

**Subtasks**:

- [ ] Run full integration tests with local PostgreSQL
- [ ] Test application startup/shutdown cycles
- [ ] Verify all database operations under load
- [ ] Test connection recovery and error handling
- [ ] Validate data persistence across application restarts
- [ ] Performance test basic operations (insert, query, update)
- [ ] Test concurrent access patterns

**Design Decisions**:

- Focus on real-world usage patterns
- Test error scenarios and recovery
- Validate performance is acceptable
- Ensure data consistency under concurrent access

## Phase 5: Documentation and Deployment

### Task 11: Update Documentation

**Objective**: Update all documentation to reflect PostgreSQL usage.

**Subtasks**:

- [ ] Update CLAUDE.md with new database setup instructions
- [ ] Update development workflow commands for PostgreSQL
- [ ] Document new environment variables and configuration
- [ ] Add troubleshooting guide for PostgreSQL issues
- [ ] Update any deployment documentation
- [ ] Add migration guide from SQLite for existing users

**Design Decisions**:

- Maintain existing documentation structure
- Focus on practical setup and troubleshooting
- Provide clear migration path for existing users
- Include PostgreSQL-specific operational notes

### Task 12: Production Deployment Considerations

**Objective**: Prepare for production PostgreSQL deployment.

**Subtasks**:

- [ ] Add configuration for PostgreSQL connection pooling in production
- [ ] Document backup and recovery procedures
- [ ] Add monitoring and health check recommendations
- [ ] Consider connection security and SSL requirements
- [ ] Document scaling considerations for PostgreSQL

**Design Decisions**:

- Separate development and production configurations clearly
- Focus on operational requirements
- Provide guidance for different deployment scenarios
- Maintain security best practices

## Success Criteria

- [ ] Local PostgreSQL development environment fully functional
- [ ] All existing tests pass with PostgreSQL
- [ ] No data precision loss in financial calculations
- [ ] Development workflow remains smooth and fast
- [ ] Documentation is complete and accurate
- [ ] Application performance is maintained or improved
