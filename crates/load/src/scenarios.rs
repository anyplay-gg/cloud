//! The scenarios: a login wave, a friend graph, presence churn, sessions and a reconnect storm.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyplay_api::{CreateSession, FriendRequest, FriendRequestAnswer, FriendState, JoinSession};
use anyplay_api::{Session, Visibility};
use reqwest::Method;
use serde_json::json;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinSet;

use crate::client::{Ctx, Shared, StormTrack, conn_loop, login};
use crate::opts::{Options, Reconnect, StormKind};
use crate::report::{DeliveryReport, RouteStat, ScenarioReport, ServerReport, StormReport};
use crate::stats::Recorder;
use crate::sys::Sampler;
use crate::target::{MetricsSnap, parse_metrics};

type Scrape = Vec<Option<MetricsSnap>>;

async fn scrape(ctx: &Ctx) -> Scrape {
    let mut out = Vec::new();
    for node in &ctx.target.nodes {
        let snap = match (&node.metrics, node.alive()) {
            (Some(address), true) => match ctx
                .http
                .get(format!("http://{address}/metrics"))
                .send()
                .await
            {
                Ok(response) => response.text().await.ok().map(|t| parse_metrics(&t)),
                Err(_) => None,
            },
            _ => None,
        };
        out.push(snap);
    }
    out
}

fn server_delta(before: &Scrape, after: &Scrape) -> ServerReport {
    let mut report = ServerReport::default();
    let mut routes: BTreeMap<(String, String), (u64, f64)> = BTreeMap::new();
    for (b, a) in before.iter().zip(after) {
        let (Some(b), Some(a)) = (b, a) else { continue };
        report.store_errors += a.store_errors.saturating_sub(b.store_errors);
        report.sweep_failures += a.sweep_failures.saturating_sub(b.sweep_failures);
        report.news_sent += a.news_sent.saturating_sub(b.news_sent);
        report.news_received += a.news_received.saturating_sub(b.news_received);
        report.event_connections_end += a.connections;
        for (key, (count, secs)) in &a.routes {
            let (c0, s0) = b.routes.get(key).copied().unwrap_or_default();
            let entry = routes.entry(key.clone()).or_default();
            entry.0 += count.saturating_sub(c0);
            entry.1 += (secs - s0).max(0.0);
        }
    }
    let mut stats: Vec<RouteStat> = routes
        .into_iter()
        .filter(|(_, (count, _))| *count > 0)
        .map(|((route, status), (count, secs))| RouteStat {
            route,
            status,
            count,
            mean_ms: secs / count as f64 * 1000.0,
            total_s: secs,
        })
        .collect();
    stats.sort_by(|a, b| b.total_s.total_cmp(&a.total_s));
    stats.truncate(8);
    report.top_routes = stats;
    report
}

/// A scenario's bookkeeping: its recorder, when it began, and what the nodes said before.
struct Run {
    name: &'static str,
    rec: Arc<Recorder>,
    started: Instant,
    before: Scrape,
    players: usize,
}

impl Run {
    async fn begin(ctx: &Ctx, name: &'static str) -> Self {
        let before = scrape(ctx).await;
        Self {
            name,
            rec: ctx.new_recorder(),
            started: Instant::now(),
            before,
            players: ctx.players.read().unwrap_or_else(|e| e.into_inner()).len(),
        }
    }

    async fn finish(
        self,
        ctx: &Ctx,
        sampler: &Sampler,
        delivery: Option<DeliveryReport>,
        storm: Option<StormReport>,
        notes: Vec<String>,
    ) -> ScenarioReport {
        let ended = Instant::now();
        let seconds = ended.duration_since(self.started).as_secs_f64();
        let after = scrape(ctx).await;
        let summary = self.rec.summarize();
        let mut counters = summary.counters;
        counters.retain(|_, n| *n > 0);
        ScenarioReport {
            name: self.name.into(),
            seconds,
            players: self.players,
            connections_held: ctx.connected.load(Ordering::Relaxed),
            http_calls: summary.http_calls,
            http_per_s: summary.http_calls as f64 / seconds.max(0.001),
            ops: summary.ops,
            http_status: summary.status_total,
            events: summary.events,
            counters,
            delivery,
            storm,
            server: server_delta(&self.before, &after),
            process: sampler.usage(self.started, ended),
            pool: sampler.pool_use(self.started, ended),
            notes,
        }
    }
}

fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let j = (rand::random::<f64>() * (i + 1) as f64) as usize;
        items.swap(i, j.min(i));
    }
}

/// Runs `f(0..n)`, starting one every `1/rate` seconds when there is a rate, and at most
/// `concurrency` at once. Returns when all have finished.
async fn run_batch<F, Fut>(n: usize, rate: Option<f64>, concurrency: usize, f: F)
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let f = Arc::new(f);
    let sem = Arc::new(Semaphore::new(concurrency.clamp(1, Semaphore::MAX_PERMITS)));
    let start = tokio::time::Instant::now();
    let mut set = JoinSet::new();
    for i in 0..n {
        if let Some(rate) = rate {
            tokio::time::sleep_until(start + Duration::from_secs_f64(i as f64 / rate)).await;
        }
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break;
        };
        let f = f.clone();
        set.spawn(async move {
            f(i).await;
            drop(permit);
        });
        while set.try_join_next().is_some() {}
    }
    while set.join_next().await.is_some() {}
}

/// Starts `rate` operations per second for `seconds`, whatever the cloud does: when `cap` are in
/// flight, further ones are counted as skipped instead of queued. Waits for the rest to finish.
async fn open_loop<F, Fut>(rate: f64, seconds: f64, cap: usize, rec: Arc<Recorder>, f: F)
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let f = Arc::new(f);
    let sem = Arc::new(Semaphore::new(cap.clamp(1, Semaphore::MAX_PERMITS)));
    let start = Instant::now();
    let mut issued = 0usize;
    let mut set = JoinSet::new();
    while start.elapsed().as_secs_f64() < seconds {
        let due = (start.elapsed().as_secs_f64() * rate) as usize;
        while issued < due {
            match sem.clone().try_acquire_owned() {
                Ok(permit) => {
                    let f = f.clone();
                    let n = issued;
                    set.spawn(async move {
                        f(n).await;
                        drop(permit);
                    });
                }
                Err(_) => rec.count("skipped_saturated", 1),
            }
            issued += 1;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        while set.try_join_next().is_some() {}
    }
    rec.count("offered", issued as u64);
    let _ = tokio::time::timeout(Duration::from_secs(60), async {
        while set.join_next().await.is_some() {}
    })
    .await;
}

fn players(ctx: &Ctx) -> Vec<Arc<Shared>> {
    ctx.players
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// P players sign up (or in) at a rate and open their event connection.
pub async fn login_wave(ctx: &Arc<Ctx>, sampler: &Sampler) -> ScenarioReport {
    let opts = &ctx.opts;
    let total = opts.players;
    let returning = (opts.returning * total as f64).round() as usize;
    if returning > 0 {
        // Their accounts exist before the wave; nobody measures that.
        ctx.new_recorder();
        let c = ctx.clone();
        run_batch(returning, None, opts.concurrency, move |i| {
            let c = c.clone();
            async move {
                login(&c, i).await;
            }
        })
        .await;
    }
    let run = Run::begin(ctx, "login-wave").await;
    let slots: Arc<Mutex<Vec<Option<Arc<Shared>>>>> = Arc::new(Mutex::new(vec![None; total]));
    let nodes = ctx.target.nodes.len();
    let (rate, concurrency) = if opts.rate > 0.0 {
        (Some(opts.rate), total)
    } else {
        (None, opts.concurrency)
    };
    let (c, rec, out) = (ctx.clone(), run.rec.clone(), slots.clone());
    run_batch(total, rate, concurrency, move |i| {
        let (c, rec, out) = (c.clone(), rec.clone(), out.clone());
        async move {
            let started = Instant::now();
            let Some((id, token)) = login(&c, i).await else {
                rec.count("login_failed", 1);
                return;
            };
            let me = Arc::new(Shared {
                idx: i,
                id,
                token,
                home: AtomicUsize::new(i % nodes),
                up: Default::default(),
                friends: Mutex::default(),
            });
            out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(me.clone());
            let (tx, rx) = oneshot::channel();
            tokio::spawn(conn_loop(c.clone(), me, Some(tx)));
            if tokio::time::timeout(Duration::from_secs(60), rx)
                .await
                .is_ok()
            {
                rec.op("e2e_login", 200, started.elapsed());
            } else {
                rec.count("welcome_timeout", 1);
            }
        }
    })
    .await;
    let signed_in: Vec<Arc<Shared>> = slots
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .flatten()
        .cloned()
        .collect();
    *ctx.players.write().unwrap_or_else(|e| e.into_inner()) = signed_in.clone();
    let mut notes = vec![format!(
        "{} of {} players signed in{}",
        signed_in.len(),
        total,
        if returning > 0 {
            format!(", {returning} of them returning")
        } else {
            String::new()
        }
    )];
    if opts.rate > 0.0 {
        notes.push(format!("{} logins per second offered", opts.rate));
    }
    let mut report = run.finish(ctx, sampler, None, None, notes).await;
    report.players = signed_in.len();
    report
}

/// Each player gets about F friends: player i befriends the next F/2 players on a ring.
pub async fn friend_graph(ctx: &Arc<Ctx>, sampler: &Sampler) -> ScenarioReport {
    let run = Run::begin(ctx, "friend-graph").await;
    let all = players(ctx);
    let n = all.len();
    let half = ctx.opts.friends.div_ceil(2).min(n.saturating_sub(1) / 2);
    let edges: Vec<(usize, usize)> = (0..n)
        .flat_map(|i| (1..=half).map(move |k| (i, (i + k) % n)))
        .collect();
    let made: Arc<Mutex<Vec<(usize, usize)>>> = Arc::default();
    let planned = edges.len();
    let edges = Arc::new(edges);
    let (c, all2, made2) = (ctx.clone(), all.clone(), made.clone());
    run_batch(planned, None, ctx.opts.concurrency, move |e| {
        let (c, all, made, edges) = (c.clone(), all2.clone(), made2.clone(), edges.clone());
        async move {
            let (i, j) = edges[e];
            let (a, b) = (&all[i], &all[j]);
            let body = json!(FriendRequest {
                handle: c.handle(b.idx)
            });
            let reply = c
                .call_as(
                    a,
                    "friend_request",
                    Method::POST,
                    "/v1/friends/requests",
                    Some(body),
                )
                .await;
            let Some(answer) = reply.json::<FriendRequestAnswer>() else {
                return;
            };
            let friends = match answer.state {
                FriendState::Friends => true,
                FriendState::Requested => {
                    let path = format!("/v1/friends/requests/{}/accept", a.id);
                    c.call_as(b, "friend_accept", Method::POST, &path, None)
                        .await
                        .status
                        == 204
                }
            };
            if friends {
                made.lock().unwrap_or_else(|e| e.into_inner()).push((i, j));
            }
        }
    })
    .await;
    let made = made.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (i, j) in &made {
        all[*i]
            .friends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(*j);
        all[*j]
            .friends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(*i);
    }
    let notes = vec![format!(
        "{} of {planned} friendships made, about {} friends per player",
        made.len(),
        made.len() * 2 / n.max(1)
    )];
    run.finish(ctx, sampler, None, None, notes).await
}

/// Players change their activity; their friends are told. The change carries the time it was made,
/// so the friend's connection can tell how long the news took.
pub async fn presence_churn(ctx: &Arc<Ctx>, sampler: &Sampler) -> ScenarioReport {
    let opts = &ctx.opts;
    let run = Run::begin(ctx, "presence-churn").await;
    let all = players(ctx);
    let eligible: Arc<Vec<Arc<Shared>>> = Arc::new(
        all.iter()
            .filter(|p| {
                !p.friends
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
            })
            .cloned()
            .collect(),
    );
    let mut notes = vec![format!(
        "{} activity changes per second offered for {} s by {} players",
        opts.presence_rate,
        opts.presence_seconds,
        eligible.len()
    )];
    let changes = Arc::new(AtomicU64::new(0));
    if !eligible.is_empty() {
        let (c, rec, changes2, pool) = (ctx.clone(), run.rec.clone(), changes.clone(), eligible);
        open_loop(
            opts.presence_rate,
            opts.presence_seconds,
            opts.max_in_flight,
            run.rec.clone(),
            move |_| {
                let (c, rec, changes, pool) =
                    (c.clone(), rec.clone(), changes2.clone(), pool.clone());
                async move {
                    let me =
                        &pool[(rand::random::<f64>() * pool.len() as f64) as usize % pool.len()];
                    if !me.up.load(Ordering::Relaxed) {
                        rec.count("skipped_offline", 1);
                        return;
                    }
                    let body = json!({"activity": {
                        "title_id": "GAME00001",
                        "title": "Load Game",
                        "version": format!("t{}", c.micros()),
                    }});
                    let reply = c
                        .call_as(me, "presence_set", Method::PUT, "/v1/presence", Some(body))
                        .await;
                    if reply.status == 204 {
                        changes.fetch_add(1, Ordering::Relaxed);
                        let online = me
                            .friends
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .iter()
                            .filter(|f| {
                                c.players.read().unwrap_or_else(|e| e.into_inner())[**f]
                                    .up
                                    .load(Ordering::Relaxed)
                            })
                            .count();
                        rec.expected_deliveries
                            .fetch_add(online as u64, Ordering::Relaxed);
                    }
                }
            },
        )
        .await;
        // Let the last news arrive: until everything is here or nothing new came for 3 seconds.
        let mut last = (run.rec.delivered_count(), Instant::now());
        let settle_until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < settle_until {
            let now = run.rec.delivered_count();
            if now >= run.rec.expected_deliveries.load(Ordering::Relaxed) {
                break;
            }
            if now != last.0 {
                last = (now, Instant::now());
            } else if last.1.elapsed() > Duration::from_secs(3) {
                notes.push("news still missing 3 s after the last change".into());
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    } else {
        notes.push("no player has a friend; run with --friends".into());
    }
    let expected = run.rec.expected_deliveries.load(Ordering::Relaxed);
    let delivered = run.rec.delivered_count();
    let summary = run.rec.summarize();
    let delivery = DeliveryReport {
        changes: changes.load(Ordering::Relaxed),
        expected,
        delivered,
        delivered_share: if expected > 0 {
            delivered as f64 / expected as f64
        } else {
            0.0
        },
        latency: summary.delivery_ms,
    };
    run.finish(ctx, sampler, Some(delivery), None, notes).await
}

/// Players create sessions, their friends join, and everyone leaves after a while.
pub async fn session_churn(ctx: &Arc<Ctx>, sampler: &Sampler) -> ScenarioReport {
    let opts = &ctx.opts;
    let run = Run::begin(ctx, "sessions").await;
    let size = opts.session_size;
    let mut leaders: Vec<Arc<Shared>> = players(ctx)
        .into_iter()
        .filter(|p| p.friends.lock().unwrap_or_else(|e| e.into_inner()).len() >= size - 1)
        .collect();
    shuffle(&mut leaders);
    let mut notes = vec![format!(
        "{} sessions per second of {} members for {} s, each held {} s",
        opts.session_rate, size, opts.session_seconds, opts.session_hold
    )];
    if leaders.is_empty() {
        notes.push("no player has enough friends for a session; raise --friends".into());
    } else {
        let leaders = Arc::new(leaders);
        let (c, hold) = (ctx.clone(), Duration::from_secs_f64(opts.session_hold));
        open_loop(
            opts.session_rate,
            opts.session_seconds,
            opts.max_in_flight,
            run.rec.clone(),
            move |n| {
                let (c, leaders) = (c.clone(), leaders.clone());
                async move { one_session(&c, &leaders[n % leaders.len()], size, hold).await }
            },
        )
        .await;
    }
    run.finish(ctx, sampler, None, None, notes).await
}

async fn one_session(ctx: &Arc<Ctx>, leader: &Arc<Shared>, size: usize, hold: Duration) {
    let endpoint = |p: &Shared| format!("{:064x}", p.idx + 1);
    let create = json!(CreateSession {
        title_id: "GAME00001".into(),
        title: "Load Game".into(),
        version: "1.00".into(),
        build: "load".into(),
        max_members: size as u16,
        visibility: Visibility::Friends,
        properties: String::new(),
        endpoint: endpoint(leader),
        relay: None,
    });
    let reply = ctx
        .call_as(
            leader,
            "session_create",
            Method::POST,
            "/v1/sessions",
            Some(create),
        )
        .await;
    let Some(session) = reply.json::<Session>() else {
        return;
    };
    let members: Vec<Arc<Shared>> = {
        let ids: Vec<usize> = leader
            .friends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .take(size - 1)
            .copied()
            .collect();
        let all = ctx.players.read().unwrap_or_else(|e| e.into_inner());
        ids.into_iter().map(|i| all[i].clone()).collect()
    };
    let join_path = format!("/v1/sessions/{}/join", session.id);
    let joins = members.iter().map(|m| {
        let body = json!(JoinSession {
            version: "1.00".into(),
            build: "load".into(),
            endpoint: endpoint(m),
            relay: None,
            invitation: None,
        });
        ctx.call_as(m, "session_join", Method::POST, &join_path, Some(body))
    });
    futures_util::future::join_all(joins).await;
    tokio::time::sleep(hold).await;
    let leave_path = format!("/v1/sessions/{}/leave", session.id);
    let leaves = members
        .iter()
        .map(|m| ctx.call_as(m, "session_leave", Method::POST, &leave_path, None));
    futures_util::future::join_all(leaves).await;
    ctx.call_as(leader, "session_leave", Method::POST, &leave_path, None)
        .await;
}

fn name_of(kind: StormKind) -> &'static str {
    match kind {
        StormKind::Drop => "drop",
        StormKind::StopNode => "stop-node",
        StormKind::KillNode => "kill-node",
    }
}

/// Breaks the connections, or stops a node, and times how long players take to come back.
pub async fn reconnect_storm(ctx: &Arc<Ctx>, sampler: &Sampler) -> ScenarioReport {
    let opts: &Options = &ctx.opts;
    let run = Run::begin(ctx, "reconnect-storm").await;
    let all = players(ctx);
    let max_idx = all.iter().map(|p| p.idx).max().unwrap_or(0);
    let mut expected = vec![false; max_idx + 1];
    for p in &all {
        let hit = p.up.load(Ordering::Relaxed)
            && match opts.storm {
                StormKind::Drop => true,
                StormKind::StopNode | StormKind::KillNode => p.home.load(Ordering::Relaxed) == 0,
            };
        expected[p.idx] = hit;
    }
    let track = Arc::new(StormTrack::new(expected));
    *ctx.storm.write().unwrap_or_else(|e| e.into_inner()) = Some(track.clone());
    let hit = track.expected_count();
    match opts.storm {
        StormKind::Drop => ctx.drop_all(),
        StormKind::StopNode => {
            let node = ctx.target.nodes[0].clone();
            // A load balancer stops sending players to a node whose readiness fails, which it does
            // as soon as the node starts to drain.
            node.set_alive(false);
            tokio::spawn(async move { node.stop().await });
        }
        StormKind::KillNode => ctx.target.nodes[0].kill(),
    }
    let timeout = Duration::from_secs_f64(opts.storm_timeout);
    let mut min_connections = ctx.connected.load(Ordering::Relaxed);
    while track.t0.elapsed() < timeout && track.done_count() < hit {
        min_connections = min_connections.min(ctx.connected.load(Ordering::Relaxed));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let back = track.done_count();
    let mut done: Vec<f64> = track
        .done
        .iter()
        .map(|d| d.load(Ordering::Relaxed))
        .filter(|d| *d != 0)
        .map(|d| d as f64 / 1e6)
        .collect();
    done.sort_by(f64::total_cmp);
    let time_to = |share: f64| -> Option<f64> {
        let need = ((share * hit as f64).ceil() as usize).max(1);
        (hit > 0 && done.len() >= need).then(|| done[need - 1])
    };
    let seconds_to: BTreeMap<String, Option<f64>> = [0.5, 0.9, 0.95, 0.99, 1.0]
        .iter()
        .map(|q| (format!("{:.0}%", q * 100.0), time_to(*q)))
        .collect();
    let storm = StormReport {
        kind: name_of(opts.storm).into(),
        reconnect: match opts.reconnect {
            Reconnect::Spec => "spec".into(),
            Reconnect::Immediate => "immediate".into(),
        },
        hit,
        back,
        back_share: if hit > 0 {
            back as f64 / hit as f64
        } else {
            0.0
        },
        seconds_to,
        target_share: opts.storm_target,
        seconds_to_target: time_to(opts.storm_target),
        min_connections,
    };
    let mut notes = Vec::new();
    if back < hit {
        notes.push(format!(
            "{} players were not back after {} s",
            hit - back,
            opts.storm_timeout
        ));
    }
    // Let the cloud settle before the next measurement: the friend lists fetched after the welcome.
    tokio::time::sleep(Duration::from_millis(500)).await;
    *ctx.storm.write().unwrap_or_else(|e| e.into_inner()) = None;
    run.finish(ctx, sampler, None, Some(storm), notes).await
}
