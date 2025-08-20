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

## Task 4. Configure Environment Variables and Runtime

- [x] Document all required environment variables in Dockerfile
- [x] Set up `DATABASE_URL` with default SQLite path
- [x] Configure logging level and output format (`RUST_LOG=info`)
- [x] Set up proper working directory for runtime
- [x] Configure entry point for main application binary

## Task 5. Optimize Image Size and Security

- [x] Use multi-stage build to minimize final image size (single-stage optimized with Nix)
- [x] Remove unnecessary packages and files from runtime image
- [x] Optimize layer caching for faster rebuilds (cargo-chef + Docker layers)
- [x] Run security scan on final image
- [x] Implement principle of least privilege for container user
- [x] Remove or secure any sensitive information in layers
- [x] Update @PLAN.md with your progress 

## Task 6. Create a GitHub Action for building the image

- [ ] Create `.github/workflows/docker-build.yml` workflow file
- [ ] Configure workflow to trigger on pushes to `master` branch and workflow dispatch
- [ ] Configure build step with proper image tagging (latest, commit SHA)
- [ ] Set up build caching to optimize CI build times
- [ ] Update @PLAN.md with your progress 
