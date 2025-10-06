# Token Encryption Implementation Plan

**Date:** 2025-10-03 **Task:** Implement AES-256-GCM encryption for Schwab OAuth
tokens in SQLite database

## Context

Currently, Schwab OAuth tokens (access_token and refresh_token) are stored in
plaintext in the SQLite database. With the new Grafana integration that reads
directly from the database, we need to encrypt these sensitive tokens at rest to
prevent unauthorized access.

## Design Decisions

### Encryption Algorithm

- **Algorithm:** AES-256-GCM (Galois/Counter Mode)
- **Rationale:**
  - Industry-standard authenticated encryption
  - Built-in authentication prevents tampering
  - Constant-time operations prevent timing attacks
  - Well-audited Rust implementation available

### Key Management

- **Storage:** Environment variable `TOKEN_ENCRYPTION_KEY`
- **Format:** 32 bytes (256 bits) as 64 hex characters
- **Generation:** `openssl rand -hex 32`
- **Rationale:**
  - Simple for MVP
  - Keeps key out of version control
  - Can be rotated via deployment config

### Storage Format

- **Ciphertext:** `nonce (12 bytes) || ciphertext || auth_tag`
- **Encoding:** Raw bytes in SQLite TEXT column
- **Nonce:** Randomly generated per encryption (must be unique, not secret)

### Migration Strategy

- **Hard Switch:** No backwards compatibility with plaintext tokens
- **Deployment:** Requires re-authentication after upgrade
- **Rationale:**
  - Simpler codebase without legacy code paths
  - Forces immediate security improvement
  - Clear migration point

## Implementation Steps

## Current Progress

- ✅ Section 1: Add Dependencies (completed)
- ✅ Section 2: Create Encryption Module (completed)
- ✅ Section 3: Update Error Types (completed)
- ✅ Section 4: Update Environment Configuration - partially complete (needs
  deployment workflow updates)
- ⏸️ Section 5: Database Migration (blocked - need to address deployment)
- ⏸️ Remaining sections (pending)

## Deployment Considerations

The `.env.example` file is used by the GitHub Actions deployment workflow as a
template. The workflow:

1. Base64 encodes `.env.example`
2. Sends it to the droplet
3. Uses `envsubst` to substitute environment variables
4. Creates the production `.env` file

**Required Changes:**

1. Add `TOKEN_ENCRYPTION_KEY` as GitHub Actions secret
2. Update deployment workflow to pass `TOKEN_ENCRYPTION_KEY` through
3. Update `.env.example` to use `$TOKEN_ENCRYPTION_KEY` for `envsubst`
   substitution

---

### Section 1: Add Dependencies

Add AES-GCM encryption crate.

- [x] Run `cargo add aes-gcm`
- [x] Verify alloy::hex is available for hex encoding/decoding
- [x] Check if flake.nix needs updates

**Why:** Use latest compatible version via cargo add.

### Section 2: Create Encryption Module

Create `src/schwab/encryption.rs` with core crypto logic.

**Module Structure:**

```rust
// src/schwab/encryption.rs

use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use aes_gcm::aead::{Aead, OsRng};
use alloy::hex;

/// 32-byte encryption key for AES-256
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    /// Parse from 64 hex character string
    pub fn from_hex(hex_str: &str) -> Result<Self, EncryptionError>;
}

/// Encrypts a token string, returns nonce || ciphertext
pub fn encrypt_token(key: &EncryptionKey, plaintext: &str) -> Result<Vec<u8>, EncryptionError>;

/// Decrypts a token from nonce || ciphertext
pub fn decrypt_token(key: &EncryptionKey, ciphertext: &[u8]) -> Result<String, EncryptionError>;

pub enum EncryptionError {
    InvalidKeyFormat,
    EncryptionFailed,
    DecryptionFailed,
    InvalidCiphertext,
    Utf8Error,
}
```

**Implementation:**

- [ ] Create file `src/schwab/encryption.rs`
- [ ] Add `pub mod encryption;` to `src/schwab/mod.rs`
- [ ] Implement `EncryptionKey` newtype
- [ ] Implement `EncryptionKey::from_hex()` using alloy::hex
- [ ] Implement `encrypt_token()`:
  - Generate random 12-byte nonce using OsRng
  - Create Aes256Gcm cipher from key
  - Encrypt plaintext bytes
  - Prepend nonce to ciphertext: `[nonce, ciphertext].concat()`
- [ ] Implement `decrypt_token()`:
  - Validate ciphertext length >= 12 bytes
  - Split nonce (first 12 bytes) and ciphertext (remainder)
  - Create Aes256Gcm cipher from key
  - Decrypt and verify authentication tag
  - Convert to UTF-8 string
- [ ] Implement `EncryptionError` enum with Display
- [ ] Add unit tests:
  - Test encryption/decryption round-trip
  - Test invalid key format
  - Test decryption with wrong key
  - Test corrupted ciphertext
  - Test short ciphertext
  - Test invalid UTF-8 in decrypted output

**Why:** Isolated crypto module is easier to test and audit.

### Section 3: Update Error Types

Add encryption errors to `SchwabError`.

- [ ] Add to `src/schwab/mod.rs`:
  ```rust
  pub enum SchwabError {
      // ... existing variants
      #[error("Encryption error: {0}")]
      Encryption(#[from] encryption::EncryptionError),
      #[error("Missing TOKEN_ENCRYPTION_KEY environment variable")]
      MissingEncryptionKey,
  }
  ```

**Why:** Proper error propagation with context.

### Section 4: Update Environment Configuration and Deployment

Add encryption key to `SchwabAuthEnv` and deployment workflow.

- [x] Update `src/schwab/auth.rs` (completed):
  ```rust
  #[derive(Parser, Debug, Clone)]
  pub struct SchwabAuthEnv {
      // ... existing fields
      #[clap(long, env)]
      pub token_encryption_key: String,
  }

  impl SchwabAuthEnv {
      pub fn get_encryption_key(&self) -> Result<EncryptionKey, SchwabError> {
          if self.token_encryption_key.is_empty() {
              return Err(SchwabError::MissingEncryptionKey);
          }
          Ok(EncryptionKey::from_hex(&self.token_encryption_key)?)
      }
  }
  ```

- [ ] Fix `.env.example` to use envsubst variable format:
  ```bash
  # Token encryption key (32 bytes as 64 hex characters)
  # Generate with: openssl rand -hex 32
  TOKEN_ENCRYPTION_KEY=$TOKEN_ENCRYPTION_KEY
  ```

- [ ] Update `.github/workflows/deploy.yaml`:
  - Add `TOKEN_ENCRYPTION_KEY` to the `envs` list in the "Deploy to Droplet"
    step (line 73)
  - Add `TOKEN_ENCRYPTION_KEY: ${{ secrets.TOKEN_ENCRYPTION_KEY }}` to the `env`
    section at the bottom (after line 194)

- [ ] Document in PLAN.md that the following manual steps are required for
      deployment:
  1. Generate encryption key: `openssl rand -hex 32`
  2. Add `TOKEN_ENCRYPTION_KEY` as a GitHub Actions secret in the repository
     settings
  3. After deployment, manually run `cargo run --bin cli -- auth` on the droplet
     to re-authenticate

**Why:** Environment variable keeps key out of code. Deployment workflow needs
to pass the secret through to the droplet for envsubst substitution.

### Section 5: Database Migration

Create migration to add encryption support and clear existing tokens.

- [ ] Run `sqlx migrate add add_token_encryption`
- [ ] Edit generated migration file:
  ```sql
  -- Add encryption version column (1 = AES-256-GCM)
  ALTER TABLE schwab_auth
  ADD COLUMN encryption_version INTEGER NOT NULL DEFAULT 1
  CHECK (encryption_version = 1);

  -- Clear existing plaintext tokens (forces re-authentication)
  DELETE FROM schwab_auth;

  -- Update schema comment
  COMMENT ON TABLE schwab_auth IS 'OAuth tokens encrypted with AES-256-GCM. Tokens stored as nonce||ciphertext in access_token and refresh_token columns.';
  ```

- [ ] Test migration:
  ```bash
  sqlx migrate run
  sqlx migrate revert
  sqlx migrate run
  ```

**Why:** Schema change required; clearing tokens simplifies deployment.

**Deployment Note:** Users must run `cargo run --bin cli -- auth` after upgrade.

### Section 6: Update Token Storage

Modify `SchwabTokens::store()` to encrypt before saving.

- [ ] Update `src/schwab/tokens.rs`:
  ```rust
  impl SchwabTokens {
      pub(crate) async fn store(
          &self,
          pool: &SqlitePool,
          env: &SchwabAuthEnv,
      ) -> Result<(), SchwabError> {
          let key = env.get_encryption_key()?;

          let encrypted_access = encrypt_token(&key, &self.access_token)?;
          let encrypted_refresh = encrypt_token(&key, &self.refresh_token)?;

          sqlx::query!(
              r#"
              INSERT INTO schwab_auth (
                  id,
                  access_token,
                  access_token_fetched_at,
                  refresh_token,
                  refresh_token_fetched_at,
                  encryption_version
              )
              VALUES (1, ?, ?, ?, ?, 1)
              ON CONFLICT(id) DO UPDATE SET
                  access_token = excluded.access_token,
                  access_token_fetched_at = excluded.access_token_fetched_at,
                  refresh_token = excluded.refresh_token,
                  refresh_token_fetched_at = excluded.refresh_token_fetched_at,
                  encryption_version = excluded.encryption_version
              "#,
              encrypted_access,
              self.access_token_fetched_at,
              encrypted_refresh,
              self.refresh_token_fetched_at,
          )
          .execute(pool)
          .await?;

          Ok(())
      }
  }
  ```

- [ ] Update call sites in `src/schwab/auth.rs`:
  - `get_tokens_from_code()`: pass `self` to `tokens.store(pool, self)`
  - `refresh_tokens()`: pass `self` to `new_tokens.store(pool, self)`

- [ ] Update call sites in `src/schwab/tokens.rs`:
  - `get_valid_access_token()`: pass `env` to `new_tokens.store(pool, env)`
  - `refresh_if_needed()`: pass `env` to `new_tokens.store(pool, env)`

**Why:** Encryption at write ensures all tokens are protected.

### Section 7: Update Token Loading

Modify `SchwabTokens::load()` to decrypt after retrieval.

- [ ] Update `src/schwab/tokens.rs`:
  ```rust
  impl SchwabTokens {
      pub(crate) async fn load(
          pool: &SqlitePool,
          env: &SchwabAuthEnv,
      ) -> Result<Self, SchwabError> {
          let row = sqlx::query!(
              r#"
              SELECT
                  id,
                  access_token,
                  access_token_fetched_at,
                  refresh_token,
                  refresh_token_fetched_at,
                  encryption_version
              FROM schwab_auth
              "#
          )
          .fetch_one(pool)
          .await?;

          let key = env.get_encryption_key()?;

          let access_token = decrypt_token(&key, &row.access_token)?;
          let refresh_token = decrypt_token(&key, &row.refresh_token)?;

          Ok(Self {
              access_token,
              access_token_fetched_at: DateTime::from_naive_utc_and_offset(
                  row.access_token_fetched_at,
                  Utc,
              ),
              refresh_token,
              refresh_token_fetched_at: DateTime::from_naive_utc_and_offset(
                  row.refresh_token_fetched_at,
                  Utc,
              ),
          })
      }
  }
  ```

- [ ] Update call sites:
  - `get_valid_access_token()`: change to `Self::load(pool, env)`
  - `refresh_if_needed()`: change to `Self::load(pool, env)`
  - Any other locations calling `load()`

- [ ] Update `src/schwab/auth.rs`:
  - `get_account_hash()`: pass `self` to `SchwabTokens::load(pool, self)`

**Why:** Decryption on read makes tokens usable.

### Section 8: Update Tests

Update test helpers and add encryption tests.

- [ ] Update `src/schwab/tokens.rs` test helpers:
  ```rust
  fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
      SchwabAuthEnv {
          // ... existing fields
          token_encryption_key: "0".repeat(64), // Valid test key
      }
  }

  fn create_test_env() -> SchwabAuthEnv {
      SchwabAuthEnv {
          // ... existing fields
          token_encryption_key: "0".repeat(64),
      }
  }
  ```

- [ ] Update all test calls to `store()` and `load()` with `env` parameter

- [ ] Add new tests in `src/schwab/encryption.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      #[test]
      fn test_encryption_round_trip() { }

      #[test]
      fn test_invalid_key_format() { }

      #[test]
      fn test_wrong_key_decryption() { }

      #[test]
      fn test_corrupted_ciphertext() { }

      #[test]
      fn test_short_ciphertext() { }
  }
  ```

- [ ] Add integration test in `src/schwab/tokens.rs`:
  ```rust
  #[tokio::test]
  async fn test_token_storage_encryption() {
      let pool = setup_test_db().await;
      let env = create_test_env();
      let tokens = SchwabTokens { /* ... */ };

      // Store encrypted
      tokens.store(&pool, &env).await.unwrap();

      // Verify database contains encrypted data (not plaintext)
      let raw = sqlx::query!("SELECT access_token FROM schwab_auth")
          .fetch_one(&pool)
          .await
          .unwrap();
      assert_ne!(raw.access_token, tokens.access_token.as_bytes());

      // Load and verify decryption
      let loaded = SchwabTokens::load(&pool, &env).await.unwrap();
      assert_eq!(loaded.access_token, tokens.access_token);
  }
  ```

- [ ] Run `cargo test -q --lib` to verify

**Why:** Tests ensure encryption works and catches regressions.

### Section 9: Documentation

Document encryption feature and deployment.

- [ ] Update `README.md`:
  ````markdown
  ## Security

  OAuth tokens are encrypted at rest using AES-256-GCM.

  Generate encryption key:

  ```bash
  openssl rand -hex 32
  ```
  ````

  Set environment variable:
  ```bash
  export TOKEN_ENCRYPTION_KEY=your_64_char_hex_key
  ```
  ```
  ```

- [ ] Update `CLAUDE.md` Configuration section:
  ```markdown
  - `TOKEN_ENCRYPTION_KEY`: AES-256 encryption key (32 bytes as 64 hex chars)
    - Generate: `openssl rand -hex 32`
    - Required for token encryption/decryption
  ```

- [ ] Update `.env.example` (already done in Section 4)

- [ ] Create deployment guide `docs/DEPLOYMENT.md` or in `CLAUDE.md`:
  ```markdown
  ## Upgrading to Encrypted Tokens

  1. Generate encryption key: `openssl rand -hex 32`
  2. Set `TOKEN_ENCRYPTION_KEY` environment variable
  3. Deploy new version
  4. Run `cargo run --bin cli -- auth` to re-authenticate
  5. Verify tokens work
  ```

**Why:** Clear docs prevent deployment issues.

### Section 10: Final Verification

Verify implementation before completion.

- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `cargo fmt`
- [ ] Run `cargo build --release`
- [ ] Manual testing:
  - Generate encryption key
  - Set TOKEN_ENCRYPTION_KEY
  - Delete existing tokens: `sqlite3 schwab.db "DELETE FROM schwab_auth;"`
  - Run auth: `cargo run --bin cli -- auth`
  - Check database: tokens should be binary/encrypted
  - Run bot: verify it works
  - Restart bot: verify tokens still work
- [ ] Security review:
  - No keys logged
  - No plaintext tokens logged
  - Errors fail closed

**Why:** Final checks catch issues before production.

## Deployment Guide

### Pre-Deployment Setup

1. **Generate Encryption Key:**
   ```bash
   openssl rand -hex 32
   ```
   Save this key securely - it will be needed for both GitHub Actions and manual
   operations.

2. **Add GitHub Secret:**
   - Go to repository Settings → Secrets and variables → Actions
   - Click "New repository secret"
   - Name: `TOKEN_ENCRYPTION_KEY`
   - Value: The 64-character hex key from step 1

### Deployment Process

1. **Merge and Deploy:**
   - Merge the encryption changes to master
   - Trigger the deployment workflow (automatic or manual)
   - The workflow will fail to start the bot because there are no tokens yet -
     this is expected

2. **Re-authenticate on Droplet:**
   ```bash
   ssh root@droplet
   cd /mnt/volume_nyc3_01
   docker compose run --rm schwarbot /app/cli auth
   ```
   Follow the OAuth flow to authenticate and store encrypted tokens.

3. **Restart Services:**
   ```bash
   docker compose up -d
   ```

4. **Verify:**
   - Check logs: `docker compose logs -f schwarbot`
   - Verify bot is trading
   - Check database encryption:
     `sqlite3 /mnt/volume_nyc3_01/schwab.db "SELECT length(access_token) FROM schwab_auth;"`
     - Should return a number > 64 (encrypted data is larger than plaintext)

### Rollback Plan

If issues occur:

1. Revert code changes
2. Tokens are already encrypted in DB - will need to re-authenticate regardless
3. Remove `TOKEN_ENCRYPTION_KEY` requirement once rolled back

## Summary

This implementation encrypts Schwab OAuth tokens using AES-256-GCM. The
hard-switch approach requires re-authentication but simplifies the codebase. The
encryption key is managed via environment variables and GitHub Secrets,
providing a secure MVP solution that integrates with the existing deployment
pipeline.
