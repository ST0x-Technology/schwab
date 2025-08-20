# Docker Containerization Plan

This document outlines the step-by-step implementation plan for containerizing the Rust arbitrage bot using Nix with DeterminateSystems installer to ensure consistent tooling between development and production environments.

## Task 1. Set Up Docker Build Context

- [ ] Create `.dockerignore` file to exclude unnecessary files from build context
- [ ] Add `target/` directory (Rust build artifacts)
- [ ] Add `.git/` directory and git-related files
- [ ] Add local database files (`*.db`, `*.db-*`)
- [ ] Add environment files (`.env*`) for security
- [ ] Update @PLAN.md with your progress 

## Task 2. Create Multi-Stage Dockerfile Base Setup

- [ ] Start with Ubuntu 22.04 LTS as base image for stability
- [ ] Install curl and other basic dependencies needed for Nix installer
- [ ] Set up non-root user for security best practices
- [ ] Configure proper working directory structure
- [ ] Set up environment variables for Nix configuration

## Task 3. Install and Configure Nix with DeterminateSystems Installer

- [ ] Add DeterminateSystems Nix installer using their official installation script
- [ ] Configure Nix with flakes support (should be enabled by default)
- [ ] Set up proper Nix configuration for containerized environment
- [ ] Ensure Nix daemon starts correctly in container
- [ ] Test basic Nix functionality and flake support

## Task 4. Set Up Build Stage

- [ ] Copy `flake.nix` and `flake.lock` files to leverage existing Nix configuration
- [ ] Copy source code and necessary build files
- [ ] Run `nix develop` to enter development environment
- [ ] Execute Solidity artifact preparation: `nix run .#prepSolArtifacts`
- [ ] Build both Rust binaries: main bot (`cargo build --release --bin main`)
- [ ] Build auth binary for OAuth setup (`cargo build --release --bin auth`)
- [ ] Verify all binaries are built successfully
- [ ] Run basic tests to ensure build integrity

## Task 5. Create Minimal Runtime Stage

- [ ] Start with minimal base image (Ubuntu 22.04 slim or distroless)
- [ ] Create non-root user for running the application
- [ ] Copy only necessary runtime binaries from build stage
- [ ] Set up proper file permissions for binaries
- [ ] Create directory structure for application data
- [ ] Set up directory for SQLite database persistence
- [ ] Configure proper ownership and permissions

## Task 6. Configure Environment Variables and Runtime

- [ ] Document all required environment variables in Dockerfile
- [ ] Set up `DATABASE_URL` with default SQLite path
- [ ] Configure `WS_RPC_URL` for blockchain connection
- [ ] Set up Schwab API configuration variables (`APP_KEY`, `APP_SECRET`, etc.)
- [ ] Configure logging level and output format
- [ ] Set up proper working directory for runtime
- [ ] Configure entry point for main application binary

## Task 7. Add Health Check and Monitoring

- [ ] Create simple health check endpoint or command
- [ ] Configure Docker HEALTHCHECK instruction
- [ ] Set appropriate health check intervals and timeouts
- [ ] Test health check functionality
- [ ] Add proper signal handling for graceful shutdown
- [ ] Configure restart policies for production deployment

## Task 8. Optimize Image Size and Security

- [ ] Use multi-stage build to minimize final image size
- [ ] Remove unnecessary packages and files from runtime image
- [ ] Run security scan on final image
- [ ] Implement principle of least privilege for container user
- [ ] Remove or secure any sensitive information in layers
- [ ] Optimize layer caching for faster rebuilds

## Task 9. Add Volume and Persistence Configuration

- [ ] Configure volume mount for SQLite database persistence
- [ ] Set up proper permissions for volume-mounted directories  
- [ ] Document volume requirements for deployment
- [ ] Test database persistence across container restarts
- [ ] Configure backup and recovery considerations
- [ ] Add migration handling for database schema updates

## Task 10. Create Build and Test Scripts

- [ ] Create build script for easy Docker image building
- [ ] Add version tagging strategy for images
- [ ] Create test script to validate container functionality
- [ ] Test container startup and shutdown procedures
- [ ] Validate all environment variable configurations
- [ ] Test database initialization and migration
- [ ] Verify Schwab API connectivity from container

## Task 11. Documentation and Deployment Preparation

- [ ] Update README.md with Docker deployment instructions
- [ ] Document required environment variables and their purposes
- [ ] Create example environment variable template
- [ ] Document volume mounting requirements
- [ ] Add container deployment examples
- [ ] Document resource requirements and limits
- [ ] Add troubleshooting guide for common container issues

## Task 12. Cloud Deployment Considerations

- [ ] Configure for cloud container platforms (AWS ECS, Google Cloud Run, etc.)
- [ ] Set up proper logging for cloud environments
- [ ] Configure resource limits and requests
- [ ] Add container registry pushing instructions
- [ ] Document secrets management for production
- [ ] Configure monitoring and alerting endpoints
- [ ] Plan for horizontal scaling considerations

## Task 13. Testing and Validation

- [ ] Test container build process end-to-end
- [ ] Validate that Nix tooling versions match development environment
- [ ] Test application startup and basic functionality
- [ ] Verify database operations work correctly
- [ ] Test environment variable injection and configuration
- [ ] Validate volume persistence across container lifecycle
- [ ] Test graceful shutdown and restart procedures
- [ ] Perform basic integration testing with mock services

## Task 14. Performance and Resource Optimization

- [ ] Benchmark container startup time
- [ ] Measure memory and CPU usage patterns
- [ ] Optimize Docker layer caching for CI/CD pipelines
- [ ] Configure appropriate resource limits
- [ ] Test under various load conditions
- [ ] Document performance characteristics and requirements
- [ ] Optimize for cold start scenarios in serverless environments
