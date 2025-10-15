# Deployment Fix Plan

## Overview

Fix the broken deployment system after the broker-trait merge. The deployment is
currently failing because:

1. CLI requires all broker credentials regardless of which broker is selected
2. Environment variable names don't match between GitHub secrets and CLI
   expectations
3. No local testing capability for deployment changes
4. No rollback mechanism when deployments fail

## Task 1. Fix CLI argument parsing with conditional requirements

**Problem**: Both `SchwabAuthEnv` and `AlpacaAuthEnv` are flattened into `Env`,
making all their fields required even when only one broker is selected.

**Approach**: Remove flattened auth structs from `Env`. Parse `Env` first to
determine broker selection, then conditionally parse broker-specific auth
structs (`SchwabAuthEnv::parse()` or `AlpacaAuthEnv::parse()`) in
`Env::into_config()`. This way credentials are only required when the
corresponding broker is selected.

- [x] Remove flattened `schwab_auth` and `alpaca_auth` fields from `Env` struct
- [x] Update `Env::into_config()` to conditionally parse broker-specific configs
- [x] Update tests to verify credentials only required for selected broker
- [x] Verify existing test suite still passes

## Task 2. Update .env.example with all required variables

**Changes**: Add missing variables with proper naming that matches CLI
expectations. Do NOT add `BROKER` or `ALPACA_TRADING_MODE` (set in
docker-compose per container). Use `${VAR_NAME}` syntax for envsubst
compatibility.

- [x] Add `SCHWAB_APP_KEY=${SCHWAB_APP_KEY}`
- [x] Add `SCHWAB_APP_SECRET=${SCHWAB_APP_SECRET}`
- [x] Add `ENCRYPTION_KEY=${ENCRYPTION_KEY}`
- [x] Add `ALPACA_API_KEY_ID=${ALPACA_API_KEY_ID}`
- [x] Add `ALPACA_API_SECRET_KEY=${ALPACA_API_SECRET_KEY}`
- [x] Verify existing variables use `${VAR_NAME}` syntax

## Task 3. Adapt prep-docker-compose.sh from feat/pnl branch

**Script**: Support both local testing and prod deployment. Generate
docker-compose.yaml from template using envsubst with different variable values
based on mode. Note: schwarbot and alpacabot containers use the SAME image,
differentiated by BROKER env var. Grafana is built separately.

- [x] Copy script from feat/pnl branch as starting point
- [x] Analyze current docker-compose.template.yaml structure
- [x] Support `--prod` flag for CI deployment mode
- [x] Support `--local` (default) for local testing mode
- [x] In local mode: build image with debug profile, use `./data` volume path,
      pull_policy=never, image=schwarbot:local
- [x] In local mode: run
      `docker build --build-arg BUILD_PROFILE=debug -t schwarbot:local .`
- [x] In prod mode: validate required env vars (REGISTRY_NAME, SHORT_SHA,
      DATA_VOLUME_PATH, GRAFANA_ADMIN_PASSWORD)
- [x] In prod mode: use registry image, use DATA_VOLUME_PATH from env,
      pull_policy=always
- [x] Generate docker-compose.yaml using envsubst with proper variables

## Task 4. Update docker-compose.template.yaml

**Changes**: Update template to use variable substitution for image and pull
policy. Add CRITICAL missing BROKER environment variables. Both schwarbot and
alpacabot use the same image, differentiated by BROKER env var.

**Current Issues**:

- No BROKER env var set (both containers will fail without this!)
- Both have ENCRYPTION_KEY but only schwab needs it
- Hardcoded image path (should use variable)
- No pull_policy specified
- Volume paths already use ${DATA_VOLUME_PATH} ✓

**Changes needed**:

- [x] Replace hardcoded image with `image: ${DOCKER_IMAGE}` for both containers
- [x] Add `pull_policy: ${PULL_POLICY}` to both schwarbot and alpacabot
- [x] Add `BROKER=schwab` to schwarbot environment section (CRITICAL)
- [x] Add `BROKER=alpaca` to alpacabot environment section (CRITICAL)
- [x] Remove `ENCRYPTION_KEY` from alpacabot (keep only on schwarbot)
- [x] Verify volume paths still use ${DATA_VOLUME_PATH}

## Task 5. Update Dockerfile with BUILD_PROFILE support

**Changes**: Add build argument to support both debug and release builds.
Reference feat/pnl branch for exact implementation.

- [x] Add `ARG BUILD_PROFILE=release` at builder stage
- [x] Add conditional cargo build: if release then `--release`, else debug
- [x] Update binary copy to handle both `target/release/server` and
      `target/debug/server` based on profile
- [x] Test local build with `docker build --build-arg BUILD_PROFILE=debug`

## Task 6. Update GitHub Actions workflow to use prep script

**Changes**: Replace manual env var mapping and docker-compose generation with
prep script. Map GitHub secrets to correct CLI variable names.

- [x] Add env var mappings to workflow: `SCHWAB_APP_KEY=${{ secrets.APP_KEY }}`
- [x] Add env var mappings: `SCHWAB_APP_SECRET=${{ secrets.APP_SECRET }}`
- [x] Add env var mappings: `ALPACA_API_KEY_ID=${{ secrets.ALPACA_KEY }}`
- [x] Add env var mappings: `ALPACA_API_SECRET_KEY=${{ secrets.ALPACA_SECRET }}`
- [x] Add env var mappings: `ENCRYPTION_KEY=${{ secrets.TOKEN_ENCRYPTION_KEY }}`
- [x] Remove manual envsubst and docker-compose.yaml generation code
- [x] Replace with: export REGISTRY_NAME, SHORT_SHA, DATA_VOLUME_PATH,
      GRAFANA_ADMIN_PASSWORD
- [x] Replace with: call `./prep-docker-compose.sh --prod`
- [x] Backup config files before deployment for rollback capability

## Task 7. Create rollback script

**Implementation**: Backup working configuration files before deployment.
Rollback restores the backed-up config instead of regenerating. This ensures
rollback uses EXACT working configuration from before deployment, avoiding
issues when prep scripts or templates change between deployments.

**Approach**:

- Deployment backs up `docker-compose.yaml` and `.env` before generating new
  config
- Rollback stops containers, restores backup files, and restarts
- No SHA tracking needed - backup files are the rollback point

- [x] Update deploy.yaml to backup config files before deployment
- [x] Rewrite `rollback.sh` to restore backed-up configuration files
- [x] Remove SHA tracking logic from rollback.sh
- [x] Remove prep script regeneration from rollback.sh
- [x] Script stops containers, restores `.env.backup` and
      `docker-compose.yaml.backup`, restarts
- [x] Support DATA_VOLUME_PATH env var (default `/mnt/volume_nyc3_01`)
- [x] Keep `--dry-run` mode with validation checks
- [x] Update documentation with backup-based approach
- [x] Make script executable: `chmod +x rollback.sh`

## Task 7.5. Add automatic rollback on deployment failure

**Rationale**: If deployment fails health checks, automatically rollback to
backed-up configuration to minimize downtime. Production should always be in a
working state (either old or new version), not left in a broken state requiring
manual intervention.

**Approach**:

- Wrap deployment steps (container start + health checks) in error handling
- On failure, check if backup files exist
- If backups exist: restore them and restart containers
- Log rollback actions clearly for debugging
- Still exit with error code so GitHub Actions shows deployment failed
- Preserve all deployment logs for post-mortem analysis

**Changes needed**:

- [x] Update deploy.yaml to add error handling around deployment steps
- [x] On deployment failure, check if backup files exist
- [x] If backups exist, call rollback logic (stop, restore, start)
- [x] Log rollback actions with timestamps
- [x] Exit with error code after rollback (for CI visibility)
- [x] Ensure deployment logs are preserved before rollback

## Task 8. Test deployment locally using prep script

**Test procedure**: Validate that prep script works correctly in local mode
before deploying.

- [x] Set required environment variables locally (blockchain config + Alpaca
      credentials)
- [x] Run `./prep-docker-compose.sh --skip-build` to regenerate
      docker-compose.yaml
- [x] Verify docker-compose.yaml has BROKER=dry-run for schwarbot
- [x] Verify docker-compose.yaml has BROKER=alpaca for alpacabot
- [x] Verify docker-compose.yaml has pull_policy: never
- [x] Verify docker-compose.yaml has volume paths as ./data

## Task 9. Rename Alpaca environment variables to match their terminology

**Rationale**: Alpaca calls these "API key" and "API secret", not "API key ID"
and "API secret key". Current names are confusing and don't match Alpaca's
documentation.

**Changes completed**:

- [x] Rename `ALPACA_API_KEY_ID` to `ALPACA_API_KEY` in
      crates/broker/src/alpaca/auth.rs
- [x] Rename `ALPACA_API_SECRET_KEY` to `ALPACA_API_SECRET` in
      crates/broker/src/alpaca/auth.rs
- [x] Update .env.example with new variable names
- [x] Update GitHub Actions workflow secrets mapping
- [x] Update all documentation references (README.md)
- [x] Update test code in crates/broker/src/alpaca/broker.rs

**Implementation details**:

Updated all references to Alpaca environment variables across the codebase:

1. **auth.rs**: Renamed struct fields `alpaca_api_key_id` → `alpaca_api_key` and
   `alpaca_api_secret_key` → `alpaca_api_secret` in `AlpacaAuthEnv` struct,
   along with all usages in `AlpacaClient` struct and methods
2. **broker.rs tests**: Updated test helper function `create_test_auth_env` to
   use new field names
3. **.env.example**: Updated environment variable names in configuration
   template
4. **deploy.yaml**: Updated GitHub Actions workflow to use new variable names in
   both the `envs` list and `env` section
5. **README.md**: Updated Alpaca setup documentation with correct variable names

Variables now match Alpaca's official terminology as documented in their API
documentation.
