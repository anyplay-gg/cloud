# Terms of use (draft)

> **Draft for legal review. Not legal advice.** Text in `[BRACKETS]` is a placeholder. A statement marked *not built yet* describes something the cloud does not do today; the change is listed in [dsa.md](dsa.md) or the [data inventory](data-inventory.md). German consumer and contract law (for example the rules on general terms and on minors' contracts) was **not** checked for this draft; see [open-questions.md](open-questions.md).

The shape follows [DSA Art. 14(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065): the terms must state the restrictions on what users provide, the moderation policies, procedures and tools (including human review), and the rules of the complaint system, in clear and plain language. Art. 14(3) asks for an explanation minors can understand when minors use the service. A short summary comes first for that reason.

---

# Terms of use for `[CLOUD NAME]`

Last changed: `[DATE]`. Version: `[VERSION]`.

## In short

- You need to be at least `[16]` years old.
- Play only games you own. Do not use cheating tools. Do not build or use anything that pretends to be a game publisher's server.
- Be decent to other players. We remove people who are not.
- If we limit your account, we tell you why, and you can appeal.
- You can delete your account at any time.

## 1. Who we are

`[OPERATOR NAME]`, `[LEGAL FORM]`, `[ADDRESS]` ("we") run `[CLOUD NAME]`, an online service for people who play with AnyPS5. Contact for any question about these terms or about content: `[USER CONTACT E-MAIL]` (a person reads it; DSA Art. 12). Contact for authorities: `[AUTHORITY CONTACT E-MAIL]` (DSA Art. 11), in `[LANGUAGES]`. `[IMPRINT DATA IF THE SERVICE IS COMMERCIAL: DDG § 5]`

## 2. The service

The service lets you sign in, add friends, see who is online and what they play, start and join game sessions, and connect your PC to other players through encrypted tunnels. It is free. We can change it. `[We can stop it with `[NOTICE]` days' notice.]`

## 3. Age

- You must be at least **`[16]`** years old to open an account. When you sign up you confirm that you are old enough. We do not ask for your date of birth or an ID, and we do not check.
- **Why 16 (our reasoning, for the lawyer).** [GDPR Art. 8(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) sets the age at which a child can give consent for an online service at 16 and lets Member States lower it, but not below 13. Our legal basis for the core service is the contract, not consent (inference, see the [data inventory](data-inventory.md)), so Art. 8 may not apply directly. Whether a contract with a person under 18 is valid without a parent is a question of national civil law that we did not check. We chose 16 because it is the GDPR default, it is the age the network's own API specification already names ("16 in the EU unless a country sets a lower age", network `spec/cloud-api.md`), and it is easy to explain. We did not find a German rule lowering the Art. 8 age; the lawyer should confirm.
- **Why only self-declared.** The European Data Protection Board asks for risk-based and least-intrusive age assurance, and [DSA Art. 28(3)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065) says a platform need not collect more personal data just to find out a user's age (the research notes quote the EDPB position; we did not re-read the EDPB text). The cloud does not need to know who is a child to run the service, so it asks once and keeps safe defaults for everyone ([network ADR 0012](https://github.com/anyplay-gg/network/blob/main/docs/adr/0012-private-by-default.md)).
- If we learn that someone is under the age, we close the account and delete its data (the bans excepted). A parent can ask us to do it at `[PRIVACY E-MAIL]`.
- *Not built yet:* the cloud stores no record of the confirmation or of the terms version (gap G9).

## 4. Your account

- You give a real e-mail address that you control. You keep your handle and sign-in devices to yourself. Anyone with access to your e-mail can sign in as you.
- One person, one account. Handles cannot copy a staff or platform name; the cloud blocks some names (for example `admin`, `moderator`, `playstation...`).
- You can delete the account at any time in the launcher. We delete your data as the [privacy notice](privacy-notice.md) says.

## 5. Rules for your use (the clean line)

These rules follow the network's published rules ([network ADR 0003](https://github.com/anyplay-gg/network/blob/main/docs/adr/0003-the-clean-line.md)). You promise to follow them.

1. **Only games you own.** Use the network only with games you legally own. Do not share, sell or use dumps, keys or accounts that are not yours.
2. **No cheating tools.** Do not use tools that change a game to give you an unfair advantage or that bypass a game's anti-cheat or account checks. Do not use the network to harm another player's game.
3. **No stand-in for a publisher's servers.** Do not use the service, or write software that uses its interfaces, to pretend to be a game publisher's or a platform owner's servers, or to hand out tokens or logins for someone else's client.
4. **No Sony servers, code or leaked documents.** Do not send us material taken from a platform owner's servers, firmware or leaked documents. Do not post it to other players.
5. **Be decent.** No harassment, hate, threats, sexual content involving minors, spam or impersonation. No illegal content, as defined by the law of the EU or of a Member State.
6. **Keep the service working.** No attacks, no scraping, no automated sign-up, no attempt to get around limits or bans (including a new account after a ban).
7. **Names.** Your handle must not be offensive or pretend to be someone else.
8. **Game data.** Do not put personal data of other people into the game data fields that sessions carry.

Software made for the network (the launcher and the net service) is open source. Its licence governs how you may use and change the code. These terms govern your use of this cloud.

## 6. What we do about rule breaks

- **What we look at.** We act on reports from players, notices from anyone (including people without an account), orders from authorities, and what we find ourselves. `[We do not scan messages or traffic.]` The relay cannot read game traffic.
- **Who decides.** A person decides. The cloud applies automatic limits (waiting times on sign-in and reports) but no automatic bans. *(That matches the code today.)* Our moderators work only from the report and the facts around it.
- **What we may do.** In order of weight: ask you to stop; warn you; limit a feature; remove a piece of content (once content that others can see exists); suspend your account for a time; close your account for good. We choose the lightest step that works. We act in a "diligent, objective and proportionate manner", as DSA Art. 14(4) requires.
- **What a ban does.** It signs out all your devices, takes you out of your sessions and stops the same e-mail address from signing up again. We keep a record of the ban, for the time in the [privacy notice](privacy-notice.md), so that the ban holds if you delete the account.
- **What we tell you.** When we limit or close your account, or remove something of yours, we send a message to your e-mail address and show it in the launcher. It says what we did, for how long, why (the facts, whether a report started it, which rule), whether any automatic means were used, and how to appeal ([DSA Art. 17(3)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065)). *Not built yet: today the cloud shows only "banned until" and keeps the reason private. Task T4 in [dsa.md](dsa.md).*
- **We may not say who reported you** unless the law requires it.
- **Reports in bad faith.** If someone files many reports that are plainly unfounded, we may ignore their reports for a time, after a warning.

## 7. Appeal

- You can appeal any step in section 6 within **6 months** of the message, free of charge, by `[APPEAL FORM / E-MAIL]` (DSA Art. 20 sets six months for platforms that are not micro or small; we use it voluntarily). *Not built yet: task T5 in [dsa.md](dsa.md).*
- Tell us why you think the step is wrong.
- A person who was not involved in the first decision, where we can manage it, looks again and answers within `[14]` days. If the step was wrong, we undo it at once.
- If you still disagree, you can use an out-of-court dispute body certified under DSA Art. 21 `[NAMES, IF ANY]`, ask a data protection authority about your data, or go to court. We tell you in the answer.

## 8. Reporting content

You can report a player in the launcher. Anyone can send a notice about content they think is illegal at `[NOTICE FORM URL]`. Please give the reasons, exactly where it is, your name and e-mail address, and confirm you believe the notice is accurate ([DSA Art. 16(2)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065)). We confirm that we got it and tell you what we decided and how to challenge it (Art. 16(4), (5)). *Not built yet: the notice form, receipt and answer are tasks T1 and T2 in [dsa.md](dsa.md).*

## 9. Your content and our rights

You keep what is yours. You give us the right to store and show your handle, your presence and the other data that the service needs, to the people you choose, for as long as your account exists. `[LICENCE TEXT FOR PROFILE, AVATAR AND MESSAGES WHEN THEY EXIST]`

## 10. Liability and changes

`[LIABILITY, WARRANTY AND GOVERNING LAW: FOR THE LAWYER. Do not copy a template without checking German general-terms law.]`

We tell you in the launcher before a significant change takes effect (DSA Art. 14(2)). If you do not agree, you can delete your account.

## 11. Contact

`[USER CONTACT E-MAIL]`, `[POSTAL ADDRESS]`.
