#!/usr/bin/env bash
# Rollback script for st0x-hedge deployment
#
# Restores backed-up configuration files from the last successful deployment
# and restarts containers with the previous working configuration.
#
# Usage:
#   ./rollback.sh [OPTIONS]
#
# Options:
#   --dry-run    Validate backup files exist and show what would be done
#
# Environment variables:
#   DATA_VOLUME_PATH (optional): Data directory path
#                                Defaults to /mnt/volume_nyc3_01 (production)
#                                Set to ./data for local testing
#
# Examples:
#   # Production: Test rollback validation
#   ./rollback.sh --dry-run
#
#   # Production: Perform actual rollback
#   ./rollback.sh
#
#   # Local testing: Validate rollback logic
#   DATA_VOLUME_PATH=./data ./rollback.sh --dry-run

set -euo pipefail

DRY_RUN=false

# Configuration
export DATA_VOLUME_PATH="${DATA_VOLUME_PATH:-/mnt/volume_nyc3_01}"
DOCKER_COMPOSE_BACKUP="${DATA_VOLUME_PATH}/docker-compose.yaml.backup"
ENV_BACKUP="${DATA_VOLUME_PATH}/.env.backup"
DOCKER_COMPOSE="${DATA_VOLUME_PATH}/docker-compose.yaml"
ENV_FILE="${DATA_VOLUME_PATH}/.env"
GRAFANA_DATASOURCE_BACKUP="${DATA_VOLUME_PATH}/grafana-datasource.yaml.backup"
GRAFANA_DATASOURCE="${DATA_VOLUME_PATH}/grafana-datasource.yaml"
GRAFANA_DOCKERFILE_BACKUP="${DATA_VOLUME_PATH}/Dockerfile.grafana.backup"
GRAFANA_DOCKERFILE="${DATA_VOLUME_PATH}/Dockerfile.grafana"

# Parse arguments
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -*)
            echo "ERROR: Unknown option: $1" >&2
            echo "Usage: $0 [--dry-run]" >&2
            exit 1
            ;;
        *)
            echo "ERROR: Unexpected argument: $1" >&2
            echo "Usage: $0 [--dry-run]" >&2
            exit 1
            ;;
    esac
done

if [ "${DRY_RUN}" = true ]; then
    echo "==> DRY RUN MODE - No changes will be made"
fi

echo "==> Rollback configuration:"
echo "    DATA_VOLUME_PATH=${DATA_VOLUME_PATH}"

if [ "${DRY_RUN}" = true ]; then
    echo ""
    echo "==> Validation checks:"

    # Check data directory exists
    if [ -d "${DATA_VOLUME_PATH}" ]; then
        echo "    ✓ Data directory exists: ${DATA_VOLUME_PATH}"
    else
        echo "    ✗ Data directory not found: ${DATA_VOLUME_PATH}" >&2
        exit 1
    fi

    # Check backup files exist
    if [ -f "${DOCKER_COMPOSE_BACKUP}" ]; then
        echo "    ✓ docker-compose.yaml.backup found"
    else
        echo "    ✗ docker-compose.yaml.backup not found: ${DOCKER_COMPOSE_BACKUP}" >&2
        echo "    No previous deployment to rollback to" >&2
        exit 1
    fi

    if [ -f "${ENV_BACKUP}" ]; then
        echo "    ✓ .env.backup found"
    else
        echo "    ✗ .env.backup not found: ${ENV_BACKUP}" >&2
        echo "    No previous deployment to rollback to" >&2
        exit 1
    fi

    # Check optional grafana backup files
    if [ -f "${GRAFANA_DATASOURCE_BACKUP}" ]; then
        echo "    ✓ grafana-datasource.yaml.backup found"
    else
        echo "    ⚠ grafana-datasource.yaml.backup not found (optional)" >&2
    fi

    if [ -f "${GRAFANA_DOCKERFILE_BACKUP}" ]; then
        echo "    ✓ Dockerfile.grafana.backup found"
    else
        echo "    ⚠ Dockerfile.grafana.backup not found (optional)" >&2
    fi

    # Check docker is available
    if command -v docker &> /dev/null; then
        echo "    ✓ docker command available"
    else
        echo "    ✗ docker command not found" >&2
        exit 1
    fi

    # Check current directory for docker compose operations
    if [ -f "${DOCKER_COMPOSE}" ]; then
        echo "    ✓ Current docker-compose.yaml exists"
    else
        echo "    ⚠ Warning: Current docker-compose.yaml not found" >&2
    fi

    echo ""
    echo "==> DRY RUN: Would execute the following steps:"
    echo "    1. Change to directory: ${DATA_VOLUME_PATH}"
    echo "    2. Stop containers: docker compose down"
    echo "    3. Restore: cp docker-compose.yaml.backup docker-compose.yaml"
    echo "    4. Restore: cp .env.backup .env"
    echo "    5. Start containers: docker compose up -d"
    echo ""
    echo "==> DRY RUN: All validation checks passed!"
    exit 0
fi

# Perform actual rollback
echo "==> Performing safety checks..."

# Check docker is available
if ! command -v docker &> /dev/null; then
    echo "ERROR: docker command not found" >&2
    exit 1
fi

# Check docker daemon is reachable
if ! docker info &> /dev/null; then
    echo "ERROR: Docker daemon is not reachable. Is Docker running?" >&2
    exit 1
fi

# Check data directory exists and is writable
if [ ! -d "${DATA_VOLUME_PATH}" ]; then
    echo "ERROR: Data directory not found: ${DATA_VOLUME_PATH}" >&2
    exit 1
fi

if [ ! -w "${DATA_VOLUME_PATH}" ]; then
    echo "ERROR: Data directory is not writable: ${DATA_VOLUME_PATH}" >&2
    exit 1
fi

echo "==> Validating backup files..."
if [ ! -f "${DOCKER_COMPOSE_BACKUP}" ] || [ ! -f "${ENV_BACKUP}" ]; then
    echo "ERROR: Backup files not found. Cannot rollback." >&2
    echo "  Missing: ${DOCKER_COMPOSE_BACKUP}" >&2
    echo "  Missing: ${ENV_BACKUP}" >&2
    exit 1
fi

cd "${DATA_VOLUME_PATH}"

# Check that containers exist before trying to stop them
if ! docker compose ps --quiet 2>/dev/null | grep -q .; then
    echo "WARNING: No running containers found in this compose project" >&2
fi

echo "==> Stopping current containers..."
docker compose down

echo "==> Restoring backed-up configuration..."
cp -p "${DOCKER_COMPOSE_BACKUP}" "${DOCKER_COMPOSE}"
cp -p "${ENV_BACKUP}" "${ENV_FILE}"

# Conditionally restore grafana config files if backups exist
if [ -f "${GRAFANA_DATASOURCE_BACKUP}" ]; then
    echo "==> Restoring grafana-datasource.yaml from backup"
    cp -p "${GRAFANA_DATASOURCE_BACKUP}" "${GRAFANA_DATASOURCE}"
else
    echo "==> No backup found for grafana-datasource.yaml, skipping"
fi

if [ -f "${GRAFANA_DOCKERFILE_BACKUP}" ]; then
    echo "==> Restoring Dockerfile.grafana from backup"
    cp -p "${GRAFANA_DOCKERFILE_BACKUP}" "${GRAFANA_DOCKERFILE}"
else
    echo "==> No backup found for Dockerfile.grafana, skipping"
fi

echo "==> Starting containers with restored configuration..."
docker compose up -d

echo "==> Waiting for containers to start..."
sleep 10

echo "==> Checking container status..."
docker compose ps

echo "==> Rollback complete!"
echo "    Restored configuration from backup files"
