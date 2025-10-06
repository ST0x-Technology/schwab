-- Add encryption version column to track encryption format
-- Version 1 = AES-256-GCM encrypted tokens
ALTER TABLE schwab_auth
ADD COLUMN encryption_version INTEGER NOT NULL DEFAULT 1
CHECK (encryption_version = 1);

-- Clear existing plaintext tokens (forces re-authentication after upgrade)
-- This is a hard switch to encrypted tokens with no backwards compatibility
DELETE FROM schwab_auth;
