# Docker Containerization Plan

This document outlines the step-by-step implementation plan for containerizing the Rust arbitrage bot using Nix with DeterminateSystems installer to ensure consistent tooling between development and production environments.

## Task 1. Set Up Docker Build Context

- [x] Create `.dockerignore` file to exclude unnecessary files from build context
- [x] Add `target/` directory (Rust build artifacts)
- [x] Add `.git/` directory and git-related files
- [x] Add local database files (`*.db`, `*.db-*`)
- [x] Add environment files (`.env*`) for security
- [x] Update @PLAN.md with your progress 

## Task 2. Create Dockerfile, install and Configure Nix with DeterminateSystems Installer

- [x] Start with latest Ubuntu LTS as base image for stability
- [x] Install curl and other basic dependencies needed for Nix installer
- [x] Add DeterminateSystems Nix installer using their official installation script
- [x] Set up proper Nix configuration for containerized environment
- [x] Test basic Nix functionality and flake support, specifically focusing on the dev shell
- [x] Update @PLAN.md with your progress 

## Task 3. Set Up Build Stage

- [x] Copy `flake.nix` and `flake.lock` files to leverage existing Nix configuration
- [x] Copy source code and necessary build files with proper Docker layer caching
- [x] Run `nix develop` to enter development environment
- [x] Execute Solidity artifact preparation: `nix run .#prepSolArtifacts`
- [x] Integrate cargo-chef for optimized Rust dependency caching
- [x] Set up database and run migrations for SQLx compile-time verification
- [x] Build the main Rust binary: main bot (`cargo build --release --bin main`)
- [x] Build auth binary: (`cargo build --release --bin auth`)
- [x] Update @PLAN.md with your progress 

## Task 4. Create Minimal Runtime Stage

- [ ] Start with minimal base image (Ubuntu 22.04 slim or distroless)
- [ ] Create non-root user for running the application
- [ ] Copy only necessary runtime binaries from build stage
- [ ] Set up proper file permissions for binaries
- [ ] Create directory structure for application data
- [ ] Set up directory for SQLite database persistence
- [ ] Configure proper ownership and permissions

## Task 5. Configure Environment Variables and Runtime

- [x] Document all required environment variables in Dockerfile
- [x] Set up `DATABASE_URL` with default SQLite path
- [x] Configure logging level and output format (`RUST_LOG=info`)
- [x] Set up proper working directory for runtime
- [x] Configure entry point for main application binary
- [ ] Configure `WS_RPC_URL` for blockchain connection
- [ ] Set up Schwab API configuration variables (`APP_KEY`, `APP_SECRET`, etc.)

## Task 6. Add Health Check and Monitoring

- [ ] Create simple health check endpoint or command
- [ ] Configure Docker HEALTHCHECK instruction
- [ ] Set appropriate health check intervals and timeouts
- [ ] Test health check functionality
- [ ] Add proper signal handling for graceful shutdown
- [ ] Configure restart policies for production deployment

## Task 7. Optimize Image Size and Security

- [x] Use multi-stage build to minimize final image size (single-stage optimized with Nix)
- [x] Remove unnecessary packages and files from runtime image
- [x] Optimize layer caching for faster rebuilds (cargo-chef + Docker layers)
- [ ] Run security scan on final image
- [ ] Implement principle of least privilege for container user
- [ ] Remove or secure any sensitive information in layers

## Task 8. Add Volume and Persistence Configuration

- [x] Add migration handling for database schema updates (SQLx migrations in build)
- [ ] Configure volume mount for SQLite database persistence
- [ ] Set up proper permissions for volume-mounted directories  
- [ ] Document volume requirements for deployment
- [ ] Test database persistence across container restarts
- [ ] Configure backup and recovery considerations

## Task 9. Create Build and Test Scripts

- [ ] Create build script for easy Docker image building
- [ ] Add version tagging strategy for images
- [ ] Create test script to validate container functionality
- [ ] Test container startup and shutdown procedures
- [ ] Validate all environment variable configurations
- [ ] Test database initialization and migration
- [ ] Verify Schwab API connectivity from container

## Task 10. Documentation and Deployment Preparation

- [ ] Update README.md with Docker deployment instructions
- [ ] Document required environment variables and their purposes
- [ ] Create example environment variable template
- [ ] Document volume mounting requirements
- [ ] Add container deployment examples
- [ ] Document resource requirements and limits
- [ ] Add troubleshooting guide for common container issues

## Task 11. Cloud Deployment Considerations

- [ ] Configure for cloud container platforms (AWS ECS, Google Cloud Run, etc.)
- [ ] Set up proper logging for cloud environments
- [ ] Configure resource limits and requests
- [ ] Add container registry pushing instructions
- [ ] Document secrets management for production
- [ ] Configure monitoring and alerting endpoints
- [ ] Plan for horizontal scaling considerations

## Task 12. Testing and Validation

- [ ] Test container build process end-to-end
- [ ] Validate that Nix tooling versions match development environment
- [ ] Test application startup and basic functionality
- [ ] Verify database operations work correctly
- [ ] Test environment variable injection and configuration
- [ ] Validate volume persistence across container lifecycle
- [ ] Test graceful shutdown and restart procedures
- [ ] Perform basic integration testing with mock services

## Task 13. Performance and Resource Optimization

- [ ] Benchmark container startup time
- [ ] Measure memory and CPU usage patterns
- [ ] Optimize Docker layer caching for CI/CD pipelines
- [ ] Configure appropriate resource limits
- [ ] Test under various load conditions
- [ ] Document performance characteristics and requirements
- [ ] Optimize for cold start scenarios in serverless environments
