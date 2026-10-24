-- Virtual addresses belong to the members of a session, not to accounts (network ADR 0020).
-- A session never holds one address twice. The unique index is what stops two nodes admitting
-- two members with the same address at the same moment, if both missed the other's row.
--
-- The `leases` table of the old scheme stays, unused, until no node of an older version runs;
-- a later migration drops it.
CREATE UNIQUE INDEX members_address ON members (session_id, address);
