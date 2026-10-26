//! Moderation (`docs/moderation.md`): the report queue, every action with its statement of
//! reasons and audit line, appeals, the moderator role, new-account limits, retention, the keyed
//! e-mail hash and a ban across two nodes. Like the other tests these run on SQLite, and on
//! PostgreSQL as well when `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use std::time::Duration;

use anyplay_api::{
    ActionKind, ActionView, AppealInfo, AppealStatus, AppealView, AuditAction, AuditEntry, Event,
    ReferenceKind, ReportOutcome, ReportStatus, ReportView, Rule, Statement, Statements, close,
};
use anyplay_cloud::moderation;
use anyplay_cloud::util::{now, sha256};
use common::{TestCloud, User};
use serde_json::{Value, json};

const ADMIN: &str = "an-admin-token-for-tests";
const DAY: i64 = 24 * 60 * 60;

async fn cloud() -> TestCloud {
    TestCloud::with(|c| c.admin_token = Some(ADMIN.into()), |_| {}).await
}

/// A new account that the operator made a moderator.
async fn moderator(cloud: &TestCloud, name: &str) -> User {
    let user = cloud.sign_up(name).await;
    cloud
        .post(
            "/v1/moderation/moderators",
            ADMIN,
            json!({"handle": user.handle}),
        )
        .await
        .ok(200);
    user
}

async fn file_report(cloud: &TestCloud, from: &User, about: &User) {
    cloud
        .post(
            "/v1/reports",
            from.token(),
            json!({"account": about.id, "reason": "harassment", "details": "rude in chat"}),
        )
        .await
        .ok(204);
}

async fn queue(cloud: &TestCloud, token: &str, status: &str) -> Vec<ReportView> {
    cloud
        .get(&format!("/v1/moderation/reports?status={status}"), token)
        .await
        .ok(200)
        .json()
}

async fn take_action(cloud: &TestCloud, token: &str, body: Value) -> ActionView {
    cloud
        .post("/v1/moderation/actions", token, body)
        .await
        .ok(200)
        .json()
}

async fn log(cloud: &TestCloud, token: &str) -> Vec<AuditEntry> {
    cloud
        .get("/v1/moderation/log?limit=200", token)
        .await
        .ok(200)
        .json()
}

async fn my_actions(cloud: &TestCloud, token: &str) -> Vec<Statement> {
    cloud
        .get("/v1/me/actions", token)
        .await
        .ok(200)
        .json::<Statements>()
        .actions
}

async fn sql(cloud: &TestCloud, query: &'static str, a: i64, b: i64) {
    sqlx::query(query)
        .bind(a)
        .bind(b)
        .execute(&cloud.state.db)
        .await
        .unwrap();
}

async fn count(cloud: &TestCloud, query: &'static str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(query)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    n
}

/// Makes an account old enough to write to strangers and returns it.
async fn age(cloud: &TestCloud, user: &User) {
    sql(
        cloud,
        "UPDATE accounts SET created = $1 WHERE id = $2",
        now() as i64 - 3 * DAY,
        user.id as i64,
    )
    .await;
}

// The queue

#[tokio::test]
async fn the_queue_lists_open_reports_oldest_first_with_what_was_reported() {
    let cloud = cloud().await;
    let m = moderator(&cloud, "Mona").await;
    let (a, b, c) = (
        cloud.sign_up("Alec").await,
        cloud.sign_up("Bea").await,
        cloud.sign_up("Sam").await,
    );
    file_report(&cloud, &a, &c).await;
    file_report(&cloud, &b, &c).await;

    let open = queue(&cloud, m.token(), "open").await;
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].reporter.as_ref().unwrap().handle, "Alec");
    assert_eq!(open[1].reporter.as_ref().unwrap().handle, "Bea");
    let first = &open[0];
    assert_eq!(first.reported.handle, "Sam");
    assert_eq!(first.status, ReportStatus::Open);
    assert_eq!(first.details, "rude in chat");
    assert!(first.content.contains("Sam"), "snapshot: {}", first.content);
    assert!(!first.reported_deleted);
    assert!(queue(&cloud, m.token(), "decided").await.is_empty());
    assert_eq!(queue(&cloud, m.token(), "all").await.len(), 2);

    let one: ReportView = cloud
        .get(&format!("/v1/moderation/reports/{}", first.id), m.token())
        .await
        .ok(200)
        .json();
    assert_eq!(one, *first);
    cloud
        .get("/v1/moderation/reports?status=sideways", m.token())
        .await
        .is_error("bad_request", 400);
    cloud
        .get("/v1/moderation/reports/999999", m.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn reports_are_limited_per_account() {
    let cloud = TestCloud::with(
        |c| c.report_limit_per_day = 3,
        |t| t.rates.report_per_account = 2,
    )
    .await;
    let a = cloud.sign_up("Alec").await;
    let others = [
        cloud.sign_up("Bea").await,
        cloud.sign_up("Cay").await,
        cloud.sign_up("Dan").await,
    ];
    file_report(&cloud, &a, &others[0]).await;
    // The same account twice while the first is open.
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": others[0].id, "reason": "spam"}),
        )
        .await
        .is_error("limit", 409);
    file_report(&cloud, &a, &others[1]).await;
    // Two reports are allowed in the window; the third waits.
    let third = cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": others[2].id, "reason": "spam"}),
        )
        .await;
    third.is_error("rate_limited", 429);
    assert!(third.headers.contains_key("retry-after"));
}

#[tokio::test]
async fn at_most_the_daily_number_of_reports_is_taken() {
    let cloud = TestCloud::with(|c| c.report_limit_per_day = 2, |_| {}).await;
    let a = cloud.sign_up("Alec").await;
    for name in ["Bea", "Cay"] {
        let other = cloud.sign_up(name).await;
        file_report(&cloud, &a, &other).await;
    }
    let dan = cloud.sign_up("Dan").await;
    cloud
        .post(
            "/v1/reports",
            a.token(),
            json!({"account": dan.id, "reason": "spam"}),
        )
        .await
        .is_error("limit", 409);
}

async fn open_chat(cloud: &TestCloud, from: &User, with: &User) -> common::Reply {
    cloud
        .post(
            "/v1/conversations",
            from.token(),
            json!({"accounts": [with.id]}),
        )
        .await
}

async fn say(cloud: &TestCloud, user: &User, conversation: &str, text: &str) -> common::Reply {
    cloud
        .post(
            &format!("/v1/conversations/{conversation}/messages"),
            user.token(),
            json!({"text": text}),
        )
        .await
}

async fn report_message(
    cloud: &TestCloud,
    from: &User,
    about: &User,
    conversation: Value,
    seq: Value,
) -> common::Reply {
    cloud
        .post(
            "/v1/reports",
            from.token(),
            json!({"account": about.id, "reason": "harassment",
                   "conversation": conversation, "seq": seq}),
        )
        .await
}

// Who may moderate

#[tokio::test]
async fn nobody_acts_without_the_role() {
    let cloud = cloud().await;
    let user = cloud.sign_up("Alec").await;
    let target = cloud.sign_up("Sam").await;
    file_report(&cloud, &user, &target).await;
    let body = json!({"account": target.id, "kind": "warning", "rule": "spam", "reason": "Stop."});

    cloud
        .post("/v1/moderation/actions", user.token(), body.clone())
        .await
        .is_error("forbidden", 403);
    cloud
        .get("/v1/moderation/reports", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .get("/v1/moderation/log", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .get("/v1/moderation/appeals", user.token())
        .await
        .is_error("forbidden", 403);
    cloud
        .request(reqwest::Method::GET, "/v1/moderation/reports", None, None)
        .await
        .is_error("unauthorized", 401);
    cloud
        .get("/v1/moderation/reports", "not-the-admin-token")
        .await
        .is_error("unauthorized", 401);
    assert!(my_actions(&cloud, target.token()).await.is_empty());

    // Only the operator names moderators.
    let mona = moderator(&cloud, "Mona").await;
    cloud
        .post(
            "/v1/moderation/moderators",
            mona.token(),
            json!({"handle": "Alec"}),
        )
        .await
        .is_error("forbidden", 403);
    take_action(&cloud, mona.token(), body.clone()).await;

    // Taking the role back works on the next request.
    cloud
        .delete(&format!("/v1/moderation/moderators/{}", mona.id), ADMIN)
        .await
        .ok(204);
    cloud
        .post("/v1/moderation/actions", mona.token(), body)
        .await
        .is_error("forbidden", 403);
    cloud
        .delete(&format!("/v1/moderation/moderators/{}", mona.id), ADMIN)
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_moderator_cannot_act_on_themselves_or_on_another_moderator() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let max = moderator(&cloud, "Max").await;
    let own = json!({"account": mona.id, "kind": "warning", "rule": "other", "reason": "Me."});
    cloud
        .post("/v1/moderation/actions", mona.token(), own)
        .await
        .is_error("forbidden", 403);
    let other = json!({"account": max.id, "kind": "warning", "rule": "other", "reason": "Him."});
    cloud
        .post("/v1/moderation/actions", mona.token(), other.clone())
        .await
        .is_error("forbidden", 403);
    // The operator may.
    take_action(&cloud, ADMIN, other).await;
}

#[tokio::test]
async fn the_operator_token_acts_under_the_name_operator() {
    let cloud = cloud().await;
    let target = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        ADMIN,
        json!({"account": target.id, "kind": "warning", "rule": "name", "reason": "Pick another name."}),
    )
    .await;
    assert_eq!(view.actor.handle, "operator");
    assert_eq!(view.actor.id, None);
    let entries = log(&cloud, ADMIN).await;
    assert_eq!(entries[0].actor.handle, "operator");
    assert_eq!(entries[0].action, AuditAction::Warned);

    // The legacy admin ban goes through the same path: statement and audit line included.
    cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "sam", "rule": "cheating", "reason": "Aimbot."})),
        )
        .await
        .ok(200);
    let statements = ban_statements(&cloud, &target).await;
    assert!(
        statements
            .iter()
            .any(|s| s.kind == ActionKind::Ban && s.rule == Rule::Cheating)
    );
    // A ban needs a reason the player can read.
    cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/bans",
            Some(ADMIN),
            Some(json!({"handle": "sam"})),
        )
        .await
        .is_error("bad_request", 400);
}

/// The statements of a banned player, read with the appeal token of the sign-in error.
async fn ban_statements(cloud: &TestCloud, user: &User) -> Vec<Statement> {
    let (challenge, code) = cloud.ask_for_code(&user.email).await;
    let reply = cloud.enter_code(&challenge.challenge, &code, "pc").await;
    reply.is_error("banned", 403);
    let token = reply.body["ban"]["appeal_token"]
        .as_str()
        .unwrap()
        .to_owned();
    my_actions(cloud, &token).await
}

// Actions

#[tokio::test]
async fn requests_for_actions_are_checked() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let sam = cloud.sign_up("Sam").await;
    let kim = cloud.sign_up("Kim").await;
    file_report(&cloud, &kim, &sam).await;
    let post = |body: Value| cloud.post("/v1/moderation/actions", mona.token(), body);
    let base = |kind: &str| json!({"account": sam.id, "kind": kind, "rule": "spam", "reason": "x"});

    let mut with_days = base("warning");
    with_days["days"] = json!(3);
    post(with_days).await.is_error("bad_request", 400);
    post(base("mute")).await.is_error("bad_request", 400);
    let mut zero = base("ban");
    zero["days"] = json!(0);
    post(zero).await.is_error("bad_request", 400);
    let mut huge = base("ban");
    huge["days"] = json!(4000);
    post(huge).await.is_error("bad_request", 400);
    let mut empty = base("warning");
    empty["reason"] = json!("   ");
    post(empty).await.is_error("bad_request", 400);
    let mut long = base("warning");
    long["reason"] = json!("x".repeat(501));
    post(long).await.is_error("bad_request", 400);
    let mut ghost = base("warning");
    ghost["account"] = json!(4242);
    post(ghost).await.is_error("not_found", 404);
    let mut wrong_report = base("warning");
    wrong_report["account"] = json!(kim.id);
    wrong_report["report"] = json!(queue(&cloud, mona.token(), "open").await[0].id);
    post(wrong_report).await.is_error("bad_request", 400);
    // Nothing above left a trace.
    assert!(log(&cloud, mona.token()).await.len() == 1, "only the grant");
    assert_eq!(queue(&cloud, mona.token(), "open").await.len(), 1);
}

#[tokio::test]
async fn a_warning_reaches_the_player_with_its_reasons_and_is_logged() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let (kim, sam) = (cloud.sign_up("Kim").await, cloud.sign_up("Sam").await);
    file_report(&cloud, &kim, &sam).await;
    let report_id = queue(&cloud, mona.token(), "open").await[0].id;
    let mut events = cloud.events(sam.token()).await;

    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "warning", "rule": "harassment",
               "reason": "Do not insult other players.", "report": report_id}),
    )
    .await;
    let statement = view.statement.clone();
    assert_eq!(statement.kind, ActionKind::Warning);
    assert_eq!(statement.rule, Rule::Harassment);
    assert_eq!(statement.reason, "Do not insult other players.");
    assert!(statement.from_report);
    assert!(!statement.automated);
    assert_eq!(statement.until, None);
    assert_eq!(statement.appeal.status, AppealStatus::Available);
    assert!(statement.appeal.deadline > now() + 100 * DAY as u64);

    // On the connection...
    let Event::Action { statement: heard } =
        events.until(|e| matches!(e, Event::Action { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(*heard, statement);
    // ...and from the endpoint, which names neither the reporter nor the moderator.
    let stored = my_actions(&cloud, sam.token()).await;
    assert_eq!(stored, vec![statement.clone()]);
    let text = serde_json::to_string(&stored).unwrap();
    assert!(!text.contains("Kim") && !text.contains("Mona"), "{text}");
    // The report is decided by the action.
    let report: ReportView = cloud
        .get(&format!("/v1/moderation/reports/{report_id}"), mona.token())
        .await
        .ok(200)
        .json();
    assert_eq!(report.status, ReportStatus::Decided);
    assert_eq!(report.outcome, Some(ReportOutcome::Actioned));
    assert_eq!(report.action, Some(statement.id));
    assert_eq!(report.decided_by.unwrap().handle, "Mona");
    // Nothing else changed for Sam: still signed in.
    cloud.get("/v1/me", sam.token()).await.ok(200);
    // The audit line.
    let entry = log(&cloud, mona.token()).await.remove(0);
    assert_eq!(entry.action, AuditAction::Warned);
    assert_eq!(entry.actor.handle, "Mona");
    assert_eq!(entry.actor.id.unwrap().0, mona.id);
    assert_eq!(entry.target.unwrap().handle, "Sam");
    assert_eq!(entry.reason, "Do not insult other players.");
    assert_eq!(entry.rule, Some(Rule::Harassment));
    let reference = entry.reference.unwrap();
    assert_eq!(
        (reference.kind, reference.id),
        (ReferenceKind::Action, statement.id)
    );
    // The moderators' list of actions about Sam.
    let about: Vec<ActionView> = cloud
        .get(
            &format!("/v1/moderation/actions?account={}", sam.id),
            mona.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(about.len(), 1);
    assert_eq!(about[0].actor.handle, "Mona");
    // A decided report cannot be decided again.
    cloud
        .post(
            "/v1/moderation/actions",
            mona.token(),
            json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Again.", "report": report_id}),
        )
        .await
        .is_error("limit", 409);
    cloud
        .post(
            &format!("/v1/moderation/reports/{report_id}/dismiss"),
            mona.token(),
            json!({"reason": "Late."}),
        )
        .await
        .is_error("limit", 409);
}

#[tokio::test]
async fn a_dismissed_report_is_decided_and_logged() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let (kim, sam) = (cloud.sign_up("Kim").await, cloud.sign_up("Sam").await);
    file_report(&cloud, &kim, &sam).await;
    let id = queue(&cloud, mona.token(), "open").await[0].id;
    cloud
        .post(
            &format!("/v1/moderation/reports/{id}/dismiss"),
            mona.token(),
            json!({"reason": ""}),
        )
        .await
        .is_error("bad_request", 400);
    let report: ReportView = cloud
        .post(
            &format!("/v1/moderation/reports/{id}/dismiss"),
            mona.token(),
            json!({"reason": "Nothing happened."}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(report.outcome, Some(ReportOutcome::Dismissed));
    assert_eq!(report.status, ReportStatus::Decided);
    assert!(queue(&cloud, mona.token(), "open").await.is_empty());
    let entry = log(&cloud, mona.token()).await.remove(0);
    assert_eq!(entry.action, AuditAction::ReportDismissed);
    assert_eq!(entry.reason, "Nothing happened.");
    assert_eq!(entry.target.unwrap().handle, "Sam");
    // Sam is told nothing: nothing was done to him.
    assert!(my_actions(&cloud, sam.token()).await.is_empty());
}

#[tokio::test]
async fn a_mute_stops_friend_requests_until_it_ends_or_is_lifted() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let (sam, kim) = (cloud.sign_up("Sam").await, cloud.sign_up("Kim").await);
    let mut events = cloud.events(sam.token()).await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "mute", "rule": "spam", "reason": "Too many requests.", "days": 2}),
    )
    .await;
    let until = view.statement.until.unwrap();
    assert!(until >= now() + 2 * DAY as u64 - 5);
    let Event::Action { statement } = events.until(|e| matches!(e, Event::Action { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(statement.kind, ActionKind::Mute);

    let refused = cloud
        .post(
            "/v1/friends/requests",
            sam.token(),
            json!({"handle": kim.handle}),
        )
        .await;
    refused.is_error("restricted", 403);
    assert_eq!(refused.body["restriction"], "muted");
    assert_eq!(refused.body["until"], until);
    // He is still signed in and can do everything else.
    cloud.get("/v1/me", sam.token()).await.ok(200);
    assert_eq!(
        log(&cloud, mona.token()).await[0].action,
        AuditAction::Muted
    );

    // Lifting ends it, tells him and is logged.
    let lifted: ActionView = cloud
        .post(
            &format!("/v1/moderation/actions/{}/lift", view.statement.id),
            mona.token(),
            json!({"reason": "Enough."}),
        )
        .await
        .ok(200)
        .json();
    assert!(lifted.statement.lifted.is_some());
    let Event::Action { statement } = events.until(|e| matches!(e, Event::Action { .. })).await
    else {
        unreachable!()
    };
    assert!(statement.lifted.is_some());
    assert_eq!(
        log(&cloud, mona.token()).await[0].action,
        AuditAction::Lifted
    );
    cloud
        .post(
            "/v1/friends/requests",
            sam.token(),
            json!({"handle": kim.handle}),
        )
        .await
        .ok(200);
    cloud
        .post(
            &format!("/v1/moderation/actions/{}/lift", view.statement.id),
            mona.token(),
            json!({"reason": "Again."}),
        )
        .await
        .is_error("limit", 409);

    // A mute that ran out stops counting by itself.
    let again = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "mute", "rule": "spam", "reason": "Once more.", "days": 1}),
    )
    .await;
    let bea = cloud.sign_up("Bea").await;
    cloud
        .post(
            "/v1/friends/requests",
            sam.token(),
            json!({"handle": bea.handle}),
        )
        .await
        .is_error("restricted", 403);
    sql(
        &cloud,
        "UPDATE actions SET until = $1 WHERE id = $2",
        now() as i64 - 10,
        again.statement.id as i64,
    )
    .await;
    cloud
        .post(
            "/v1/friends/requests",
            sam.token(),
            json!({"handle": bea.handle}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_ban_closes_the_connection_and_shows_the_player_why() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let sam = cloud.sign_up("Sam").await;
    let mut events = cloud.events(sam.token()).await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "ban", "rule": "cheating",
               "reason": "Aimbot in a ranked match.", "days": 7}),
    )
    .await;
    let until = view.statement.until.unwrap();
    assert!(until > now() + 6 * DAY as u64);

    let Event::Banned {
        until: heard,
        statement,
    } = events.until(|e| matches!(e, Event::Banned { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(heard, Some(until));
    assert_eq!(*statement.unwrap(), view.statement);
    assert_eq!(events.close_code().await, Some(close::BANNED));

    // Every request answers with the reason and a token to appeal with.
    let reply = cloud.get("/v1/me", sam.token()).await;
    reply.is_error("banned", 403);
    assert_eq!(reply.body["until"], until);
    assert_eq!(
        reply.body["ban"]["statement"]["reason"],
        "Aimbot in a ranked match."
    );
    assert_eq!(reply.body["ban"]["statement"]["rule"], "cheating");
    // So do sign-in (after the e-mail code) and refresh.
    let (challenge, code) = cloud.ask_for_code(&sam.email).await;
    let signin = cloud.enter_code(&challenge.challenge, &code, "pc").await;
    signin.is_error("banned", 403);
    assert_eq!(signin.body["ban"]["statement"]["id"], view.statement.id);
    let refresh = cloud
        .request(
            reqwest::Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": sam.tokens.refresh})),
        )
        .await;
    refresh.is_error("banned", 403);
    let token = refresh.body["ban"]["appeal_token"]
        .as_str()
        .unwrap()
        .to_owned();
    // The appeal token reads the statements, and does nothing else.
    assert_eq!(my_actions(&cloud, &token).await[0].id, view.statement.id);
    cloud
        .get("/v1/me", &token)
        .await
        .is_error("unauthorized", 401);
    cloud
        .get("/v1/friends", &token)
        .await
        .is_error("unauthorized", 401);
    // The still-signed-in device may read them too.
    assert_eq!(my_actions(&cloud, sam.token()).await.len(), 1);
    assert_eq!(
        log(&cloud, mona.token()).await[0].action,
        AuditAction::Banned
    );
    // The ban for good has no end.
    let tia = cloud.sign_up("Tia").await;
    let forever = take_action(
        &cloud,
        mona.token(),
        json!({"account": tia.id, "kind": "ban", "rule": "illegal", "reason": "Unlawful content."}),
    )
    .await;
    assert_eq!(forever.statement.until, None);
    cloud
        .get("/v1/me", tia.token())
        .await
        .is_error("banned", 403);
}

// Appeals

#[tokio::test]
async fn an_appeal_is_decided_by_another_moderator_and_every_step_is_logged() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let max = moderator(&cloud, "Max").await;
    let sam = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "ban", "rule": "spam", "reason": "Link spam."}),
    )
    .await;
    let id = view.statement.id;
    let appeal_path = format!("/v1/me/actions/{id}/appeal");
    let rejected = cloud.get("/v1/me", sam.token()).await;
    let token = rejected.body["ban"]["appeal_token"]
        .as_str()
        .unwrap()
        .to_owned();

    // He appeals with the token the ban gave him.
    cloud
        .post(&appeal_path, &token, json!({"text": "   "}))
        .await
        .is_error("bad_request", 400);
    let statement: Statement = cloud
        .post(
            &appeal_path,
            &token,
            json!({"text": "It was a shared clip, not spam."}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(statement.appeal.status, AppealStatus::Open);
    assert_eq!(
        statement.appeal.text.as_deref(),
        Some("It was a shared clip, not spam.")
    );
    // One appeal per action.
    cloud
        .post(&appeal_path, &token, json!({"text": "Please."}))
        .await
        .is_error("limit", 409);
    // Someone else's action is not his to appeal.
    let kim = cloud.sign_up("Kim").await;
    cloud
        .post(&appeal_path, kim.token(), json!({"text": "Mine."}))
        .await
        .is_error("not_found", 404);

    let open: Vec<AppealView> = cloud
        .get("/v1/moderation/appeals", max.token())
        .await
        .ok(200)
        .json();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].action.statement.id, id);
    assert_eq!(open[0].action.account.handle, "Sam");
    let appeal_id = open[0].id;
    let decision =
        |outcome: &str| json!({"outcome": outcome, "reason": "Looked at the clip again."});
    let decide = format!("/v1/moderation/appeals/{appeal_id}/decision");

    // The moderator who acted does not decide while another one exists.
    cloud
        .post(&decide, mona.token(), decision("reverse"))
        .await
        .is_error("forbidden", 403);
    cloud
        .post(
            &decide,
            max.token(),
            json!({"outcome": "reverse", "reason": ""}),
        )
        .await
        .is_error("bad_request", 400);
    let decided: AppealView = cloud
        .post(&decide, max.token(), decision("reverse"))
        .await
        .ok(200)
        .json();
    assert_eq!(decided.decided_by.unwrap().handle, "Max");
    let AppealInfo {
        status,
        answer,
        decided,
        ..
    } = decided.action.statement.appeal;
    assert_eq!(status, AppealStatus::Reversed);
    assert_eq!(answer.as_deref(), Some("Looked at the clip again."));
    assert!(decided.is_some());
    // Reversing lifted the ban at once.
    cloud.get("/v1/me", sam.token()).await.ok(200);
    let stored = my_actions(&cloud, sam.token()).await;
    assert!(stored[0].lifted.is_some());
    assert_eq!(stored[0].appeal.status, AppealStatus::Reversed);
    cloud
        .post(&decide, max.token(), decision("uphold"))
        .await
        .is_error("limit", 409);
    assert!(
        cloud
            .get("/v1/moderation/appeals", max.token())
            .await
            .ok(200)
            .json::<Vec<AppealView>>()
            .is_empty()
    );
    // Every step is in the log, newest first: lifted, reversed, filed, banned.
    let entries = log(&cloud, max.token()).await;
    let actions: Vec<_> = entries.iter().map(|e| e.action).take(4).collect();
    assert_eq!(
        actions,
        [
            AuditAction::Lifted,
            AuditAction::AppealReversed,
            AuditAction::AppealFiled,
            AuditAction::Banned
        ]
    );
    assert_eq!(entries[1].actor.handle, "Max");
    assert_eq!(entries[2].actor.handle, "Sam");
}

#[tokio::test]
async fn an_upheld_appeal_leaves_the_action_standing() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let max = moderator(&cloud, "Max").await;
    let sam = cloud.sign_up("Sam").await;
    let mut events = cloud.events(sam.token()).await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "warning", "rule": "name", "reason": "Rename."}),
    )
    .await;
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", view.statement.id),
            sam.token(),
            json!({"text": "I like my name."}),
        )
        .await
        .ok(200);
    let appeal: Vec<AppealView> = cloud
        .get("/v1/moderation/appeals", max.token())
        .await
        .ok(200)
        .json();
    // The operator may decide too: a different identity than Mona's.
    cloud
        .post(
            &format!("/v1/moderation/appeals/{}/decision", appeal[0].id),
            ADMIN,
            json!({"outcome": "uphold", "reason": "The name stays against the rules."}),
        )
        .await
        .ok(200);
    let Event::Action { statement } = events
        .until(|e| matches!(e, Event::Action { statement } if statement.appeal.status == AppealStatus::Upheld))
        .await
    else {
        unreachable!()
    };
    assert_eq!(statement.lifted, None);
    assert_eq!(
        statement.appeal.answer.as_deref(),
        Some("The name stays against the rules.")
    );
    assert_eq!(
        log(&cloud, mona.token()).await[0].action,
        AuditAction::AppealUpheld
    );
    let decided: Vec<AppealView> = cloud
        .get("/v1/moderation/appeals?status=upheld", mona.token())
        .await
        .ok(200)
        .json();
    assert_eq!(decided.len(), 1);
}

#[tokio::test]
async fn a_sole_moderator_decides_the_appeal_of_their_own_action() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let sam = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Stop."}),
    )
    .await;
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", view.statement.id),
            sam.token(),
            json!({"text": "No."}),
        )
        .await
        .ok(200);
    let appeal: Vec<AppealView> = cloud
        .get("/v1/moderation/appeals", mona.token())
        .await
        .ok(200)
        .json();
    // There is nobody else, so she may. A moderator cannot decide an appeal about themselves.
    cloud
        .post(
            &format!("/v1/moderation/appeals/{}/decision", appeal[0].id),
            mona.token(),
            json!({"outcome": "uphold", "reason": "Still true."}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn the_operators_own_action_is_not_decided_by_the_operator_when_a_moderator_exists() {
    let cloud = cloud().await;
    let sam = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Stop."}),
    )
    .await;
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", view.statement.id),
            sam.token(),
            json!({"text": "No."}),
        )
        .await
        .ok(200);
    let mona = moderator(&cloud, "Mona").await;
    let appeal: Vec<AppealView> = cloud
        .get("/v1/moderation/appeals", ADMIN)
        .await
        .ok(200)
        .json();
    let path = format!("/v1/moderation/appeals/{}/decision", appeal[0].id);
    let body = json!({"outcome": "reverse", "reason": "Fair."});
    cloud
        .post(&path, ADMIN, body.clone())
        .await
        .is_error("forbidden", 403);
    cloud.post(&path, mona.token(), body).await.ok(200);
}

#[tokio::test]
async fn appeals_close_when_the_time_has_run_out() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let sam = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Stop."}),
    )
    .await;
    sql(
        &cloud,
        "UPDATE actions SET created = $1 WHERE id = $2",
        now() as i64 - 200 * DAY,
        view.statement.id as i64,
    )
    .await;
    assert_eq!(
        my_actions(&cloud, sam.token()).await[0].appeal.status,
        AppealStatus::Expired
    );
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", view.statement.id),
            sam.token(),
            json!({"text": "Late."}),
        )
        .await
        .is_error("forbidden", 403);
}

#[tokio::test]
async fn appeals_are_rate_limited() {
    let cloud = TestCloud::with(
        |c| c.admin_token = Some(ADMIN.into()),
        |t| t.rates.appeal_per_account = 1,
    )
    .await;
    let sam = cloud.sign_up("Sam").await;
    let first = take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "One."}),
    )
    .await;
    let second = take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Two."}),
    )
    .await;
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", first.statement.id),
            sam.token(),
            json!({"text": "No."}),
        )
        .await
        .ok(200);
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", second.statement.id),
            sam.token(),
            json!({"text": "No."}),
        )
        .await
        .is_error("rate_limited", 429);
}

// Deletion, retention, hashes

#[tokio::test]
async fn a_report_survives_the_deletion_of_the_reported_account() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let (kim, sam) = (cloud.sign_up("Kim").await, cloud.sign_up("Sam").await);
    file_report(&cloud, &kim, &sam).await;
    cloud.delete_account_now(&sam).await;

    let open = queue(&cloud, mona.token(), "open").await;
    assert_eq!(open.len(), 1, "the evidence stays");
    assert!(open[0].reported_deleted);
    assert_eq!(open[0].reported.handle, "Sam");
    assert_eq!(open[0].reported.id.0, sam.id);
    assert_eq!(open[0].reporter.as_ref().unwrap().handle, "Kim");
    assert!(open[0].content.contains("Sam"));
    // The reporter leaving does not take it either; only the reporter is forgotten.
    cloud.delete_account_now(&kim).await;
    let open = queue(&cloud, mona.token(), "open").await;
    assert_eq!(open.len(), 1);
    assert!(open[0].reporter.is_none());
    // It can still be decided.
    cloud
        .post(
            &format!("/v1/moderation/reports/{}/dismiss", open[0].id),
            mona.token(),
            json!({"reason": "The account is gone."}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_ban_and_its_statement_outlive_the_account() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let sam = cloud.sign_up("Sam").await;
    let view = take_action(
        &cloud,
        mona.token(),
        json!({"account": sam.id, "kind": "ban", "rule": "cheating", "reason": "Cheating."}),
    )
    .await;
    // He is banned, so deleting is refused; the operator may remove the account by hand.
    cloud
        .delete("/v1/me", sam.token())
        .await
        .is_error("banned", 403);
    cloud.state.delete_account(sam.id).await.unwrap();
    let (challenge, code) = cloud.ask_for_code(&sam.email).await;
    let reply = cloud.enter_code(&challenge.challenge, &code, "pc").await;
    reply.is_error("banned", 403);
    assert_eq!(reply.body["ban"]["statement"]["id"], view.statement.id);
}

#[tokio::test]
async fn retention_deletes_what_is_past_its_period_and_keeps_the_rest() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let people: Vec<User> = {
        let mut v = Vec::new();
        for name in ["Aa1", "Bb2", "Cc3", "Dd4", "Ee5", "Ff6"] {
            v.push(cloud.sign_up(name).await);
        }
        v
    };
    let [old_ban, warned, forever, kept_ban, open_old, decided_old] = &people[..] else {
        unreachable!()
    };
    let reporter = cloud.sign_up("Rep").await;

    // Actions: an ended ban, an old warning, a ban for good, a recently ended ban.
    let ended = take_action(
        &cloud,
        mona.token(),
        json!({"account": old_ban.id, "kind": "ban", "rule": "spam", "reason": "Old.", "days": 1}),
    )
    .await;
    let warning = take_action(
        &cloud,
        mona.token(),
        json!({"account": warned.id, "kind": "warning", "rule": "spam", "reason": "Old."}),
    )
    .await;
    let permanent = take_action(
        &cloud,
        mona.token(),
        json!({"account": forever.id, "kind": "ban", "rule": "spam", "reason": "Forever."}),
    )
    .await;
    let recent = take_action(&cloud, mona.token(), json!({"account": kept_ban.id, "kind": "ban", "rule": "spam", "reason": "Recent.", "days": 1})).await;
    let long_ago = now() as i64 - 200 * DAY;
    sql(
        &cloud,
        "UPDATE actions SET created = $1, until = $1 + 86400 WHERE id = $2",
        long_ago,
        ended.statement.id as i64,
    )
    .await;
    sql(
        &cloud,
        "UPDATE bans SET until = $1 WHERE action_id = $2",
        long_ago + 86_400,
        ended.statement.id as i64,
    )
    .await;
    sql(
        &cloud,
        "UPDATE actions SET created = $1 WHERE id = $2",
        long_ago,
        warning.statement.id as i64,
    )
    .await;
    sql(
        &cloud,
        "UPDATE actions SET until = $1 WHERE id = $2",
        now() as i64 - 10 * DAY,
        recent.statement.id as i64,
    )
    .await;
    sql(
        &cloud,
        "UPDATE bans SET until = $1 WHERE action_id = $2",
        now() as i64 - 10 * DAY,
        recent.statement.id as i64,
    )
    .await;

    // Reports: decided long ago, decided recently, open for over a year, open for a month.
    for target in [decided_old, open_old, warned] {
        file_report(&cloud, &reporter, target).await;
    }
    let reports = queue(&cloud, mona.token(), "open").await;
    let id_of = |handle: &str| {
        reports
            .iter()
            .find(|r| r.reported.handle == handle)
            .unwrap()
            .id as i64
    };
    for handle in [decided_old.handle.as_str(), warned.handle.as_str()] {
        cloud
            .post(
                &format!("/v1/moderation/reports/{}/dismiss", id_of(handle)),
                mona.token(),
                json!({"reason": "No."}),
            )
            .await
            .ok(200);
    }
    sql(
        &cloud,
        "UPDATE reports SET decided = $1 WHERE id = $2",
        long_ago,
        id_of(&decided_old.handle),
    )
    .await;
    sql(
        &cloud,
        "UPDATE reports SET created = $1 WHERE id = $2",
        now() as i64 - 400 * DAY,
        id_of(&open_old.handle),
    )
    .await;

    // The log: one line from four years ago.
    sqlx::query("INSERT INTO audit_log (created, actor_handle, action, reason) VALUES ($1, 'operator', 'banned', 'ancient')")
        .bind(now() as i64 - 4 * 365 * DAY)
        .execute(&cloud.state.db)
        .await
        .unwrap();

    let swept = moderation::sweep_retention(&cloud.state).await.unwrap();
    assert_eq!(swept.bans, 1, "{swept:?}");
    assert_eq!(swept.actions, 2, "{swept:?}");
    assert_eq!(swept.reports, 2, "{swept:?}");
    assert_eq!(swept.log_lines, 1, "{swept:?}");

    let actions = count(&cloud, "SELECT COUNT(*) FROM actions").await;
    assert_eq!(actions, 2, "the ban for good and the recent one stay");
    assert_eq!(
        count(&cloud, "SELECT COUNT(*) FROM bans").await,
        2,
        "the ban for good and the recent one"
    );
    let remaining: Vec<ReportView> = queue(&cloud, mona.token(), "all").await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].reported.handle, warned.handle);
    assert!(
        log(&cloud, mona.token())
            .await
            .iter()
            .all(|e| e.reason != "ancient")
    );
    // The ban for good still holds; the ended one is forgotten, so its owner is free.
    cloud
        .get("/v1/me", forever.token())
        .await
        .is_error("banned", 403);
    assert_eq!(permanent.statement.until, None);
    // Nothing is left to sweep.
    assert_eq!(
        moderation::sweep_retention(&cloud.state).await.unwrap(),
        Default::default()
    );
}

#[tokio::test]
async fn the_periods_are_settings_and_an_open_appeal_keeps_its_action() {
    let cloud = TestCloud::with(
        |c| {
            c.admin_token = Some(ADMIN.into());
            c.retention_bans_days = 10;
            c.appeal_days = 5;
            c.retention_reports_days = 10;
        },
        |_| {},
    )
    .await;
    let sam = cloud.sign_up("Sam").await;
    let kim = cloud.sign_up("Kim").await;
    let appealed = take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "A."}),
    )
    .await;
    let plain = take_action(
        &cloud,
        ADMIN,
        json!({"account": kim.id, "kind": "warning", "rule": "spam", "reason": "B."}),
    )
    .await;
    cloud
        .post(
            &format!("/v1/me/actions/{}/appeal", appealed.statement.id),
            sam.token(),
            json!({"text": "No."}),
        )
        .await
        .ok(200);
    for id in [appealed.statement.id, plain.statement.id] {
        sql(
            &cloud,
            "UPDATE actions SET created = $1 WHERE id = $2",
            now() as i64 - 11 * DAY,
            id as i64,
        )
        .await;
    }
    let swept = moderation::sweep_retention(&cloud.state).await.unwrap();
    assert_eq!(swept.actions, 1, "only the one nobody appealed");
    assert_eq!(my_actions(&cloud, sam.token()).await.len(), 1);
    assert!(my_actions(&cloud, kim.token()).await.is_empty());
}

#[tokio::test]
async fn the_audit_log_cannot_be_changed() {
    let cloud = cloud().await;
    let sam = cloud.sign_up("Sam").await;
    take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "A."}),
    )
    .await;
    let changed = sqlx::query("UPDATE audit_log SET reason = 'nothing happened'")
        .execute(&cloud.state.db)
        .await;
    assert!(changed.is_err(), "an update of the log must fail");
    assert_eq!(log(&cloud, ADMIN).await[0].reason, "A.");
}

#[tokio::test]
async fn banned_addresses_are_stored_as_keyed_hashes_and_older_hashes_still_match() {
    let cloud = cloud().await;
    let sam = cloud.sign_up("Sam").await;
    take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "ban", "rule": "spam", "reason": "A."}),
    )
    .await;
    let (hash, salted): (Vec<u8>, i64) = sqlx::query_as("SELECT email_hash, salted FROM bans")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(salted, 1);
    assert_ne!(
        hash,
        sha256(sam.email.as_bytes()).to_vec(),
        "not the plain SHA-256"
    );
    assert_eq!(hash, cloud.state.ban_key.hash(&sam.email).to_vec());

    // A ban from before the key: plain hash, no statement, account since deleted.
    let kim = cloud.sign_up("Kim").await;
    cloud.state.delete_account(kim.id).await.unwrap();
    sqlx::query("INSERT INTO bans (account_id, email_hash, until, reason, created, salted) VALUES ($1, $2, NULL, 'old', $3, 0)")
        .bind(kim.id as i64)
        .bind(sha256(kim.email.as_bytes()).to_vec())
        .bind(now() as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let (challenge, code) = cloud.ask_for_code(&kim.email).await;
    let reply = cloud.enter_code(&challenge.challenge, &code, "pc").await;
    reply.is_error("banned", 403);
    assert!(
        reply.body["ban"]["statement"].is_null(),
        "an old ban has no statement"
    );
    assert!(reply.body["ban"]["appeal_token"].is_string());
    // Seeing the address again replaced the old hash.
    let (_, salted): (Vec<u8>, i64) =
        sqlx::query_as("SELECT email_hash, salted FROM bans WHERE account_id = $1")
            .bind(kim.id as i64)
            .fetch_one(&cloud.state.db)
            .await
            .unwrap();
    assert_eq!(salted, 1);

    // And one whose account still exists is replaced at start.
    let lee = cloud.sign_up("Lee").await;
    sqlx::query("INSERT INTO bans (account_id, email_hash, until, reason, created, salted) VALUES ($1, $2, NULL, 'old', $3, 0)")
        .bind(lee.id as i64)
        .bind(sha256(lee.email.as_bytes()).to_vec())
        .bind(now() as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let done = moderation::banhash::resalt_known(
        &mut cloud.state.db.acquire().await.unwrap(),
        &cloud.state.ban_key,
    )
    .await
    .unwrap();
    assert_eq!(done, 1);
}

// New accounts

#[tokio::test]
async fn a_new_account_may_send_ten_friend_requests_on_its_first_day_and_not_an_eleventh() {
    let cloud = TestCloud::with(|c| c.new_account_hours = 24, |_| {}).await;
    let nils = cloud.sign_up("Nils").await;
    let mut targets = Vec::new();
    for k in 0..11 {
        targets.push(cloud.sign_up(&format!("Pal{k}")).await);
    }
    for target in &targets[..10] {
        cloud
            .post(
                "/v1/friends/requests",
                nils.token(),
                json!({"handle": target.handle}),
            )
            .await
            .ok(200);
    }
    let refused = cloud
        .post(
            "/v1/friends/requests",
            nils.token(),
            json!({"handle": targets[10].handle}),
        )
        .await;
    refused.is_error("restricted", 403);
    assert_eq!(refused.body["restriction"], "new_account");
    let until = refused.body["until"].as_u64().unwrap();
    assert!(
        until > now() + 23 * 3600 && until <= now() + 24 * 3600,
        "until {until}"
    );
    assert_eq!(
        cloud.get("/v1/friends", nils.token()).await.ok(200).body["outgoing"]
            .as_array()
            .unwrap()
            .len(),
        10,
        "the refused request was not stored"
    );
    // Answering a request that waits is no new request: the eleventh asks him first.
    cloud
        .post(
            "/v1/friends/requests",
            targets[10].token(),
            json!({"handle": nils.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(
            &format!("/v1/friends/requests/{}/accept", targets[10].id),
            nils.token(),
        )
        .await
        .ok(204);
    // When the day is over the request goes through.
    age(&cloud, &nils).await;
    let more = cloud.sign_up("Late").await;
    cloud
        .post(
            "/v1/friends/requests",
            nils.token(),
            json!({"handle": more.handle}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn an_old_account_is_not_counted_and_the_allowance_is_a_setting() {
    let cloud = TestCloud::with(
        |c| {
            c.new_account_hours = 24;
            c.new_account_friend_requests_per_day = 2;
        },
        |_| {},
    )
    .await;
    let old = cloud.sign_up("Olga").await;
    age(&cloud, &old).await;
    let new = cloud.sign_up("Nils").await;
    for k in 0..4 {
        let other = cloud.sign_up(&format!("Pal{k}")).await;
        cloud
            .post(
                "/v1/friends/requests",
                old.token(),
                json!({"handle": other.handle}),
            )
            .await
            .ok(200);
    }
    let a = cloud.sign_up("Ann").await;
    let b = cloud.sign_up("Bob").await;
    let c = cloud.sign_up("Cid").await;
    for target in [&a, &b] {
        cloud
            .post(
                "/v1/friends/requests",
                new.token(),
                json!({"handle": target.handle}),
            )
            .await
            .ok(200);
    }
    cloud
        .post(
            "/v1/friends/requests",
            new.token(),
            json!({"handle": c.handle}),
        )
        .await
        .is_error("restricted", 403);
    assert_eq!(
        count(&cloud, "SELECT COUNT(*) FROM new_account_requests").await,
        2,
        "only the new account is recorded"
    );
}

#[tokio::test]
async fn a_new_account_cannot_message_a_stranger_but_can_message_a_friend() {
    let cloud = TestCloud::with(|c| c.new_account_hours = 24, |_| {}).await;
    let (nils, bea) = cloud.friends("Nils", "Bea").await;
    let stranger = cloud.sign_up("Sam").await;
    let refused = open_chat(&cloud, &nils, &stranger).await;
    refused.is_error("restricted", 403);
    assert_eq!(refused.body["restriction"], "new_account");
    assert!(refused.body["until"].as_u64().unwrap() > now());
    // A friend is fine at once.
    let chat = open_chat(&cloud, &nils, &bea).await.ok(200).body;
    let id = chat["id"].as_str().unwrap().to_owned();
    cloud
        .post(
            &format!("/v1/conversations/{id}/messages"),
            nils.token(),
            json!({"text": "hello"}),
        )
        .await
        .ok(200);
    // One function, one setting.
    let err = moderation::may_write_to_strangers(&cloud.state, nils.id)
        .await
        .unwrap_err();
    assert_eq!(err.body.error.as_str(), "restricted");
    // A day later a stranger may be written to.
    age(&cloud, &nils).await;
    open_chat(&cloud, &nils, &stranger).await.ok(200);
}

#[tokio::test]
async fn a_muted_account_sends_no_friend_request_and_no_message() {
    let cloud = cloud().await;
    let (sam, kim) = cloud.friends("Sam", "Kim").await;
    let lee = cloud.sign_up("Lee").await;
    let chat = cloud
        .post(
            "/v1/conversations",
            sam.token(),
            json!({"accounts": [kim.id]}),
        )
        .await
        .ok(200)
        .body;
    let id = chat["id"].as_str().unwrap().to_owned();
    say(&cloud, &sam, &id, "before").await.ok(200);
    take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "mute", "rule": "spam", "reason": "Quiet.", "days": 1}),
    )
    .await;
    let refused = say(&cloud, &sam, &id, "after").await;
    refused.is_error("restricted", 403);
    assert_eq!(refused.body["restriction"], "muted");
    cloud
        .post(
            "/v1/conversations",
            sam.token(),
            json!({"accounts": [lee.id]}),
        )
        .await
        .is_error("restricted", 403);
    cloud
        .post(
            "/v1/friends/requests",
            sam.token(),
            json!({"handle": lee.handle}),
        )
        .await
        .is_error("restricted", 403);
    // His friend can still write to him.
    say(&cloud, &kim, &id, "hi").await.ok(200);
}

#[tokio::test]
async fn a_report_about_a_message_keeps_its_text() {
    let cloud = cloud().await;
    let mona = moderator(&cloud, "Mona").await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let outsider = cloud.sign_up("Zed").await;
    let chat = cloud
        .post(
            "/v1/conversations",
            alec.token(),
            json!({"accounts": [bea.id]}),
        )
        .await
        .ok(200)
        .body;
    let id = chat["id"].as_str().unwrap().to_owned();
    say(&cloud, &alec, &id, "fine").await.ok(200);
    let rude = say(&cloud, &bea, &id, "you are the worst")
        .await
        .ok(200)
        .body["seq"]
        .as_u64()
        .unwrap();
    // Both fields or none; a message of the other account; a conversation the reporter is not in.
    report_message(&cloud, &alec, &bea, json!(id), Value::Null)
        .await
        .is_error("bad_request", 400);
    report_message(&cloud, &alec, &bea, json!(id), json!(1))
        .await
        .is_error("bad_request", 400);
    report_message(&cloud, &alec, &bea, json!(id), json!(99))
        .await
        .is_error("not_found", 404);
    report_message(&cloud, &outsider, &bea, json!(id), json!(rude))
        .await
        .is_error("not_found", 404);
    report_message(&cloud, &alec, &bea, json!(id), json!(rude))
        .await
        .ok(204);
    let open = queue(&cloud, mona.token(), "open").await;
    assert_eq!(open.len(), 1, "the refused reports left nothing");
    assert!(
        open[0].content.contains("you are the worst"),
        "{}",
        open[0].content
    );
    assert!(open[0].content.contains(&id));
    // The message goes with its retention; the report keeps the words.
    sqlx::query("DELETE FROM messages")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let again = queue(&cloud, mona.token(), "open").await;
    assert!(again[0].content.contains("you are the worst"));
}

#[tokio::test]
async fn the_minimum_age_is_a_setting_and_zero_turns_it_off() {
    let cloud = TestCloud::with(|c| c.new_account_hours = 0, |_| {}).await;
    let (a, b) = (cloud.sign_up("Alec").await, cloud.sign_up("Bea").await);
    cloud
        .post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": b.handle}),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn a_player_reads_their_actions_in_the_export() {
    let cloud = cloud().await;
    let sam = cloud.sign_up("Sam").await;
    take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "warning", "rule": "spam", "reason": "Noted."}),
    )
    .await;
    let export = cloud.get("/v1/me/export", sam.token()).await.ok(200).body;
    assert_eq!(export["actions_against_you"][0]["reason"], "Noted.");
}

// Two nodes

#[tokio::test]
async fn a_ban_on_one_node_closes_the_players_connection_on_the_other() {
    let Some(mut nodes) = TestCloud::cluster(2, |c| c.admin_token = Some(ADMIN.into())).await
    else {
        eprintln!("no test database and NATS server; skipped");
        return;
    };
    let two = nodes.pop().unwrap();
    let one = nodes.pop().unwrap();
    let mona = moderator(&one, "Mona").await;
    let sam = one.sign_up("Sam").await;
    // Sam listens through the second node; Mona acts through the first.
    let mut on_two = two.events(sam.token()).await;
    let view = take_action(
        &one,
        mona.token(),
        json!({"account": sam.id, "kind": "ban", "rule": "harassment", "reason": "Threats.", "days": 3}),
    )
    .await;
    let Event::Banned { statement, .. } = on_two.until(|e| matches!(e, Event::Banned { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(*statement.unwrap(), view.statement);
    assert_eq!(on_two.close_code().await, Some(close::BANNED));
    // The second node refuses him too, and shows the reason.
    let mut banned = false;
    for _ in 0..50 {
        let reply = two.get("/v1/me", sam.token()).await;
        if reply.status == 403 {
            assert_eq!(reply.body["ban"]["statement"]["reason"], "Threats.");
            banned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(banned, "the second node knows the ban");
    // A moderator of the second node sees the same audit log.
    let entries = log(&two, ADMIN).await;
    assert_eq!(entries[0].action, AuditAction::Banned);
    // Lifting on the second node frees him on the first.
    two.post(
        &format!("/v1/moderation/actions/{}/lift", view.statement.id),
        ADMIN,
        json!({"reason": "Mistake."}),
    )
    .await
    .ok(200);
    let mut free = false;
    for _ in 0..50 {
        if one.get("/v1/me", sam.token()).await.status == 200 {
            free = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(free, "the first node learned of the lifting");
}

// The migration

#[tokio::test]
async fn migration_0005_keeps_the_bans_and_reports_that_exist_and_gives_bans_a_statement() {
    use sqlx::any::{AnyPoolOptions, install_default_drivers};
    use sqlx::migrate::Migrator;
    use std::path::Path;

    install_default_drivers();
    let dir = tempfile::tempdir().unwrap();
    // Held until the end of the test: dropping it drops the database.
    let mut _database = None;
    let (flavour, url) = match common::postgres_database().await {
        Some(database) => {
            let url = database.url.clone();
            _database = Some(database);
            ("postgres", url)
        }
        None => (
            "sqlite",
            format!("sqlite:{}?mode=rwc", dir.path().join("old.db").display())
                .replace(std::path::MAIN_SEPARATOR, "/"),
        ),
    };
    let full = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join(flavour);
    // The schema as it was before this change: migrations 0001 to 0003.
    let old = dir.path().join("old-migrations");
    std::fs::create_dir_all(&old).unwrap();
    for entry in std::fs::read_dir(&full).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("0004") && !name.starts_with("0005") && name.ends_with(".sql") {
            std::fs::copy(&path, old.join(&name)).unwrap();
        }
    }
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    Migrator::new(old.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();

    for (id, handle) in [(10_i64, "Kim"), (11, "Sam")] {
        sqlx::query("INSERT INTO accounts (id, handle, handle_key, email, created) VALUES ($1, $2, $3, $4, 5)")
            .bind(id)
            .bind(handle)
            .bind(handle.to_lowercase())
            .bind(format!("{}@example.com", handle.to_lowercase()))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO reports (reporter, reported, reason, details, created) VALUES (10, 11, 'spam', 'links', 7)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bans (account_id, email_hash, until, reason, created) VALUES (11, $1, NULL, 'Link spam.', 9)")
        .bind(sha256(b"sam@example.com").to_vec())
        .execute(&pool)
        .await
        .unwrap();

    Migrator::new(full.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();

    let (handle, reported_handle, status, content): (String, String, String, String) =
        sqlx::query_as("SELECT reporter_handle, reported_handle, status, content FROM reports")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (
            handle.as_str(),
            reported_handle.as_str(),
            status.as_str(),
            content.as_str()
        ),
        ("Kim", "Sam", "open", "")
    );
    let (kind, reason, handle, actor): (String, String, String, String) = sqlx::query_as(
        "SELECT x.kind, x.reason, x.account_handle, x.actor_handle FROM bans b JOIN actions x ON x.id = b.action_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            kind.as_str(),
            reason.as_str(),
            handle.as_str(),
            actor.as_str()
        ),
        ("ban", "Link spam.", "Sam", "operator")
    );
    let (salted,): (i64,) = sqlx::query_as("SELECT salted FROM bans")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(salted, 0, "the old hash is plain until the cloud starts");
    let (lines,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE action = 'banned'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lines, 1);
    // A report no longer follows its accounts.
    sqlx::query("DELETE FROM accounts WHERE id = 11")
        .execute(&pool)
        .await
        .unwrap();
    let (reports,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM reports")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reports, 1);
}

#[tokio::test]
async fn a_muted_account_cannot_call() {
    let cloud = cloud().await;
    let (sam, kim) = cloud.friends("Sam", "Kim").await;
    take_action(
        &cloud,
        ADMIN,
        json!({"account": sam.id, "kind": "mute", "rule": "spam", "reason": "Quiet.", "days": 1}),
    )
    .await;
    let refused = cloud
        .post(
            "/v1/party/invitations",
            sam.token(),
            json!({"to": [kim.id], "call": true}),
        )
        .await;
    refused.is_error("restricted", 403);
    assert_eq!(refused.body["restriction"], "muted");
}
