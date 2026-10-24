-- Where a party member's net service can be reached, for party voice (network ADR 0017 and
-- spec/cloud-api.md, "Parties"). `endpoint` is "" until the member announces one. The three go
-- with the member's row, so leaving, being removed or the party ending clears them.
ALTER TABLE party_members ADD COLUMN endpoint TEXT NOT NULL DEFAULT '';
ALTER TABLE party_members ADD COLUMN relay TEXT;
ALTER TABLE party_members ADD COLUMN voice INTEGER NOT NULL DEFAULT 0;
