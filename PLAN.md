# REST API Implementation Plan

## Goal

Create a single `server` binary that runs both the REST API server and the
arbitrage bot with automatic retry logic for expired tokens.

## Step-by-Step Implementation

### Task 1: Add Dependencies and Setup

**Subtasks:**

- [ ] Add Rocket web framework dependencies to `Cargo.toml`
- [ ] Add required tokio sync and time dependencies
- [ ] Update binary configuration to use `server` instead of `main`
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 2: Create API Module Structure

**Subtasks:**

- [ ] Create `src/api/mod.rs` with basic module structure
- [ ] Create `src/api/responses.rs` for JSON response types
- [ ] Create `src/api/handlers.rs` for endpoint handlers
- [ ] Export API module from `src/lib.rs`
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 3: Implement Health Endpoint

**Subtasks:**

- [ ] Create basic health check handler in `src/api/handlers.rs`
- [ ] Define health response structure in `src/api/responses.rs`
- [ ] Add health endpoint route configuration
- [ ] Test endpoint returns proper JSON response
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 4: Implement Manual Auth Endpoint

**Subtasks:**

- [ ] Create auth refresh handler that wraps existing `run_oauth_flow`
- [ ] Define auth request/response structures for JSON API
- [ ] Modify auth flow to accept redirect URL from request body instead of stdin
- [ ] Add proper error handling and JSON responses
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 5: Modify Bot Flow for Retry Logic

**Subtasks:**

- [ ] Modify `src/lib.rs::run()` to handle `RefreshTokenExpired` error
- [ ] Implement exponential backoff retry loop when tokens are unavailable
- [ ] Add logging for retry attempts and token status
- [ ] Ensure bot continues trading when tokens become available
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 6: Create Server Binary

**Subtasks:**

- [ ] Create `src/bin/server.rs` to replace `main.rs`
- [ ] Implement concurrent execution of Rocket server and bot task
- [ ] Add graceful shutdown handling for both server and bot
- [ ] Configure server to bind to `0.0.0.0:8080`
- [ ] Run `cargo test -q` to verify all tests pass
- [ ] Update PLAN.md with progress

### Task 7: Integration and Testing

**Subtasks:**

- [ ] Test complete flow: server starts, bot retries on missing tokens
- [ ] Test manual auth endpoint with mock/real OAuth flow
- [ ] Test health endpoint accessibility
- [ ] Verify bot automatically starts trading when tokens are available
- [ ] Run `cargo test -q` and `cargo clippy` to verify all checks pass
- [ ] Update PLAN.md with final completion status

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
7. `PLAN.md` - This plan document for tracking progress

## Progress Tracking

- [ ] Task 1 Complete
- [ ] Task 2 Complete
- [ ] Task 3 Complete
- [ ] Task 4 Complete
- [ ] Task 5 Complete
- [ ] Task 6 Complete
- [ ] Task 7 Complete
