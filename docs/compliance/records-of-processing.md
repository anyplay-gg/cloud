# Record of processing activities (draft)

A draft record under [GDPR Art. 30(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679), built from the [data inventory](data-inventory.md). It is a draft for legal review, not legal advice ([README](README.md)). Every operator of a cloud fills in the placeholders and keeps its own copy.

## Do we need one?

[Art. 30(5)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) frees organisations with fewer than 250 persons, unless the processing is likely to risk people's rights, is "not occasional", or includes special-category data. A cloud processes accounts all the time, so the exemption does not apply (inference; the lawyer to confirm). Keep the record in writing, electronic form is fine (Art. 30(3)), and show it to the supervisory authority on request (Art. 30(4)).

## Controller (Art. 30(1)(a))

| Field | Value |
|---|---|
| Controller | `[OPERATOR NAME]`, `[LEGAL FORM: volunteer project / association / company; not yet decided]` |
| Address | `[OPERATOR ADDRESS]` |
| Contact | `[PRIVACY E-MAIL]` |
| Joint controllers | None known. Each operator runs its own cloud; AnyPlay as a project does not see an independent operator's data (inference). |
| Representative in the EU | Not needed if the operator is established in the EU ([Art. 27](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) applies to controllers outside it). |
| Data protection officer | `[NONE / NAME]`. [Art. 37(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) requires one for public bodies, large-scale regular and systematic monitoring, or large-scale special-category data as a core activity. We do not think the cloud meets those (inference; open question). |

## Processing activities

Each activity lists the items required by Art. 30(1)(b) to (g): purposes, categories of people and data, recipients, transfers outside the EU, erasure time limits and security measures. "Proposed" retention is ours; the [inventory](data-inventory.md) states what the code does today.

### P1. Accounts and sign-in

| Item | Content |
|---|---|
| Purpose (b) | Create and run a player's account; sign players in with an e-mail code; keep devices signed in. |
| Legal basis | Art. 6(1)(b) (contract). Theft check on refresh tokens: Art. 6(1)(f). |
| People (c) | Players, including minors who meet the age rule in the [terms](https://github.com/anyplaynet/web/blob/main/apps/web/src/components/legal/terms.tsx). |
| Data (c) | E-mail address, handle, account ID, creation time; device name, platform, refresh token hash, creation and last use; pending sign-in codes (hashed) with the address. |
| Recipients (d) | The mail provider (processor) gets the address and the code. The hosting provider (processor) holds the database. `[PROCESSORS]` |
| Transfers (e) | None planned. All servers in the EU. `[CONFIRM FOR EACH PROCESSOR AND SUB-PROCESSOR]` |
| Erasure (f) | Codes: 10 minutes; permits: 15 minutes. Devices: until sign-out or account deletion; proposed 90 days of disuse. Account: until deletion; proposed 24 months of inactivity. |
| Security (g) | Passwordless sign-in; only hashes of codes, tokens and challenge IDs stored; code allows 5 tries; rate limits; TLS at the proxy; access tokens live 15 minutes and refresh tokens change on every use. The e-mail address itself is stored in plain text. `[DISK ENCRYPTION, ACCESS CONTROL]` |

### P2. Friends, blocks and presence

| Item | Content |
|---|---|
| Purpose | Friend lists, blocking, showing friends who is online and what they play. |
| Legal basis | Art. 6(1)(b). |
| People | Players. |
| Data | Friendships and requests with times; blocks; online state, game title ID, title and version, joinable flag, last-seen time. |
| Recipients | Other players: friends see presence. Hosting provider. |
| Transfers | None. |
| Erasure | Until removed by a player or until the account is deleted. Presence: overwritten, one row per account. |
| Security | Access only through the signed-in API; presence goes only to friends, and not at all while the player appears offline; blocks are never shown to the blocked person. |

### P3. Sessions, invitations and tunnels

| Item | Content |
|---|---|
| Purpose | Let players join each other's game sessions and connect their devices. |
| Legal basis | Art. 6(1)(b). |
| People | Players in a session. |
| Data | Handle, tunnel endpoint ID, relay URL, virtual address, game title and version, opaque game data; invitations. |
| Recipients | Other members of the session. The relay sees IP addresses and the account ID in the ticket when a player uses it. |
| Transfers | None. |
| Erasure | Member: 30 seconds after the last connection closes; session: when the last member leaves; invitations: 15 minutes; virtual address: with the member. |
| Security | Relay tickets are signed by the cloud and checked offline by the relay; game traffic is end-to-end encrypted. |

### P4. Relay operation

| Item | Content |
|---|---|
| Purpose | Carry encrypted game traffic when a direct connection is not possible; stop abuse. |
| Legal basis | Art. 6(1)(b) for carrying traffic; Art. 6(1)(f) for abuse limits. |
| People | Players who use a relay. |
| Data | IP address and port while connected; endpoint ID; ticket (with account ID). Not written to disk by our code. |
| Recipients | The relay operator. |
| Transfers | None if relays run in the EU (research note: data, logs and backups stay in the EU). `[RELAY LOCATIONS]` |
| Erasure | End of the connection. Any operator log: not more than 7 days (proposed). |
| Security | Ticket check; per-client rate limit; TLS. |

### P5. Abuse prevention (rate limits and security logs)

| Item | Content |
|---|---|
| Purpose | Stop floods and code guessing; detect stolen tokens; keep the service running. |
| Legal basis | Art. 6(1)(f). |
| People | Everyone who calls the API. |
| Data | IP address and e-mail hash in memory counters; account and device IDs in warning logs. |
| Recipients | Operator staff with log access. |
| Transfers | None. |
| Erasure | Counters: at most 1 hour, gone at restart. Logs: proposed 30 days; the code sets none today. |
| Security | Logs hold no IP address or token in the default setting. `[LOG ACCESS]` |

### P6. Reports, moderation and bans

| Item | Content |
|---|---|
| Purpose | Handle reports of abuse, remove people who break the rules, stop them returning. |
| Legal basis | Art. 6(1)(f); for notices of illegal content also Art. 6(1)(c) where the operator must act ([dsa.md](dsa.md)). |
| People | Reporters, reported people, banned people. |
| Data | Report (both account IDs and handles, reason, free text, snapshot, time, decision); action (account, rule, reason, moderator, time); appeal text and decision; ban (account ID, keyed hash of the e-mail address, end date); the audit log of every step. |
| Recipients | Moderators. Authorities when the law requires it (DSA Art. 9, 10, 18; [dsa.md](dsa.md)). |
| Transfers | None. |
| Erasure | Decided reports, temporary bans, mutes, warnings and appeals 6 months after the decision or end; undecided reports 1 year; the audit log 3 years; bans for good until lifted. Settings in [moderation.md](../moderation.md). |
| Security | Admin routes exist only with a token of 16 or more characters and are rate limited. One shared token; no audit log (gap G8). |

### P7. Data subject requests and deletion

| Item | Content |
|---|---|
| Purpose | Answer access, portability and erasure requests ([Art. 12 and 15 to 21](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)). |
| Legal basis | Art. 6(1)(c). |
| People | Players. |
| Data | The export JSON (built on request, not stored); a request log `[PROPOSED: date, kind, outcome; no content]`. |
| Recipients | The requester. |
| Transfers | None. |
| Erasure | Request log: 3 years (suggestion; the lawyer to confirm). |
| Security | Export and deletion need a signed-in device. |

### P8. Backups

| Item | Content |
|---|---|
| Purpose | Restore the service after a failure. |
| Legal basis | Art. 6(1)(f); Art. 32(1)(c) asks for the ability to restore data "in a timely manner". |
| Data | A copy of the whole database. |
| Recipients | Hosting or backup provider (processor). |
| Erasure | Not defined (gap G10). Proposed: 30 days, and a restore must replay deletions. |
| Security | `[ENCRYPTED, EU, ACCESS LIST]` |

### P9. In flight: trophy records, profiles, messages, feed

Not in the code yet (see the [inventory](data-inventory.md#in-flight-social-layer)). Add one activity per feature when it lands. Messages need their own entry, purpose, retention and moderator access rule before release.

## Security measures, general (Art. 30(1)(g))

A general description in the sense of [Art. 32(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679). From the repository: containers run as non-root with a read-only root file system (ARCHITECTURE, "Security"); TLS by the proxy and the relay; hashes for codes and tokens; tokens signed with one key, kept in `data_dir`; rate limits on every route; nightly fuzzing of the token check. To fill in by the operator: encryption at rest, access control and logging for staff, backup encryption, restore tests.

## Transfers outside the EU (Art. 30(1)(e))

None planned. Operators must check each processor and each sub-processor ([Art. 28(2)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) lets a processor use another only with the controller's authorisation). Outside the EU a transfer needs [Chapter V](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): an adequacy decision (Art. 45) or safeguards (Art. 46). Mail providers with a US parent are the likely trap.

## Record as processor (Art. 30(2))

If AnyPlay ever runs a cloud for another operator, it becomes a processor and needs a record under Art. 30(2). Not applicable today.
