# Privacy notice (draft)

> **Draft for legal review. Not legal advice.** Text in `[BRACKETS]` is a placeholder the operator must fill in. Where the code and this text disagree, the code is described and the change is a gap in the [data inventory](data-inventory.md). Do not publish before a lawyer has read it ([README](README.md)).

The content follows [GDPR Art. 13(1) and (2)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679), because we collect the data from you. The language follows [Art. 12(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): concise, plain, and understandable for a child, since young people play (see [Recital 58](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)). A launcher shows this text, or a link to it, before sign-up (`privacy_url` in the [configuration](../configuration.md)).

---

# How `[CLOUD NAME]` handles your data

Last changed: `[DATE]`. Version: `[VERSION]`.

## Who is responsible

`[OPERATOR NAME]`, `[LEGAL FORM]`, `[ADDRESS]`, runs this cloud. You can write to us at `[PRIVACY E-MAIL]`. `[DATA PROTECTION OFFICER, IF ANY]`

## The short version

- We keep what we need to run your account, your friends and your game sessions. Nothing else.
- We do not show ads. We do not sell data. We do not use analytics or tracking tools. `[CONFIRM BEFORE PUBLISHING]`
- We never see your game files, saves or what happens inside a game.
- You can download your data and delete your account in the app at any time.
- Your data stays on servers in the EU. `[CONFIRM FOR EACH PROVIDER]`

## What we keep, why, and for how long

| What | Why we need it | Our legal basis | How long |
|---|---|---|---|
| **E-mail address.** | To send you a sign-in code and to find your account. | Contract: you asked for an account (Art. 6(1)(b)). | Until you delete your account. `[If you do not sign in for 24 months we delete it. NOT IMPLEMENTED YET]` |
| **Handle** (your public name) and the **account number** we give you. | So your friends can find you. Other players see it. | Contract (Art. 6(1)(b)). | Until you delete your account. |
| **Sign-in codes.** | To check the code you type. We store only a scrambled form (a hash) with your e-mail address. | Contract (Art. 6(1)(b)). | 10 minutes. |
| **Your devices:** the name your computer gives, the system (Windows, Linux or macOS), and when you last signed in. | To keep you signed in, and to spot a stolen sign-in. | Contract; our interest in security (Art. 6(1)(b) and (f)). | Until you sign the device out or delete your account. `[90 days without use. NOT IMPLEMENTED YET]` |
| **Friends, friend requests and blocks.** | For the friend list. A block is private; the other person is not told. | Contract (Art. 6(1)(b)). | Until you remove it or delete your account. |
| **Presence:** whether you are online, which game you play and whether others can join. | So your friends see it. | Contract (Art. 6(1)(b)). | Only the latest state is kept, until you delete your account. |
| **Game sessions:** your handle, a technical address of your connection setup (an endpoint ID, not your IP address), and your game title and version. | So players can join the same game. | Contract (Art. 6(1)(b)). | Until you leave the session, 30 seconds after you disconnect. |
| **Your IP address.** | The server must see it to answer you. We use it in memory to stop abuse, for example someone guessing codes. We do not write it into our logs or database. | Our interest in a safe service (Art. 6(1)(f)). | At most 1 hour, then it is gone. |
| **Reports you send or that name you.** | To stop abuse and keep players safe. | Our interest (Art. 6(1)(f)); the law, where it asks us to act on illegal content (Art. 6(1)(c)). | `[6 months after we decide, or 12 months after you send it if nobody decided. The report stays if the reporter or the reported person deletes their account, and the reporter is then forgotten.]` |
| **Bans:** your account number, a hash of your e-mail address, the end date and the reason. | To enforce a ban, also if you come back with a new account and the same address. | Our interest (Art. 6(1)(f)). | `[Time-limited bans: 6 months after they end. Bans for good: until we lift them. The statement and appeal: 6 months after the action ends. The record of who decided what: 3 years.]` |
| **Server logs.** | To find faults. They do not hold e-mail addresses, IP addresses or codes. | Our interest (Art. 6(1)(f)). | `[30 days. NOT SET IN THE CODE YET]` |

`[WHEN TROPHIES, PROFILES, MESSAGES AND THE FEED ARE RELEASED, ADD THEM HERE WITH THEIR RETENTION. Trophy records hold the game number, the trophy number, the grade and the time; they are visible to friends only unless you open them.]`

## What your computer sends

The launcher and the net service on your PC send the cloud: your sign-in token, the device name and system, the game you are playing, and the data needed to connect to other players. They keep your sign-in token in your system's credential store, or in a protected file when your system has none. They do not upload your game files or saves. If you turn on *Record Network Calls*, the log stays on your PC unless you share it yourself.

The launcher also asks GitHub for updates and recommended settings. GitHub sees your IP address when it does. `[LAUNCHER PRIVACY TEXT; CHECK § 25 TDDDG]`

## Relays

When your PC cannot reach another player directly, your game traffic goes through a relay run by `[RELAY OPERATOR]`. The traffic is encrypted end to end, so the relay cannot read it. It does see your IP address while you are connected. `[We do not log it. / We keep relay logs for N days.]` You can switch on relay-only mode in the launcher so that other players never see your IP address.

## Who gets your data

- **Other players** see your handle, your presence and the session data above, as described.
- **Service providers** that work for us under a contract (called processors): `[MAIL PROVIDER, NAME, COUNTRY]` sends your sign-in codes and sees your e-mail address and the code; `[HOSTING PROVIDER, NAME, COUNTRY]` runs the servers. `[BACKUP PROVIDER]`
- **Authorities,** only when the law requires it, for example a court order, and when we must report a threat to someone's life (DSA Art. 9, 10 and 18; see [dsa.md](dsa.md)).

We do not send your data outside the EU. `[CONFIRM. IF NOT: name the country and the safeguard, as Art. 13(1)(f) requires.]`

## Do you have to give us this data?

To have an account we need your e-mail address and a handle. Without them we cannot create it. Everything else follows from using a feature: if you do not want presence, do not start a game while signed in. (Art. 13(2)(e).)

## Your rights

You have these rights under the GDPR ([Art. 15 to 21](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)):

- **Access** (Art. 15) and **a copy in a machine-readable format** (Art. 20): through the launcher `[MENU PATH; THE API CALL IS GET /v1/me/export]`. The file holds your account, devices, friends, requests, blocks, your reports and your sessions. `[It does not yet hold presence. NOT IMPLEMENTED YET]`
- **Correction** (Art. 16): you can ask us to fix wrong data.
- **Erasure** (Art. 17): through the launcher `[MENU PATH; THE API CALL IS DELETE /v1/me]`. We delete your account, devices, friends, requests, blocks, reports, presence and sessions at once. We keep a ban if you were banned, for the time set above. Backups are overwritten within `[30]` days. `[NOT IMPLEMENTED YET: the delete flow does not ask for a second confirmation]`
- **Objection** (Art. 21(1)): where we rely on our interest, you can object for reasons that concern your situation. We then stop unless we show strong reasons that override yours, or we need the data for legal claims.
- **Restriction** (listed in Art. 13(2)(b)): you can ask us to pause the use of your data in the cases the law lists.
- **Complaint**: you can complain to a data protection authority. In Germany that is the authority of your state or of ours: `[AUTHORITY, ADDRESS]`.

We answer within one month ([Art. 12(3)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)), free of charge. If we need to confirm that you are you, we may ask you to answer from the e-mail address of the account (Art. 12(6)). If we say no, we tell you why and how to complain (Art. 12(4)).

## Automated decisions

Nobody is banned by a computer alone. Rate limits are automatic and short: they ask you to wait. (Art. 22 does not apply. `[CONFIRM; the code has no automatic bans]`)

## Children and young people

You must be at least `[16]` to use this cloud on your own. See the [terms](terms.md), section "Age". We do not ask for your date of birth. We ask you to confirm your age once, at sign-up. `[We store the date of that confirmation. NOT IMPLEMENTED YET]` We do not use your data for ads or profiles. If you are under the age, tell a parent; they can write to us and we delete the account.

## Changes

If we change this text in an important way, we tell you in the app before it takes effect.
