# Moderation

For operators of a cloud. It describes what the cloud does when players report each other, how moderators work, what a player sees, and how long the data lives. The API is in the [network spec](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md#moderation); the decision behind the model is [network ADR 0015](https://github.com/anyplaynet/network/blob/main/docs/adr/0015-moderators-are-accounts.md). The legal background is in [compliance/dsa.md](compliance/dsa.md). Whether these settings and periods are right for you is a question for your lawyer.

## The model in short

1. A player **reports** an account. The report keeps a snapshot of what was reported.
2. A **moderator** reads the queue and either **dismisses** the report or takes an **action** with a rule and a reason: a **warning**, a **mute** (no friend requests, and later no messages, for some days) or a **ban** (temporary or for good).
3. The player gets a **statement of reasons**. They may **appeal** once per action. A different moderator decides when there is one.
4. Every step is a line in the append-only **audit log**.
5. Old data is deleted by the sweep once its **retention** period ends.

## Who moderates

A moderator is a normal account that the operator gave the moderator role. Moderators use the same sign-in as players and call `/v1/moderation/...` with their access token. The role is read from the database on every request, so taking it back works at once.

The **admin token** (`admin_token`) is the bootstrap. It is one shared secret with no name, so use it to name the first moderators and then keep it for emergencies. Whatever it does is logged under the name `operator`. It also keeps the older `/admin/v1/...` routes and the `anyplay-cloud admin` commands working.

```text
# Name a moderator (the account must exist: they sign up like any player)
anyplay-cloud admin grant-moderator <handle>
anyplay-cloud admin moderators
anyplay-cloud admin revoke-moderator <handle>
```

Rules the cloud enforces:

- Only the operator grants or takes back the role.
- A moderator cannot act on themselves, and cannot act on another moderator. Only the operator can.
- A moderator cannot decide an appeal about themselves.
- When a moderator took an action, a different moderator decides its appeal as long as there is one. The operator counts as a separate person from every moderator. With no other moderator the same person may decide, and the audit log shows it.

## The queue and the actions

The commands below call the cloud on this machine with the admin token. Moderators use the same operations through the API (`GET /v1/moderation/reports` and so on), for example from a moderation tool that signs in as them.

```text
anyplay-cloud admin reports [--status open|decided|all] [--limit N]
anyplay-cloud admin dismiss <report-id> --reason TEXT
anyplay-cloud admin warn <handle> --reason TEXT [--rule RULE] [--report ID]
anyplay-cloud admin mute <handle> --days N --reason TEXT [--rule RULE] [--report ID]
anyplay-cloud admin ban  <handle> --reason TEXT [--days N] [--rule RULE]
anyplay-cloud admin unban <handle>
anyplay-cloud admin appeals
anyplay-cloud admin decide-appeal <appeal-id> [--reverse] --reason TEXT
anyplay-cloud admin log [--limit N]
```

- `RULE` is `cheating`, `harassment`, `spam`, `name`, `illegal` or `other`. It is the rule of your terms that the action rests on.
- The **reason** is written for the player: they read it word for word. Do not put the reporter's name, other players' private data or internal notes in it. It has 1 to 500 characters and is required for every action.
- `--report` links the action to the report it decides. A report is decided once. Other reports about the same account stay open until someone dismisses them or acts on them.
- A ban signs the account out on every node at once, takes it out of its sessions and stops the same e-mail address from signing up, even after the account is deleted. Without `--days` it is for good.
- A mute stops friend requests and messages, also to friends. The player can still play, accept requests and read.

### What a report holds

The report keeps the reporter and the reported account with the handles they had, the reason, the reporter's text, the time, and a **snapshot of what was reported**: the reported handle and the account's creation time and, when the reporter names a message (`conversation` and `seq`), its text. The reporter must be in that conversation and able to read the message, and the account reported must have written it. The snapshot stays when the message is swept by its retention or the account is deleted; that is its point, and the lawyer should know that it holds message text for as long as the report does. It does not vanish when an account is deleted:

- If the **reported** account is deleted, the report stays with its handle and ID so that the evidence and any ban remain, and the queue marks it `reported_deleted`.
- If the **reporter** deletes their account, the link and the handle are erased from their reports. The report stays.

## What the player sees

The **statement of reasons** (DSA Art. 17) tells the player what was done and for how long (`kind`, `until`), why (`reason`, `rule`), whether a report started it (`from_report`), whether software decided alone (`automated`, always `false`: the cloud never decides by itself), and how to appeal (`appeal.deadline`). It never names the reporter or the moderator.

| Action | How the player learns |
|---|---|
| Warning or mute | The event `action` on their connection, if they are online; always `GET /v1/me/actions`. |
| Ban | The event `banned` with the statement, then the connection closes. Every later request, refresh and sign-in answers `banned` with the statement and an appeal token. At sign-in the statement appears only after the e-mail code is entered, so someone who merely knows an address learns nothing. |
| Appeal decided | The event `action` with the new statement, and `appeal.answer`. |

The cloud sends no e-mail about an action. The statement is shown in the launcher, which has to make it visible; see "Open points" in [compliance/dsa.md](compliance/dsa.md).

A banned person cannot sign in, so the `banned` error carries an **appeal token**, valid for seven days, that reads their statements (`GET /v1/me/actions`) and files the appeal (`POST /v1/me/actions/{id}/appeal`). It can do nothing else. A ban made before this version has no statement; its owner still gets the token and can write to you. An appeal can be filed until `appeal_days` after the action (183 days), once per action, and is limited to 10 per hour per account.

## New and muted accounts

One rule for new accounts, `moderation::may_write_to_strangers`, and one setting, `new_account_hours` (24). A new account is younger than that.

- **Messages:** a new account cannot start a conversation with someone who is not its friend. The answer is `restricted` with `restriction: "new_account"` and `until`, the time it may. It can write to its friends at once. The message code calls the same function.
- **Friend requests:** a new account may send them, but at most `new_account_friend_requests_per_day` (10) while it is new. The next one answers `restricted` with the same `until`. Accepting a waiting request, or answering it with your own, does not count. The requests of new accounts are counted in `new_account_requests`, and old lines are swept.
- **Muted accounts** send no friend requests and no messages at all, also to friends. Receiving still works. The answer is `restricted` with `restriction: "muted"` and the end of the mute.

Set `new_account_hours = 0` to turn the age rule and the allowance off.

## Limits on reports

An account may file `report_limit_per_day` (20) reports in 24 hours and 5 in 10 minutes, and may have one open report about the same account at a time. These counts and the general rate limits are per account. The 10-minute window lives in memory of each node, like the other rate limits.

## Settings

All are in the table in [configuration.md](configuration.md#moderation).

## Retention

The sweep runs on every node about once an hour and deletes:

| What | Deleted | Default | Setting |
|---|---|---|---|
| A temporary ban, and its e-mail hash | After its end | 6 months (183 days) | `retention_bans_days` |
| A mute, an ended or a lifted action (a ban lifted, a mute lifted) with its appeal | After its end or lifting | 6 months | `retention_bans_days` |
| A warning with its appeal | After it was given | 6 months | `retention_bans_days` |
| A decided report | After the decision | 6 months | `retention_reports_days` |
| A report nobody decided | After it was filed | 1 year (365 days) | `retention_open_reports_days` |
| A line of the audit log | After it was written | 3 years (1095 days) | `retention_audit_days` |

- A **ban for good** is kept until a moderator lifts it, and for `retention_bans_days` after that. It is the one record the cloud keeps without an end, because it is the ban. Decide how often your moderators review them; the lawyer should set the review period ([compliance/open-questions.md](compliance/open-questions.md)).
- An action with an **open appeal** is kept until the appeal is decided.
- `retention_bans_days` may not be shorter than `appeal_days`, so that no warning goes before its appeal time ends. The cloud refuses to start otherwise.
- The audit log is written by the cloud and never changed; the database refuses an `UPDATE`. Only this sweep deletes lines. Because lines name players and carry the reason text, the period is a data-protection choice: keep it as short as your duty to show who did what allows.

## The e-mail hash of a ban

A ban keeps a hash of the banned person's e-mail address so that they cannot sign up again. The hash is HMAC-SHA-256 under a key that the database does not hold. The key is the `ban_secret` setting, or if that is unset a random key in `data_dir/ban.key`, made on first start (mode 0600 where the system has modes). Anyone who copies only the database cannot test guessed addresses against it.

- **Every node of a cluster must have the same key.** Share `ban.key` with `signing.key` in the data folder, or set `ban_secret` for all nodes. Different keys make a node miss bans made on another.
- **Back the key up with the database.** Without it the hashes of existing bans cannot be matched, and those people could sign up again.
- **Changing the key** has the same effect as losing it. Ban again if you must.
- **Bans made before this version** hold a plain SHA-256. At start the cloud rewrites those whose account still exists (it knows the address). For the others, whose account was deleted, the plain hash keeps matching until the ban ends or is deleted, and the first time the address shows up it is rewritten. There is no way to rewrite them sooner: the address is not stored.

## The audit log

Each line has the time, who did it (a moderator's account and handle, or `operator`), what happened (`warned`, `muted`, `banned`, `lifted`, `report_dismissed`, `appeal_filed`, `appeal_upheld`, `appeal_reversed`, `moderator_granted`, `moderator_revoked`), the account concerned with the handle it had, the reason, the rule, and the report, action or appeal it belongs to. A player who files an appeal appears as the actor of that line. Read it with `anyplay-cloud admin log` or `GET /v1/moderation/log`.

The older `tracing` line `account banned` is still written to the ordinary log with the account ID. It is not the record.
