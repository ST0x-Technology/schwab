# Digital Ocean Deployment Implementation Plan

## Overview

Deploy the schwad arbitrage bot to Digital Ocean using a Droplet with Block
Storage for SQLite persistence and DO Container Registry for image management.

## Implementation Sections

### Section 1: Infrastructure Setup

**Objective:** Create Digital Ocean Droplet with Docker and Block Storage

**Tasks:**

- [x] Create Digital Ocean Droplet with Ubuntu 22.04, Docker pre-installed, and
      Block Storage volume
- [x] Test SSH connectivity and verify Docker and volume mounting

**Rationale:** Digital Ocean's Droplet creation handles Docker installation and
Block Storage mounting automatically. User ID 1001 matches existing Dockerfile
configuration.

### Section 2: Digital Ocean Container Registry Setup

**Objective:** Configure DO Container Registry for image storage and deployment

**Tasks:**

- [x] Generate DO API token with required permissions and save the required
      scopes
- [x] Configure local doctl CLI for registry access testing
- [x] Test image push/pull workflow to verify registry connectivity

**Rationale:** DO Container Registry integrates seamlessly with Droplet
deployment and eliminates external dependencies.

### Section 3: GitHub Actions CI/CD Pipeline

**Objective:** Automate build and deployment using DO Container Registry

**Tasks:**

- [x] Create GitHub Actions workflow based on updated report example
- [x] Configure doctl installation and authentication in CI
- [x] Implement Docker build with commit-based tagging strategy (7-char SHA)
- [x] Add image push to DO Container Registry
- [x] Configure SSH deployment with graceful container restart and health check
      validation
- [x] Set up required repository secrets (DIGITALOCEAN_ACCESS_TOKEN,
      DROPLET_HOST, DROPLET_SSH_KEY). Start by just having those as environment
      variables, so that the deployment config can be tested with a separate
      account and plain text values and then that account can be cleaned up and
      actual production values will be set.

**Implementation Details:**

Created `.github/workflows/deploy.yaml` with:

- doctl installation and authentication using environment variables
- Docker build with 7-character SHA tagging plus latest tag
- Image push to DO Container Registry (registry.digitalocean.com/stox)
- SSH-based deployment to Droplet using appleboy/ssh-action
- Graceful container restart (stop/rm old, start new with --restart
  unless-stopped)
- Health checks that fail the pipeline if container doesn't start or shows
  errors
- Automatic cleanup of old Docker images (keeps last 3 versions)
- Environment variable injection for all application configuration
- Uses environment variables instead of secrets for initial testing phase

**Rationale:** DO Container Registry with doctl provides native integration.
Commit-based tagging enables rollback capabilities. Health checks ensure
successful deployment.

### Section 4: Production Environment Configuration

**Objective:** Configure secure environment variable management for production
deployment

**Tasks:**

- [ ] Create production environment variable template for deployment
- [ ] Configure Charles Schwab API credentials injection via environment
      variables
- [ ] Set WebSocket RPC URL for mainnet operations
- [ ] Configure DATABASE_URL for Block Storage mount path
      (`/mnt/volume_nyc3_01/schwab.db`)
- [ ] Set orderbook contract addresses and order hashes for production
- [ ] Test complete application startup with production configuration

**Rationale:** Environment variable injection keeps secrets secure while
enabling environment-specific configuration.

### Section 5: Deployment Testing and Validation

**Objective:** Validate complete deployment pipeline and application
functionality

**Tasks:**

- [ ] Execute full deployment pipeline from code push to running application
- [ ] Verify SQLite database initialization and persistence across restarts
- [ ] Test Charles Schwab authentication flow in production environment
- [ ] Monitor WebSocket connections and trade processing functionality
- [ ] Test rollback procedure with previous image version
- [ ] Document troubleshooting guide for common deployment issues

**Rationale:** End-to-end testing ensures the deployment works reliably before
production use.

## Technical Design Decisions

### SQLite Configuration

- **Use default SQLx settings**: Current configuration should work fine for
  single-container deployment
- **Block Storage mounting**: Direct host-to-container volume mount for
  persistence

### Infrastructure Architecture

- **DO Container Registry**: Native integration with Droplet deployment
- **Single-container deployment**: Simplifies management while meeting
  performance needs

### CI/CD Strategy

- **Commit-based tagging**: Enables precise version tracking and rollback
  capability
- **Graceful deployment**: Stop old container before starting new one
- **Health check validation**: Ensures deployment success before completing

### Security and Reliability

- **Environment variable injection**: Keeps secrets out of images
- **Non-root container user**: Proper privilege management
- **Automatic restart policies**: Handles container failures
- **Volume snapshots**: Manual backup capability

## Success Criteria

1. **Data Persistence**: Database survives container restarts and deployments
2. **Automated Deployment**: Code changes trigger reliable production updates
3. **Application Functionality**: Bot processes trades and maintains API
   connectivity
4. **Rollback Capability**: Can revert to previous version if issues arise

## Risk Mitigation

- **Deployment failures**: Health checks and graceful container restart
  procedures
- **Data loss**: Regular volume snapshots and persistent Block Storage
- **Security**: Environment variable management and proper container permissions

This plan leverages Digital Ocean's native container registry while keeping
SQLite configuration simple with sensible defaults.
