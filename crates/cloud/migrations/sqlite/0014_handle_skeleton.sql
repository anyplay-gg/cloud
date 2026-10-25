-- Handles that look alike are one handle (network spec/cloud-api.md, "handle"): `handle_skeleton`
-- is `anyplay_api::handle_skeleton(handle)`, and a skeleton belongs to one account.
--
-- The backfill below repeats the steps of that function in SQL, in the same order: lower case,
-- drop `_` and `-`, `rn` to `m`, `vv` to `w`, `1` and `i` to `l`, `0` to `o`. A test checks that
-- both give the same skeleton.
--
-- Accounts that exist already may collide (`FIokzy` and `Flokzy`). The migration never fails
-- on them: the oldest account (by `created`, then by `id`) keeps the skeleton and the later ones
-- get NULL, so they keep signing in and keep their handle, and nobody new can take a look-alike
-- of any of them. NULL is allowed many times by the unique index.
ALTER TABLE accounts ADD COLUMN handle_skeleton TEXT;

UPDATE accounts SET handle_skeleton = replace(replace(replace(replace(replace(replace(replace(
    lower(handle), '_', ''), '-', ''), 'rn', 'm'), 'vv', 'w'), '1', 'l'), 'i', 'l'), '0', 'o');

UPDATE accounts SET handle_skeleton = NULL WHERE EXISTS (
    SELECT 1 FROM accounts older
    WHERE older.handle_skeleton = accounts.handle_skeleton
      AND (older.created < accounts.created
           OR (older.created = accounts.created AND older.id < accounts.id))
);

CREATE UNIQUE INDEX accounts_handle_skeleton_key ON accounts (handle_skeleton);
