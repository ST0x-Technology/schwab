# REST API Implementation Plan

## Goal

Create a single `server` binary that runs both the REST API server and the
arbitrage bot with automatic retry logic for expired tokens.

## Step-by-Step Implementation

### Task 1: Add Dependencies and Setup

- [x] Add Rocket web framework dependencies using `cargo add` (to get the latest
      versions)
- [x] Add required tokio sync and time dependencies (they might already be
      present)
- [x] Update binary configuration to use `server` instead of `main`
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 2: Create API Module Structure

- [x] Create `src/api.rs` with JSON response types and endpoint handlers
- [x] Export API module from `src/lib.rs`
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 3: Implement Health Endpoint

- [x] Create basic health check endpoint in `src/api.rs`
- [x] Add health endpoint route configuration
- [x] Test endpoint returns proper JSON response
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 4: Implement Manual Auth Endpoint

- [x] Create auth refresh endpoint that works similarly to the existing
      `run_oauth_flow`
- [x] Accept redirect URL from request body instead of stdin
- [x] Add proper error handling and JSON responses
- [x] Add comprehensive test coverage for the auth endpoint
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 5: Modify Bot Flow for Retry Logic

- [x] Modify `src/lib.rs::run()` to handle `RefreshTokenExpired` error
- [x] Implement constant backoff retry loop when tokens are unavailable, so that
      when someone goes through the manual auth flow and refresh tokens become
      available, the bot can unblock itself. Use `backon` for retries
- [x] Add logging for retry attempts and token status
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 6: Create Server Binary

- [x] Create `src/bin/server.rs` to replace `main.rs`
- [x] Implement concurrent execution of Rocket server and bot task in
      `fn launch` in src/lib.rs
- [x] Add graceful shutdown handling for both server and bot
- [x] Configure server to bind to `0.0.0.0:8080`
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 7: Remove the auth binary and add the manual auth flow as an extra cli command

- [x] Add `Auth` command to `Commands` enum in `src/cli.rs`
- [x] Implement auth command handler in `run_command_with_writers` function
- [x] Add proper CLI help text and argument handling for auth command
- [x] Add comprehensive test coverage for the new auth command
- [x] Remove `src/bin/auth.rs` binary entirely
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [x] Update @PLAN.md with progress

### Task 8: Integration and Testing

- [ ] Test complete flow: server starts, bot retries on missing tokens
- [ ] Test manual auth endpoint with mock OAuth flow
- [ ] Test health endpoint accessibility
- [ ] Verify bot automatically starts trading when tokens are available
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a` and ensure it
      passes
- [ ] Update @PLAN.md with final completion status

## Architecture Overview

```
Server Binary:
├── Rocket HTTP Server (0.0.0.0:8080)
│   ├── GET /health - Health check
│   └── POST /auth/refresh - Manual OAuth flow
└── Bot Task (Always Running)
    ├── Retry loop on RefreshTokenExpired
    └── Existing arbitrage logic when tokens available
```

## Files to Create/Modify

1. `Cargo.toml` - Add Rocket dependencies
2. `src/api/mod.rs` - API module
3. `src/api/handlers.rs` - Endpoint handlers
4. `src/api/responses.rs` - JSON response types
5. `src/bin/server.rs` - New unified server binary (replaces main.rs)
6. `src/lib.rs` - Export API module, modify run() for retry logic
7. @PLAN.md - This plan document for tracking progress

## Progress Tracking

- [x] Task 1 Complete
- [x] Task 2 Complete
- [x] Task 3 Complete
- [x] Task 4 Complete
- [x] Task 5 Complete
- [x] Task 6 Complete
- [x] Task 7 Complete
- [ ] Task 8 Complete
