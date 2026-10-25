# EU compliance groundwork

Groundwork for anyone who runs an AnyPlay cloud in the EU. They cover the GDPR (Regulation (EU) 2016/679) and the Digital Services Act (Regulation (EU) 2022/2065). They are written from what the code stores and logs.

> **This is not legal advice, and no lawyer has read it.** The notes below (inventory, record, DSA, open questions) are drafts written from the code. The privacy notice and the terms of use are final texts, published on 25 October 2026 at https://anyplaynet.com/privacy and https://anyplaynet.com/terms. The operator decided to publish them without a lawyer's review and under the name "AnyPlay", with no person's name or address. Their one source is the website repository: [privacy.tsx](https://github.com/anyplay-gg/web/blob/main/apps/web/src/components/legal/privacy.tsx) and [terms.tsx](https://github.com/anyplay-gg/web/blob/main/apps/web/src/components/legal/terms.tsx). Change the text there, in the same pull request as any change to the code it describes, and raise the version and date in `meta.ts` and `TERMS_VERSION`. Statements marked "inference" are ours, not the law's.

## Where the law comes from

The documents cite the article and paragraph and link the text: [GDPR](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679), [DSA](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065), [Cyber Resilience Act](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847), [TDDDG § 25](https://www.gesetze-im-internet.de/ttdsg/__25.html), [DDG § 5](https://www.gesetze-im-internet.de/ddg/__5.html). The research behind the scope is in the workspace notes ("Skalierbare Architektur für AnyPlay", German). Where a text could not be read in full, the document says so.

## The documents

| Document | What it is | Status |
|---|---|---|
| [data-inventory.md](data-inventory.md) | Every kind of personal data the cloud and relay store or log, where, why, how long, who sees it, and the gaps in the code. | Draft, from the code at the base commit |
| [records-of-processing.md](records-of-processing.md) | A record of processing activities under GDPR Art. 30. | Draft, operator fills the placeholders |
| [privacy notice](https://github.com/anyplay-gg/web/blob/main/apps/web/src/components/legal/privacy.tsx) | The privacy notice for players (Art. 12 to 14). | Published, version 1 |
| [terms of use](https://github.com/anyplay-gg/web/blob/main/apps/web/src/components/legal/terms.tsx) | Terms of use: clean-line duties, moderation, appeal, age. | Published, version 1 |
| [dsa.md](dsa.md) | The DSA category, the duties of a small provider, and the engineering tasks. | Draft |
| [open-questions.md](open-questions.md) | What the lawyer must answer, by risk. | Draft |

## Checklist: before an operator opens a public cloud

"Status" is what exists in the repository today. "Draft" means a draft is in this folder. "Code" means the cloud has to change first.

| # | Needed | Source | Status |
|---|---|---|---|
| 1 | Operator identity, legal form, address, contact (imprint if commercial) | GDPR Art. 13(1)(a); DSA Art. 11, 12; DDG § 5 | **Decided:** the operator is named "AnyPlay"; the contact is one e-mail address; no person or street address is published. Whether that meets DDG § 5 and GDPR Art. 13(1)(a) for this service is not settled. |
| 2 | Lawyer has read all documents here and answered [open-questions.md](open-questions.md) | | **Open** |
| 3 | Privacy notice, published at `privacy_url` | GDPR Art. 12 to 14 | **Published** (anyplaynet.com/privacy) |
| 4 | Terms of use, published at `terms_url`, machine-readable | DSA Art. 14 | **Published** (anyplaynet.com/terms) |
| 5 | Record of processing, filled in | GDPR Art. 30 | Draft |
| 6 | Data protection impact assessment decision, and one if needed | GDPR Art. 35 | **Open.** Question O3. |
| 7 | Processor contracts: mail provider, hosting, backup, relay host | GDPR Art. 28 | **Open.** Question O6. |
| 8 | Breach process: who decides, 72-hour notice to the authority, notice to players | GDPR Art. 33, 34 | **Open.** Question O7. |
| 9 | Point of contact for authorities, and for players, published | DSA Art. 11, 12 | **Open** (no page, no address) |
| 10 | Notice form for illegal content, receipt and decision | DSA Art. 16 | **Code.** Tasks T1, T2 in [dsa.md](dsa.md). |
| 11 | Statement of reasons for bans | DSA Art. 17 | **Built** for the app and the API (task T4, [moderation.md](../moderation.md)). **Code:** the mail. |
| 12 | Appeal route | DSA Art. 20 (voluntary), Art. 17(3)(f) | **Built** (task T5). |
| 13 | Moderation audit log, per-person admin access | DSA Art. 9, 10; GDPR Art. 5(2) | **Built** for moderation: audit log and moderator accounts (task T6). **Open:** the log of orders from authorities. |
| 14 | Retention rules in force: bans, reports, devices, inactive accounts, logs | GDPR Art. 5(1)(e) | **Partly.** Bans, reports, actions, appeals and the audit log are swept (G1, G2 closed). **Code:** devices and inactive accounts (G3), log retention (G5). |
| 15 | No e-mail addresses or codes in logs: `mail = smtp`, log rotation, relay and proxy log settings | GDPR Art. 5(1)(c), 32 | **Code and configuration.** Gaps G4, G5. |
| 16 | Age and terms version recorded at sign-up | GDPR Art. 7(1) if consent, Art. 5(2); DSA Art. 14(2) | **Done for new accounts** (migration 0008, account web, `terms_version`). Gap G9 keeps the question of older accounts. |
| 17 | Access and deletion work for all stored data, backups included | GDPR Art. 15, 17, 20 | **Partly.** Gaps G6, G7, G10. |
| 18 | Procedure for threats to life and orders from authorities | DSA Art. 9, 10, 18 | **Open.** Task T7. |
| 19 | Launcher: local storage and the privacy text | TDDDG § 25; GDPR Art. 13 | **Open.** Question O9. |
| 20 | Chat and voice: youth-protection duties | JMStV, JuSchG | **Open.** Question O2. Needed before messages and voice ship. |
| 21 | Cyber Resilience Act position of the launcher | CRA | **Open.** Question O13. |
| 22 | Data, logs and backups in the EU, confirmed per provider | GDPR Chapter V | **Open** |

## How the documents relate

The inventory is the base. The record, the privacy notice and the terms are built from it and name the same retention periods. Where the code does something different from what a draft says, the draft carries the marker "NOT IMPLEMENTED YET" or "not built yet" and the inventory lists the gap with an ID (G1 and so on). The DSA tasks (T1 and so on) are in [dsa.md](dsa.md).

## Status of the social layer

Trophy records (migration 0003), profiles, messages and the feed are not in the code at the base commit. The inventory lists what to add when they land, following the network decisions ([ADR 0009 to 0012](https://github.com/anyplay-gg/network/tree/main/docs/adr)). Messages change the risk picture most.

## For contributors

- Change the inventory in the same pull request as any change to a table, a log line or a retention rule.
- Keep numbers (minutes, days) the same across documents. The inventory is the source.
- Cite the article and paragraph, and link the text. Mark your own inferences. Do not invent the operator's details.
