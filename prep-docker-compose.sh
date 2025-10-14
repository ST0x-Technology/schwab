#!/usr/bin/env bash
set -euo pipefail

PROD_MODE=false

# Parse arguments
for arg in "$@"; do
  case $arg in
    --prod)
      PROD_MODE=true
      shift
      ;;
    *)
      echo "Unknown argument: $arg"
      echo "Usage: prep-docker-compose.sh [--prod]"
      exit 1
      ;;
  esac
done

if [ "$PROD_MODE" = true ]; then
  echo "==> Production mode: using registry images"

  # Validate required environment variables for production
  if [ -z "${REGISTRY_NAME:-}" ]; then
    echo "ERROR: REGISTRY_NAME environment variable is required for --prod mode"
    exit 1
  fi
  if [ -z "${SHORT_SHA:-}" ]; then
    echo "ERROR: SHORT_SHA environment variable is required for --prod mode"
    exit 1
  fi
  if [ -z "${DATA_VOLUME_PATH:-}" ]; then
    echo "ERROR: DATA_VOLUME_PATH environment variable is required for --prod mode"
    exit 1
  fi
  if [ -z "${GRAFANA_ADMIN_PASSWORD:-}" ]; then
    echo "ERROR: GRAFANA_ADMIN_PASSWORD environment variable is required for --prod mode"
    exit 1
  fi

  export DOCKER_IMAGE="registry.digitalocean.com/${REGISTRY_NAME}/schwarbot:${SHORT_SHA}"
  export PULL_POLICY="always"
else
  echo "==> Local/debug mode: building image locally"

  export DOCKER_IMAGE="schwarbot:local"
  export DATA_VOLUME_PATH="./data"
  export PULL_POLICY="never"
  export GRAFANA_ADMIN_PASSWORD="admin"

  # Build Docker image with debug profile
  echo "==> Building Docker image with debug profile..."
  docker build --build-arg BUILD_PROFILE=debug -t "${DOCKER_IMAGE}" .
fi

# Generate docker-compose.yaml from template
echo "==> Generating docker-compose.yaml"
envsubst '$DOCKER_IMAGE $DATA_VOLUME_PATH $PULL_POLICY $GRAFANA_ADMIN_PASSWORD' < docker-compose.template.yaml > docker-compose.yaml

echo "==> docker-compose.yaml generated successfully"
echo "    DOCKER_IMAGE=$DOCKER_IMAGE"
echo "    DATA_VOLUME_PATH=$DATA_VOLUME_PATH"
echo "    PULL_POLICY=$PULL_POLICY"
