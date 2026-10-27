-- The deletion ledger: which accounts were deleted, and when, and nothing else.
--
-- A backup brings back accounts that were deleted after it was made. After a restore the cloud
-- deletes again what this ledger lists (`anyplay-cloud replay-deletions`, deploy/restore.sh). A
-- row holds the random account ID and the time of the deletion, no handle, address or other
-- data. The sweep removes a row after 90 days, the longest a backup is kept.
CREATE TABLE deleted_accounts (
    account_id INTEGER PRIMARY KEY,
    deleted_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX deleted_accounts_time ON deleted_accounts (deleted_ms);
