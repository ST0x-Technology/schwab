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
based on mode.

- [ ] Copy script from feat/pnl branch as starting point
- [ ] Support `--prod` flag for CI deployment mode
- [ ] Support `--local` (default) for local testing mode
- [ ] In local mode: build image with debug profile, use `./data` volume path,
      pull_policy=never
- [ ] In local mode: run
      `docker build --build-arg BUILD_PROFILE=debug -t schwarbot:local .`
- [ ] In prod mode: validate required env vars (REGISTRY_NAME, SHORT_SHA,
      DATA_VOLUME_PATH, GRAFANA_ADMIN_PASSWORD)
- [ ] In prod mode: use registry image, use `/mnt/volume_nyc3_01` volume path,
      pull_policy=always
- [ ] Generate docker-compose.yaml using
      `envsubst '$DOCKER_IMAGE $DATA_VOLUME_PATH $PULL_POLICY $GRAFANA_ADMIN_PASSWORD'`

## Task 4. Update docker-compose.template.yaml

**Changes**: Update template to use variable substitution for image, paths, and
policies. Set broker-specific environment variables per container.

- [ ] Replace hardcoded registry image with `image: ${DOCKER_IMAGE}`
- [ ] Add `pull_policy: ${PULL_POLICY}` to both schwarbot and alpacabot
- [ ] Set `BROKER=schwab` in schwarbot environment section
- [ ] Set `BROKER=alpaca` in alpacabot environment section
- [ ] Add `ENCRYPTION_KEY` to schwarbot environment (NOT alpacabot)
- [ ] Replace hardcoded volume paths with `${DATA_VOLUME_PATH}:/data`
- [ ] Update grafana database volume mounts to use
      `${DATA_VOLUME_PATH}/schwab.db` and `${DATA_VOLUME_PATH}/alpaca.db`

## Task 5. Update Dockerfile with BUILD_PROFILE support

**Changes**: Add build argument to support both debug and release builds.
Reference feat/pnl branch for exact implementation.

- [ ] Add `ARG BUILD_PROFILE=release` at builder stage
- [ ] Add conditional cargo build: if release then `--release`, else debug
- [ ] Update binary copy to handle both `target/release/server` and
      `target/debug/server` based on profile
- [ ] Test local build with `docker build --build-arg BUILD_PROFILE=debug`

## Task 6. Update GitHub Actions workflow to use prep script

**Changes**: Replace manual env var mapping and docker-compose generation with
prep script. Map GitHub secrets to correct CLI variable names.

- [ ] Add env var mappings to workflow: `SCHWAB_APP_KEY=${{ secrets.APP_KEY }}`
- [ ] Add env var mappings: `SCHWAB_APP_SECRET=${{ secrets.APP_SECRET }}`
- [ ] Add env var mappings: `ALPACA_API_KEY_ID=${{ secrets.ALPACA_KEY }}`
- [ ] Add env var mappings: `ALPACA_API_SECRET_KEY=${{ secrets.ALPACA_SECRET }}`
- [ ] Add env var mappings: `ENCRYPTION_KEY=${{ secrets.TOKEN_ENCRYPTION_KEY }}`
- [ ] Remove manual envsubst and docker-compose.yaml generation code
- [ ] Replace with: export REGISTRY_NAME, SHORT_SHA, DATA_VOLUME_PATH,
      GRAFANA_ADMIN_PASSWORD
- [ ] Replace with: call `./prep-docker-compose.sh --prod`
- [ ] After deployment succeeds, save SHA:
      `echo "${SHORT_SHA}" > /mnt/volume_nyc3_01/.last-deployed-sha`

## Task 7. Create rollback script

**Implementation**: Track last-deployed SHA in state file. Script reads previous
SHA and regenerates docker-compose.yaml with old image.

- [ ] Create `rollback.sh` script in repo root
- [ ] Script accepts optional SHA argument, defaults to reading
      `/mnt/volume_nyc3_01/.last-deployed-sha`
- [ ] Script exports: REGISTRY_NAME=stox, SHORT_SHA=(from arg or file),
      DATA_VOLUME_PATH=/mnt/volume_nyc3_01
- [ ] Script reads GRAFANA_ADMIN_PASSWORD from environment
- [ ] Script calls `./prep-docker-compose.sh --prod`
- [ ] Script runs
      `cd /mnt/volume_nyc3_01 && docker compose down && docker compose up -d`
- [ ] Make script executable: `chmod +x rollback.sh`
- [ ] Document usage in script header comments

## Task 8. Test deployment locally using prep script

**Test procedure**: Validate that prep script works correctly in local mode
before deploying.

- [ ] Set required environment variables locally (all Schwab/Alpaca credentials)
- [ ] Run `./prep-docker-compose.sh` (defaults to local mode)
- [ ] Verify docker-compose.yaml generated with correct image tag
      (schwarbot:local)
- [ ] Verify docker-compose.yaml has pull_policy: never
- [ ] Verify docker-compose.yaml has volume paths as ./data
- [ ] Run `docker compose up -d`
- [ ] Check container logs: `docker compose logs schwarbot alpacabot`
- [ ] Verify schwarbot starts with BROKER=schwab, has ENCRYPTION_KEY
- [ ] Verify alpacabot starts with BROKER=alpaca, uses Alpaca credentials
- [ ] Confirm no "missing required arguments" errors in logs
- [ ] Clean up: `docker compose down`
