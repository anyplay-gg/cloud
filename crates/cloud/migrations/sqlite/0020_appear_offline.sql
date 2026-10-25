-- Appearing offline (network ADR 0028 and spec/cloud-api.md, "Appearing offline"): 1 when friends
-- see the account as offline although it is signed in.
ALTER TABLE accounts ADD COLUMN appear_offline INTEGER NOT NULL DEFAULT 0;
