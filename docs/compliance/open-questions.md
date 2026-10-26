# Open questions for the lawyer

Ordered by risk, highest first. Each question says why it matters, what we found in the primary text and the code, and what we propose. These are drafts for legal review, not legal advice ([README](README.md)). "Inference" marks our own reading.

Texts we read for this pass: GDPR, DSA, the Cyber Resilience Act (CRA), TDDDG § 25, DDG § 5, the JMStV as in force since 1 December 2025, JuSchG §§ 10a, 10b, 24a, 24b, and the UK Online Safety Act sections named below. The research notes had listed these as not yet checked in primary text.

## O1. Minors: the age rule, contract capacity and parents

**Why it matters.** Many players are teenagers ([network ADR 0012](https://github.com/anyplaynet/network/blob/main/docs/adr/0012-private-by-default.md)). The cloud asks players to confirm they are old enough and stores nothing (gap G9).

**What the law says.**
- [GDPR Art. 8(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): where consent is the basis for an information society service offered directly to a child, the child must be at least 16, below that a parent must give or authorise it. Member States may lower the age, not below 13.
- [Art. 6(1)(f)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) weighs the interests of a data subject who is a child. [Recital 38](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) asks for specific protection of children's data.
- [Art. 17(1)(f)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): the right to erasure for data collected from a child under Art. 8(1).

**Questions.**
1. Is a contract with a 16-year-old (or any minor) valid without a parent under German civil law? We did not read it. If not, is the core service on Art. 6(1)(b) safe, or should the basis for minors be different?
2. Is 16 the right minimum age? We propose 16 ([published terms](https://github.com/anyplaynet/web/blob/main/apps/web/src/components/legal/terms.tsx)). Did Germany lower the Art. 8 age? We found no such rule but did not search the German statutes.
3. Is a self-declared age enough? Our basis: DSA Art. 28(3) says a platform need not collect more data to find out an age ([DSA](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065)), and the research notes quote the EDPB for risk-based, least-intrusive assurance. We did not re-read the EDPB text; the lawyer should check what the EDPB and the German authorities expect.
4. What do we do when we learn a player is under the age? The terms say: close the account, delete the data, on a parent's request.

## O2. Youth media protection for chat and voice (JMStV, JuSchG)

**Why it matters.** Messages, party chat and party voice are built (network `docs/SOCIAL.md`, phases S2 and S3). Contact functions are named as youth risks in German law.

**What the law says.**
- [JMStV](https://www.tlm.de/assets/uploads/general/DieTLM/Rechtsgrundlagen/JMStV_2025.pdf) § 5(1), sentence 2 now covers "Risiken durch Kommunikations- und Kontaktfunktionen" (risks from communication and contact functions) as a factor in age-appropriateness.
- JMStV § 2(2): it does not apply to intermediary services as in DSA Art. 3(g) "soweit" (as far as) the DSA applies. JMStV § 7(3): no youth protection officer for such services. (Text as read in the TLM consolidation of the 6th amendment.)
- [JuSchG § 24a](https://www.gesetze-im-internet.de/juschg/__24a.html): providers of services must take precautionary measures (report and remedy procedure with child-friendly guidance, information on help offers, parental controls, private defaults: profiles not findable by search services, communication limited to a chosen circle, pseudonymous use). The text names no user threshold and no exemption for non-commercial providers. [§ 10b(3)](https://www.gesetze-im-internet.de/juschg/__10b.html) ties interaction risks to those measures. The authority is the BZKJ ([§ 24b](https://www.gesetze-im-internet.de/juschg/__24b.html)). § 24a(1) refers to DSA Art. 28(1).
- DSA Art. 28 is in Section 3, so micro and small platforms are excluded by [DSA Art. 19(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065) (inference: § 24a then follows that scope, since it enforces Art. 28).

**Questions.**
1. Is a gaming network with text and voice for friends and for strangers an "intermediary service" so that JMStV steps back, or is it a "Telemedium" under the JMStV for part of its content (the launcher's game listings are our own content)? Our reading: the game listings are JMStV content (§ 5c(3) asks for age labels of games offered as own content), the chat is DSA territory.
2. Does § 24a JuSchG bind a volunteer operator who is a micro enterprise or not an "enterprise"? Does it apply without a user threshold?
3. Which measures must exist before messages and voice ship? We propose the ADR 0012 defaults (private profile, request inbox for strangers, no writing to strangers in the first day) plus a report route that works for children, a visible help link, and no public lookup of minors by search engines.
4. Is there anything in the JMStV operating-system rules (§§ 12, 12a) the launcher must honour? Our reading: they bind operating-system providers; whether the KJM has decided which systems children use was not checked.

## O3. Data protection impact assessment (DPIA)

**Why it matters.** [Art. 35(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) asks for a DPIA before processing likely to result in a high risk, especially with new technology. Art. 35(3) lists three cases (systematic extensive profiling with significant effects, large-scale special-category data, large-scale systematic monitoring of public areas). None applies clearly, but the list is not closed. Art. 35(7) gives the contents: description, necessity and proportionality, risk assessment, measures.

**Why we think one is likely needed (inference).** A network used by children, with presence (what they play), friends, messages stored readable by the operator ([ADR 0011](https://github.com/anyplaynet/network/blob/main/docs/adr/0011-messages-are-stored-to-be-delivered.md)) and voice, is a service where children are a vulnerable group. We did not read the German supervisory authorities' DPIA lists; that is the next step.

**Questions.** Is a DPIA required today, before messages exist? Required before messages? Who signs it? Must it be done by each operator, or can one shared DPIA be a template? We propose a DPIA for the child-facing parts when messages and voice are specified, and a short "no DPIA needed" note for the current core, with the reasons.

## O4. Messages stored readable by the operator

**Why it matters.** ADR 0011 decides that the cloud stores messages, encrypted in transit but readable by the cloud, for a limited time (no period named), so that reports can carry them to moderation. This is the highest-risk new data.

**Questions.**
1. Is that lawful for private conversations? On what basis (Art. 6(1)(b) for delivery, and for moderator access Art. 6(1)(f))? Is access only on a report enough?
2. Is a messaging feature a telecommunications or digital-service offer under German telecom and TDDDG secrecy rules? We did not read those rules.
3. What retention? We propose delivery plus 30 days for read messages, and a copy kept with a report for the life of the case.
4. If end-to-end encryption comes later, what changes for reports?

## O5. The operator: legal form, address, liability, "micro or small" status

**Why it matters.** The operator is a volunteer project. Everything with an operator name, address and contact is a placeholder.

**Questions.**
1. Which form (association, company, sole person)? Who is the controller under GDPR Art. 4 and 13(1)(a)? Personal liability of volunteers?
2. Is an unincorporated project an "enterprise" for [Recommendation 2003/361/EC](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32003H0361), so that DSA Art. 15(2) and 19(1) exempt it? What happens to the status when a form is chosen? Linked and partner enterprises are added up (Annex Art. 3, 6).
3. Is the imprint duty of [DDG § 5(1)](https://www.gesetze-im-internet.de/ddg/__5.html) triggered? It applies to services offered "geschäftsmäßig, in der Regel gegen Entgelt" (in the course of business, usually for a fee). A free service from a volunteer may not be one; if donations or sponsors appear, that may change.
4. May the operator's postal address be kept off the public page (a c/o address, a post box)? DSA Art. 11 and 12 ask for electronic contact points; DSA Art. 13 asks for a representative only outside the EU.

## O6. Processors: mail, hosting, backups, relay hosts (Art. 28)

**What the law says.** [Art. 28(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): use only processors with sufficient guarantees. Art. 28(3): a contract or other legal act with subject matter, duration, nature and purpose, data types, categories of people, and duties (instructions only, confidentiality, Art. 32 security, sub-processor rules, help with data subject requests and Art. 32 to 36, deletion or return at the end, audits). Art. 28(2): sub-processors only with the controller's authorisation.

**Who is a processor here.**
- The **mail provider** behind `smtp_host`: sees the e-mail address and the code of every sign-in.
- The **hosting provider** of the cloud and database.
- The **backup provider**, if separate.
- The **relay host**: sees IP addresses (inventory section 4).
- Not a processor: the certificate authority (receives the operator's address only); GitHub for launcher updates (an independent party that sees the player's IP).

**Questions.** Which standard contract (provider's or ours)? Are mail providers with a US parent acceptable ([Chapter V](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): adequacy under Art. 45 or safeguards under Art. 46)? We propose an EU-only provider and a written list. Open source operators who self-host mail need no processor for it; the notice must say so.

## O7. Breach process (Art. 33, 34)

**What the law says.**
- [Art. 33(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): notify the supervisory authority "without undue delay and, where feasible, not later than 72 hours" after becoming aware, unless the breach is unlikely to result in a risk. Art. 33(3): content (nature, categories and numbers, contact point, consequences, measures). Art. 33(4): in phases. Art. 33(5): document every breach, including those not reported.
- Art. 33(2): a processor tells the controller without undue delay.
- [Art. 34(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): tell the people concerned if the risk is high, in clear and plain language; Art. 34(3)(a) excuses it for data that is encrypted and unintelligible to others.
- The signing key is a separate risk: a stolen `signing.key` lets an attacker issue tickets and tokens for any account (inference). The key needs its own rotation procedure ([ARCHITECTURE](../ARCHITECTURE.md) plans key IDs).

**Questions.** Who decides? Which authority is competent for a volunteer operator in Germany? What counts as "aware"? What template and log? We propose a one-page runbook (detect, decide, notify within 72 hours, tell players, document) and a breach register; also a sign-in e-mail text for players.

## O8. Retention periods and the ban hash

**Why it matters.** The code keeps bans, reports and their reasons for ever (gaps G1, G2). The e-mail hash is plain SHA-256.

**Questions.**
1. Is keeping a hash of the e-mail address of a banned person lawful after the account is deleted? Our basis is Art. 6(1)(f), with a reason in the notice. For how long?
2. Is a keyed hash (HMAC) enough to count as pseudonymised ([Art. 4(5)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)) and does it change the answer?
3. Are the proposed periods (reports 12 months after decision, temporary bans 12 months after they end, permanent bans reviewed after 5 years, logs 30 days, inactive accounts 24 months, devices 90 days) reasonable? They are our guesses.
4. May the operator refuse an erasure request for a ban record under [Art. 17(3)(e)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) (legal claims)? Or must it rest on Art. 21(1) balancing?

## O9. The launcher's local storage (TDDDG § 25)

**What the law says.** [§ 25(1) TDDDG](https://www.gesetze-im-internet.de/ttdsg/__25.html): storing information on, or reading information from, the end user's device is allowed only with consent after clear and full information. § 25(2) no. 2: no consent is needed where the storage or access is "unbedingt erforderlich" (absolutely necessary) for a digital service the user expressly asked for.

**What the apps store (from the code).** Sign-in token (system credential store or an owner-only file), an endpoint key file, preferences, game library settings, optional observation logs. The launcher also reads the game files the player points it to. None of it is analytics or tracking. The launcher has no telemetry that we found (we searched the launcher repository for telemetry and analytics and found none).

**Questions.** Is all of it "unbedingt erforderlich" so that § 25(2) no. 2 applies and no banner is needed (our reading)? Does reading the player's installed games and trophy files fall under § 25(1) ("access to information already stored")? If so, is the player's choice of the game folder an "express" request? What must the notice say? Does the rule apply to a program installed on a PC at all, or only to browser-like storage? The text speaks of "Endeinrichtung" (terminal equipment) generally.

## O10. IP addresses at the relay and the proxy

**Why it matters.** The relay sees every client's IP and a ticket that names the account ID. Our code stores nothing, but `iroh-relay` was not audited (inventory section 4) and Caddy's access-log default was not run.

**Questions.** Is the IP address of a relay client personal data processed for the service (yes, we assume) and on which basis (Art. 6(1)(b))? Is a short abuse log (7 days) justified under Art. 6(1)(f)? Should the notice name the relay operator? We propose to run both programs once, read their output at info and debug level, and record the results in the inventory.

## O11. Which DSA category, and when does it change?

See [dsa.md](dsa.md). We propose hosting service today and online platform once profiles are public. Questions: is the handle lookup by any signed-in player "dissemination to the public" ([Art. 3(k)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065)) with open sign-up? Is the relay a mere conduit and the cloud a hosting service, or one service? Does the DSA apply to a service of this size to begin with, when it has no EU consumers outside Germany? (It applies to services offered to recipients in the EU, [Art. 2(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065).) We did not retrieve Art. 93 itself; check the date 17 February 2024.

## O12. Procedure for authority orders and threats to life (DSA Art. 9, 10, 18)

[Art. 18](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32022R2065): inform law enforcement promptly on suspicion of an offence involving a threat to the life or safety of a person. Orders under Art. 9 and 10 reach the Art. 11 point of contact. **Questions:** Who in the project reads that mailbox, in what languages? What is the German authority? How does a volunteer team get legal help on a short deadline? How is the player told when the operator hands over data (Art. 10(5))?

## O13. Cyber Resilience Act

**What the law says.** [Art. 2(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847): products with digital elements made available on the market whose use includes a data connection. [Art. 3(22)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847): "made available" means "in the course of a commercial activity, whether in return for payment or free of charge". [Recital 18](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847): free and open-source software that is "not monetised by [its] manufacturers" is not a commercial activity; [Recital 15](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847): "Accepting donations without the intention of making a profit should not be considered to be a commercial activity". [Art. 3(2)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847) includes "remote data processing" the product cannot work without, which may reach the cloud (see Recitals 11 and 12). [Art. 24](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32024R2847): a lighter regime for open-source stewards.

**Dates (Art. 71, as read in the 2024/2847 text).** The reporting duty of Art. 14 (actively exploited vulnerabilities: early warning in 24 hours, report in 72 hours) applies from 11 September 2026, so it has applied since then; Art. 14 also covers products placed on the market before 11 December 2027. The rest applies from 11 December 2027.

**Questions.**
1. Is the launcher (open source, free, no monetisation) outside the CRA? What if the project takes donations beyond cost, a sponsor, or forms a company that offers paid features? What if it is distributed through a store?
2. If the launcher is covered, who is the "manufacturer" for an unincorporated project, and who is a steward? Does the cloud count as remote data processing of the launcher or as an independent service (Recitals 11 and 12)?
3. If Art. 14 applies, who files the report and on what platform? We propose a security policy and a coordinated vulnerability disclosure contact now: it costs little and CRA Art. 13(8) asks for one from covered manufacturers. We did not search for Commission guidance on open-source projects (Recital 6 announces some).

## O14. UK Online Safety Act

The research listed it as unchecked. [Section 4 OSA 2023](https://www.legislation.gov.uk/ukpga/2023/50/section/4/data.htm) makes a user-to-user service regulated if it has links to the UK: "a significant number of United Kingdom users" (s.4(5)) or a "material risk of significant harm" to people in the UK (s.4(6)); s.3 defines user-to-user services as services where user content may be encountered by another user; there is no exemption for volunteer or free services in the text we read. Schedule 1 exemptions (private one-to-one messaging is one candidate) were not read, nor Ofcom's guidance. **Questions:** Is a German friends-only gaming chat in scope? What is "significant" for a small network? Do we geo-block the UK for chat and voice, or accept the duties (illegal-content risk assessment, reporting, complaints)? We propose to decide before messages ship; it does not affect the core.

## O14a. Looking for group and shared media (0.3.0)

**OPERATOR AGENT: update the legal privacy page for LFG listings and shared screenshots/clips** (data inventory sections 7.2a and 7.2b). Questions for the lawyer: (1) a screenshot or clip is user content that the cloud stores for 30 days and cannot inspect: is the report-and-hold path enough, and may the cloud keep a held file for a year (`retention_open_reports_days`)? (2) A looking-for-group listing shows a handle and a free note to strangers who have the same game: is a note without links and with a report button enough for minors (see O2)? (3) Does the media folder need the erasure period of backups (G10)?

## O15. Transfers and location

The research says data, logs and backups stay in the EU. We could not confirm this per provider (no provider is chosen). **Question:** the checklist per provider: servers, support access from outside the EU, sub-processors, logs.

## O16. Do we need a DPO?

[Art. 37(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) requires one for public bodies, large-scale regular and systematic monitoring of people, or large-scale special-category data as core activity. We think none applies (inference). Is "presence" large-scale systematic monitoring? German law may add a duty by headcount; we did not read the BDSG.

## O17. Is the contract basis right for presence and sessions?

We proposed Art. 6(1)(b) for presence, friends and sessions. If the lawyer finds the contract basis too narrow for "what you are playing now", the alternative is legitimate interest with an opt-out, or consent. This matters for how the launcher presents the feature. It also matters because consent for a child under 16 needs a parent (Art. 8(1)).
