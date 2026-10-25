//! Join codes: issuing, replacing and revoking them, redeeming them for a session or a party
//! invitation, and the public page of a code. Every test runs on SQLite, and on PostgreSQL when
//! `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use std::sync::{Arc, Mutex};

use anyplay_api::{
    Event, Invitation, JOIN_CODE_ALPHABET, JOIN_CODE_LEN, JoinCode, Party, PartyInvitation,
    Redeemed, Session,
};
use common::portal::Portal;
use common::{TestCloud, User};
use serde_json::{Value, json};

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

async fn open_session(
    cloud: &TestCloud,
    user: &User,
    n: u8,
    max: u16,
    visibility: &str,
) -> Session {
    cloud
        .post(
            "/v1/sessions",
            user.token(),
            json!({"title_id": "GAME00001", "title": "Test Game", "version": "01.000",
                   "build": "0.1.3", "max_members": max, "visibility": visibility,
                   "properties": "", "endpoint": endpoint(n), "relay": null}),
        )
        .await
        .ok(200)
        .json()
}

async fn join_session(cloud: &TestCloud, user: &User, id: &str, n: u8, invitation: Option<&str>) {
    cloud
        .post(
            &format!("/v1/sessions/{id}/join"),
            user.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(n),
                   "relay": null, "invitation": invitation}),
        )
        .await
        .ok(200);
}

async fn make_party(cloud: &TestCloud, user: &User) -> Party {
    cloud
        .post_empty("/v1/party", user.token())
        .await
        .ok(200)
        .json()
}

async fn issue(cloud: &TestCloud, user: &User) -> JoinCode {
    cloud
        .post("/v1/join-codes", user.token(), json!({}))
        .await
        .ok(200)
        .json()
}

async fn redeem(cloud: &TestCloud, user: &User, code: &str) -> common::Reply {
    cloud
        .post_empty(&format!("/v1/join-codes/{code}/redeem"), user.token())
        .await
}

fn session_invitation(redeemed: Redeemed) -> Invitation {
    match redeemed {
        Redeemed::Session(invitation) => invitation,
        other => panic!("a session invitation, got {other:?}"),
    }
}

fn party_invitation(redeemed: Redeemed) -> PartyInvitation {
    match redeemed {
        Redeemed::Party(invitation) => invitation,
        other => panic!("a party invitation, got {other:?}"),
    }
}

async fn block(cloud: &TestCloud, by: &User, who: &User) {
    cloud
        .put(&format!("/v1/blocks/{}", who.id), by.token(), None)
        .await
        .ok(204);
}

async fn rows(cloud: &TestCloud) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM join_codes")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_code_needs_something_to_join_and_is_replaced_and_revoked() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;

    // Nothing to join yet.
    cloud
        .post("/v1/join-codes", alec.token(), json!({}))
        .await
        .is_error("not_found", 404);
    // A closed session is nothing to join either.
    let closed = open_session(&cloud, &alec, 1, 4, "closed").await;
    cloud
        .post("/v1/join-codes", alec.token(), json!({}))
        .await
        .is_error("not_found", 404);
    cloud.post_empty(&format!("/v1/sessions/{}/leave", closed.id), alec.token()).await.ok(204);

    open_session(&cloud, &alec, 1, 4, "invite").await;
    let first = issue(&cloud, &alec).await;
    assert_eq!(first.code.len(), JOIN_CODE_LEN);
    assert!(first.code.chars().all(|c| JOIN_CODE_ALPHABET.contains(c)));
    let public = cloud.state.config.web_origin().unwrap();
    assert_eq!(first.url, format!("{public}/join/{}", first.code));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(first.expires.abs_diff(now + 120 * 60) < 30, "{}", first.expires);
    assert_eq!(rows(&cloud).await, 1);

    // A new code replaces the old one, which stops working. An account has one row.
    let second = issue(&cloud, &alec).await;
    assert_ne!(first.code, second.code);
    assert_eq!(rows(&cloud).await, 1);
    redeem(&cloud, &bob, &first.code).await.is_error("not_found", 404);
    redeem(&cloud, &bob, &second.code).await.ok(200);

    // Revoking ends it for good, and is harmless twice.
    cloud.delete("/v1/join-codes/current", alec.token()).await.ok(204);
    cloud.delete("/v1/join-codes/current", alec.token()).await.ok(204);
    assert_eq!(rows(&cloud).await, 0);
    redeem(&cloud, &bob, &second.code).await.is_error("not_found", 404);
    // Nobody else's code is touched by a revoke.
    cloud.delete("/v1/join-codes/current", bob.token()).await.ok(204);
}

#[tokio::test]
async fn the_cloud_stores_a_hash_and_never_the_code() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    let stored: Vec<u8> = sqlx::query_scalar("SELECT code_hash FROM join_codes")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert!(!stored.windows(code.len()).any(|w| w == code.as_bytes()));
    // The account's own row is the only column that names it, and it has no handle or ID in the code.
    assert!(!code.contains(&alec.id.to_string()));
    assert!(!code.contains("alec"));
}

#[tokio::test]
async fn a_session_code_makes_an_invitation_that_lets_a_stranger_into_an_invite_session() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    let session = open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    let mut events = cloud.events(dan.token()).await;

    // No friendship, and any case.
    let redeemed: Redeemed = redeem(&cloud, &dan, &code.to_uppercase()).await.ok(200).json();
    let invitation = session_invitation(redeemed);
    assert_eq!(invitation.from.handle, "Alec");
    assert_eq!(invitation.session, session.id);
    assert_eq!(invitation.title, "Test Game");
    match events
        .until(|e| matches!(e, Event::Invitation { .. }))
        .await
    {
        Event::Invitation { invitation: pushed } => assert_eq!(pushed, invitation),
        _ => unreachable!(),
    }
    // Redeeming did not join Dan.
    let seen: Session = cloud
        .get(&format!("/v1/sessions/{}", session.id), dan.token())
        .await
        .ok(200)
        .json();
    assert_eq!(seen.members.len(), 1);

    // Again: the invitation Dan holds, and no second one.
    let again = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_eq!(again.id, invitation.id);
    events.quiet(std::time::Duration::from_millis(300)).await;

    // The invitation lets Dan in; the version guard still runs on the join.
    cloud
        .post(
            &format!("/v1/sessions/{}/join", session.id),
            dan.token(),
            json!({"version": "02.000", "build": "0.1.3", "endpoint": endpoint(2),
                   "relay": null, "invitation": invitation.id}),
        )
        .await
        .is_error("version_mismatch", 409);
    join_session(&cloud, &dan, &session.id, 2, Some(&invitation.id)).await;
    // Now Dan is a member: the code answers limit.
    redeem(&cloud, &dan, &code).await.is_error("limit", 409);
}

#[tokio::test]
async fn a_declined_invitation_is_made_again_by_redeeming_again() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    open_session(&cloud, &alec, 1, 4, "friends").await;
    let code = issue(&cloud, &alec).await.code;
    let first = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    cloud
        .delete(&format!("/v1/invitations/{}", first.id), dan.token())
        .await
        .ok(204);
    let second = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_ne!(first.id, second.id);
}

#[tokio::test]
async fn a_code_opens_the_session_joined_last_that_is_not_closed_else_the_party() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    let party = make_party(&cloud, &alec).await;
    let code = {
        // A party leader with no session offers the party.
        let code = issue(&cloud, &alec).await.code;
        let invitation = party_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
        assert_eq!(invitation.party, party.id);
        assert_eq!(invitation.from.handle, "Alec");
        assert_eq!(invitation.members, 1);
        cloud
            .delete(&format!("/v1/party/invitations/{}", invitation.id), dan.token())
            .await
            .ok(204);
        code
    };

    // The same code now opens a session: it stands for the player.
    let older = open_session(&cloud, &alec, 1, 4, "invite").await;
    // Sessions order by the second they were joined in.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let newer = open_session(&cloud, &alec, 2, 4, "friends").await;
    let invitation = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_eq!(invitation.session, newer.id, "the one joined last");
    cloud
        .delete(&format!("/v1/invitations/{}", invitation.id), dan.token())
        .await
        .ok(204);

    // The newest one closes: the older one is next.
    cloud
        .patch(
            &format!("/v1/sessions/{}", newer.id),
            alec.token(),
            json!({"visibility": "closed"}),
        )
        .await
        .ok(200);
    let invitation = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_eq!(invitation.session, older.id);
    cloud
        .delete(&format!("/v1/invitations/{}", invitation.id), dan.token())
        .await
        .ok(204);

    // Both end: the party again, and with nothing at all, not_found.
    for id in [&older.id, &newer.id] {
        cloud
            .post_empty(&format!("/v1/sessions/{id}/leave"), alec.token())
            .await
            .ok(204);
    }
    let invitation = party_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_eq!(invitation.party, party.id);
    cloud.post_empty("/v1/party/leave", alec.token()).await.ok(204);
    redeem(&cloud, &dan, &code).await.is_error("not_found", 404);
    // The code lives on and works again when something opens.
    open_session(&cloud, &alec, 3, 4, "invite").await;
    redeem(&cloud, &dan, &code).await.ok(200);
}

#[tokio::test]
async fn a_party_code_makes_a_party_invitation_that_is_accepted_like_any_other() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    let eve = cloud.sign_up("Eve").await;
    let party = make_party(&cloud, &alec).await;
    let code = issue(&cloud, &alec).await.code;
    let mut events = cloud.events(dan.token()).await;

    let invitation = party_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    match events
        .until(|e| matches!(e, Event::PartyInvitation { .. }))
        .await
    {
        Event::PartyInvitation { invitation: pushed } => assert_eq!(pushed, invitation),
        _ => unreachable!(),
    }
    // Held already: the same invitation, with no second push.
    let again = party_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    assert_eq!(again.id, invitation.id);
    events.quiet(std::time::Duration::from_millis(300)).await;

    let joined: Party = cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            dan.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(joined.id, party.id);
    assert_eq!(joined.members.len(), 2);
    // A member: limit. Eve is in another party: limit.
    redeem(&cloud, &dan, &code).await.is_error("limit", 409);
    make_party(&cloud, &eve).await;
    redeem(&cloud, &eve, &code).await.is_error("limit", 409);
}

#[tokio::test]
async fn a_full_session_or_party_answers_full() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let cy = cloud.sign_up("Cy").await;
    let session = open_session(&cloud, &alec, 1, 2, "friends").await;
    let code = issue(&cloud, &alec).await.code;
    // Bob joins with an invitation and the session is full.
    let invitation = session_invitation(redeem(&cloud, &bob, &code).await.ok(200).json());
    join_session(&cloud, &bob, &session.id, 2, Some(&invitation.id)).await;
    redeem(&cloud, &cy, &code).await.is_error("full", 409);

    // A party of eight.
    let leader = cloud.sign_up("Lead").await;
    make_party(&cloud, &leader).await;
    let code = issue(&cloud, &leader).await.code;
    for k in 0..7 {
        let member = cloud.sign_up(&format!("Mate{k}")).await;
        let invitation = party_invitation(redeem(&cloud, &member, &code).await.ok(200).json());
        cloud
            .post_empty(
                &format!("/v1/party/invitations/{}/accept", invitation.id),
                member.token(),
            )
            .await
            .ok(200);
    }
    redeem(&cloud, &cy, &code).await.is_error("full", 409);
}

#[tokio::test]
async fn blocks_and_the_own_code_and_bad_codes_are_refused() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let cy = cloud.sign_up("Cy").await;
    let dan = cloud.sign_up("Dan").await;
    let session = open_session(&cloud, &alec, 1, 6, "friends").await;
    let code = issue(&cloud, &alec).await.code;

    // The own code.
    redeem(&cloud, &alec, &code).await.is_error("bad_request", 400);

    // Not codes at all, unknown codes and expired ones look alike.
    let unknown = "z".repeat(JOIN_CODE_LEN);
    let bodies = [
        redeem(&cloud, &bob, &unknown).await,
        redeem(&cloud, &bob, "short").await,
        redeem(&cloud, &bob, &"0".repeat(JOIN_CODE_LEN)).await,
    ];
    for reply in &bodies {
        reply.is_error("not_found", 404);
        assert_eq!(reply.body, bodies[0].body);
    }

    // The issuer blocked Bob: not found, as if the code did not exist.
    block(&cloud, &alec, &bob).await;
    redeem(&cloud, &bob, &code).await.is_error("not_found", 404);
    // Cy blocked a member (Dan), and Dan is in the session.
    let invitation = session_invitation(redeem(&cloud, &dan, &code).await.ok(200).json());
    join_session(&cloud, &dan, &session.id, 2, Some(&invitation.id)).await;
    block(&cloud, &cy, &dan).await;
    redeem(&cloud, &cy, &code).await.is_error("not_found", 404);
    // The same for a party.
    let lead = cloud.sign_up("Lead").await;
    let eve = cloud.sign_up("Eve").await;
    make_party(&cloud, &lead).await;
    let party_code = issue(&cloud, &lead).await.code;
    block(&cloud, &eve, &lead).await;
    redeem(&cloud, &eve, &party_code).await.is_error("not_found", 404);
    // A block set after redeeming takes the invitation back, like any invitation.
    let fay = cloud.sign_up("Fay").await;
    let held = party_invitation(redeem(&cloud, &fay, &party_code).await.ok(200).json());
    block(&cloud, &lead, &fay).await;
    cloud
        .post_empty(&format!("/v1/party/invitations/{}/accept", held.id), fay.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_code_ends_when_it_expires_and_the_sweep_removes_it() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    sqlx::query("UPDATE join_codes SET expires = 1")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    redeem(&cloud, &dan, &code).await.is_error("not_found", 404);
    Portal::new(&cloud)
        .get(&format!("/join/{code}"))
        .await
        .error(404, "not_found");
    assert_eq!(rows(&cloud).await, 1);
    cloud.state.sweep().await.unwrap();
    assert_eq!(rows(&cloud).await, 0);
}

#[tokio::test]
async fn the_lifetime_is_a_setting() {
    let cloud = TestCloud::with(|c| c.join_code_minutes = 5, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(code.expires.abs_diff(now + 300) < 30, "{}", code.expires);
}

#[tokio::test]
async fn without_an_account_web_the_link_uses_the_address_the_request_came_to() {
    let cloud = TestCloud::with(|c| c.public_url = None, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await;
    assert_eq!(code.url, format!("http://{}/join/{}", cloud.base, code.code));
}

#[tokio::test]
async fn deleting_the_account_ends_its_code() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    cloud.delete_account_now(&alec).await;
    assert_eq!(rows(&cloud).await, 0);
    redeem(&cloud, &dan, &code).await.is_error("not_found", 404);
}

#[tokio::test]
async fn codes_are_limited_per_account() {
    let cloud = TestCloud::with(
        |_| {},
        |t| {
            t.rates.join_codes_new = 2;
            t.rates.join_codes_redeemed = 3;
        },
    )
    .await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    issue(&cloud, &alec).await;
    let code = issue(&cloud, &alec).await.code;
    let limited = cloud.post("/v1/join-codes", alec.token(), json!({})).await;
    limited.is_error("rate_limited", 429);
    assert!(limited.headers.contains_key("retry-after"));
    // The wait counts per account.
    issue(&cloud, &dan_with_session(&cloud).await).await;

    for _ in 0..3 {
        redeem(&cloud, &dan, &code).await.ok(200);
    }
    // Wrong codes count too: a program cannot guess for free.
    redeem(&cloud, &dan, &code).await.is_error("rate_limited", 429);
    redeem(&cloud, &dan, "abcdefghij").await.is_error("rate_limited", 429);
}

async fn dan_with_session(cloud: &TestCloud) -> User {
    let user = cloud.sign_up("Fred").await;
    open_session(cloud, &user, 9, 4, "invite").await;
    user
}

#[tokio::test]
async fn the_public_page_shows_who_invites_to_what_and_nothing_else() {
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let bob = cloud.sign_up("Bob").await;
    let browser = Portal::new(&cloud);

    let session = open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    // No session of the visitor is needed, and the case does not matter.
    let seen = browser.get(&format!("/join/{}", code.to_uppercase())).await.ok(200);
    assert_eq!(
        seen,
        json!({"handle": "Alec", "title": "Test Game", "members": 1, "max_members": 4,
               "kind": "session"})
    );
    let text = seen.to_string();
    for secret in [
        session.id.as_str(),
        "GAME00001",
        &alec.id.to_string(),
        &code,
        "01.000",
    ] {
        assert!(!text.contains(secret), "{secret} leaked in {text}");
    }
    // It reads: nothing was made, nobody joined.
    let invitations: Value = cloud
        .get("/v1/party/invitations", bob.token())
        .await
        .ok(200)
        .body;
    assert_eq!(invitations["invitations"], json!([]));
    let seen: Session = cloud
        .get(&format!("/v1/sessions/{}", session.id), alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(seen.members.len(), 1);

    // A party shows no title and its size of eight.
    cloud
        .post_empty(&format!("/v1/sessions/{}/leave", session.id), alec.token())
        .await
        .ok(204);
    make_party(&cloud, &alec).await;
    assert_eq!(
        browser.get(&format!("/join/{code}")).await.ok(200),
        json!({"handle": "Alec", "title": null, "members": 1, "max_members": 8, "kind": "party"})
    );

    // One answer for every code that opens nothing.
    let revoked = issue(&cloud, &alec).await.code;
    cloud.delete("/v1/join-codes/current", alec.token()).await.ok(204);
    cloud.post_empty("/v1/party/leave", alec.token()).await.ok(204);
    let nothing = browser.get(&format!("/join/{code}")).await;
    let gone = browser.get(&format!("/join/{revoked}")).await;
    let unknown = browser.get("/join/zzzzzzzzzz").await;
    let malformed = browser.get("/join/x").await;
    let first = nothing.error(404, "not_found");
    for answer in [gone, unknown, malformed] {
        assert_eq!(answer.error(404, "not_found"), first);
    }
}

#[tokio::test]
async fn public_pages_are_limited_per_address() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.portal_public = 3).await;
    let browser = Portal::new(&cloud);
    for _ in 0..3 {
        browser.get("/join/zzzzzzzzzz").await.error(404, "not_found");
    }
    let limited = browser.get("/join/zzzzzzzzzz").await;
    limited.error(429, "rate_limited");
    assert!(limited.header("retry-after").is_some());
}

/// Collects what `tracing` writes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;

    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

#[tokio::test]
async fn no_log_line_holds_a_code() {
    let log = Log::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(log.clone())
            .finish(),
    );
    let cloud = TestCloud::start().await;
    let alec = cloud.sign_up("Alec").await;
    let dan = cloud.sign_up("Dan").await;
    open_session(&cloud, &alec, 1, 4, "invite").await;
    let code = issue(&cloud, &alec).await.code;
    redeem(&cloud, &dan, &code).await.ok(200);
    redeem(&cloud, &dan, &code.to_uppercase()).await.ok(200);
    redeem(&cloud, &alec, &code).await.is_error("bad_request", 400);
    Portal::new(&cloud)
        .get(&format!("/join/{code}"))
        .await
        .ok(200);
    cloud.delete("/v1/join-codes/current", alec.token()).await.ok(204);
    redeem(&cloud, &dan, &code).await.is_error("not_found", 404);
    Portal::new(&cloud)
        .get(&format!("/join/{code}"))
        .await
        .error(404, "not_found");

    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).to_lowercase();
    assert!(text.contains("request"), "the request log was captured");
    assert!(!text.contains(&code), "a join code is in the log");
}
