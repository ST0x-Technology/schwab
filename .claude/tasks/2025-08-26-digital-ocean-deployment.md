# Digital Ocean Deployment Implementation Plan

## Overview

Deploy the schwad arbitrage bot to Digital Ocean using a Droplet with Block
Storage for SQLite persistence and DO Container Registry for image management.

## Implementation Sections

### Section 1: Infrastructure Setup

**Objective:** Create Digital Ocean Droplet with Docker and Block Storage

**Tasks:**

- [ ] Create Digital Ocean Droplet with Ubuntu 22.04, Docker pre-installed, and
      Block Storage volume
- [ ] Configure firewall rules (SSH port 22, application port 8080)
- [ ] Set up Block Storage directory permissions for SQLite data
- [ ] Test SSH connectivity and verify Docker and volume mounting

**Rationale:** Digital Ocean's Droplet creation handles Docker installation and
Block Storage mounting automatically.

### Section 2: Docker Configuration with Block Storage

**Objective:** Create production-ready Dockerfile with proper volume mounting

**Tasks:**

- [ ] Create multi-stage Dockerfile with Rust builder and Debian slim runtime
- [ ] Configure container user (UID 1001) to match Block Storage permissions
- [ ] Install required system dependencies (ca-certificates, sqlite3)
- [ ] Configure volume mount from Block Storage to container data directory
- [ ] Test Docker build and SQLite database creation with persistence
- [ ] Implement health check endpoint for deployment validation

**Rationale:** Proper user ID mapping ensures SQLite file permissions work
correctly. Health checks enable reliable deployment verification.

### Section 3: Digital Ocean Container Registry Setup

**Objective:** Configure DO Container Registry for image storage and deployment

**Tasks:**

- [ ] Create Digital Ocean Container Registry (if not exists)
- [ ] Generate DO API token with registry permissions
- [ ] Configure local doctl CLI for registry access testing
- [ ] Test image push/pull workflow to verify registry connectivity

**Rationale:** DO Container Registry integrates seamlessly with Droplet
deployment and eliminates external dependencies.

### Section 4: GitHub Actions CI/CD Pipeline

**Objective:** Automate build and deployment using DO Container Registry

**Tasks:**

- [ ] Create GitHub Actions workflow with DO-specific deployment steps
- [ ] Configure doctl installation and authentication in CI
- [ ] Implement Docker build with commit-based tagging strategy
- [ ] Add image push to DO Container Registry
- [ ] Configure SSH deployment with graceful container restart
- [ ] Add post-deployment health check validation
- [ ] Set up required repository secrets (DIGITALOCEAN_ACCESS_TOKEN,
      DROPLET_HOST, DROPLET_SSH_KEY)

**Rationale:** DO Container Registry with doctl provides native integration.
Commit-based tagging enables rollback capabilities.

### Section 5: Production Environment Configuration

**Objective:** Configure secure environment variable management for production
deployment

**Tasks:**

- [ ] Create production environment variable template
- [ ] Configure Charles Schwab API credentials injection
- [ ] Set WebSocket RPC URL for mainnet operations
- [ ] Configure DATABASE_URL for Block Storage mount path
- [ ] Set orderbook contract addresses and order hashes
- [ ] Implement environment variable passing in Docker deployment
- [ ] Test complete application startup with production configuration

**Rationale:** Environment variable injection keeps secrets secure while
enabling environment-specific configuration.

### Section 6: SQLx Dependencies and Offline Mode

**Objective:** Configure SQLx with proper dependencies for deployment

**Tasks:**

- [ ] Generate SQLx offline query data using `cargo sqlx prepare`
- [ ] Commit `.sqlx/` directory for CI/CD builds
- [ ] Set SQLX_OFFLINE=true in Dockerfile
- [ ] Test offline build process in CI environment

**Rationale:** Offline mode enables builds without database access.

### Section 7: Operational Setup and Monitoring

**Objective:** Implement monitoring and maintenance procedures

**Tasks:**

- [ ] Configure Docker restart policies for automatic recovery
- [ ] Set up basic log collection and rotation
- [ ] Create manual backup procedure using DO volume snapshots
- [ ] Document operational procedures (deployment, rollback, monitoring)
- [ ] Test container recovery after system reboot

**Rationale:** Proper restart policies and backup procedures ensure system
reliability and data protection.

### Section 8: Deployment Testing and Validation

**Objective:** Validate complete deployment pipeline and application
functionality

**Tasks:**

- [ ] Execute full deployment pipeline from code push to running application
- [ ] Verify SQLite database initialization
- [ ] Test Charles Schwab authentication flow in production environment
- [ ] Monitor WebSocket connections and trade processing
- [ ] Validate database persistence across container restarts
- [ ] Test rollback procedure with previous image version
- [ ] Document troubleshooting guide for common issues

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
