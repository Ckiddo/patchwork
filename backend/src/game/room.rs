use crate::{
    persistence::{Database, StoreError as E, friends::*},
    sessions::{Publish, SessionRegistry},
};
use actix::prelude::*;
use std::collections::VecDeque;
use tokio::sync::oneshot;
use uuid::Uuid;
pub struct Resolved {
    pub original: Outcome,
    pub current: RoomView,
}
struct Job {
    mutation: RoomMutation,
    reply: oneshot::Sender<Result<Resolved, E>>,
}
pub struct Room {
    id: Uuid,
    database: Database,
    registry: Addr<SessionRegistry>,
    queue: VecDeque<Job>,
    running: bool,
    config: super::Configure,
    lobby: Addr<super::LobbyManager>,
    last_tick: Option<std::time::Instant>,
    published_version: Option<i64>,
}
impl Room {
    pub fn new(id: Uuid, config: super::Configure, lobby: Addr<super::LobbyManager>) -> Self {
        Self {
            id,
            database: config.database.clone(),
            registry: config.registry.clone(),
            queue: VecDeque::new(),
            running: false,
            config,
            lobby,
            last_tick: None,
            published_version: None,
        }
    }
    fn next(&mut self, ctx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(job) = self.queue.pop_front() else {
            return;
        };
        self.running = true;
        let db = self.database.clone();
        ctx.spawn(
            async move {
                let result = if job.mutation.permit.live() {
                    db.friend_mutate(job.mutation.clone())
                        .await
                        .map(|r| r.into_inner())
                } else {
                    Err(E::Permission)
                };
                (job, result)
            }
            .into_actor(self)
            .map(|(job, result), actor, ctx| {
                if result == Err(E::CommitUnknown) {
                    actor.recover(job, ctx);
                } else {
                    actor.finish(job, result, ctx);
                }
            }),
        );
    }
    fn recover(&mut self, job: Job, ctx: &mut Context<Self>) {
        let db = self.database.clone();
        ctx.spawn(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let result = db
                    .friend_recover(job.mutation.clone())
                    .await
                    .map(|r| r.into_inner());
                (job, result)
            }
            .into_actor(self)
            .map(|(job, result), actor, ctx| match result {
                Ok(Some(outcome)) => actor.finish(job, Ok(outcome), ctx),
                Ok(None) => actor.finish(job, Err(E::Unavailable), ctx),
                Err(E::RequestIdConflict) => actor.finish(job, Err(E::RequestIdConflict), ctx),
                Err(_) => actor.recover(job, ctx),
            }),
        );
    }
    fn finish(&mut self, job: Job, result: Result<Outcome, E>, ctx: &mut Context<Self>) {
        // Never broadcast an old receipt over a newer snapshot.
        let db = self.database.clone();
        let id = self.id;
        let registry = self.registry.clone();
        ctx.spawn(
            async move {
                let mut latest = if result.is_ok() {
                    db.friend_current(id).await.ok()
                } else {
                    None
                };
                if let (Ok(original), Some(current)) = (&result, &mut latest) {
                    current
                        .recipients
                        .extend(original.recipients.iter().copied());
                    current.recipients.sort();
                    current.recipients.dedup();
                    let _ = registry.send(Publish(current.clone())).await;
                }
                (job, result, latest)
            }
            .into_actor(self)
            .map(|(job, result, latest), actor, ctx| {
                let result = match (result, latest) {
                    (Ok(outcome), Some(latest)) => {
                        actor.published_version = Some(latest.room.version);
                        Ok(Resolved {
                            original: outcome,
                            current: latest.room,
                        })
                    }
                    (Ok(outcome), None) => {
                        // COMMIT is known but the index/snapshot cannot yet be synchronized.
                        // Keep both the room queue and the user's reservation held.
                        ctx.run_later(std::time::Duration::from_secs(1), move |actor, ctx| {
                            actor.finish(job, Ok(outcome), ctx)
                        });
                        return;
                    }
                    (Err(e), _) => Err(e),
                };
                let _ = job.reply.send(result);
                actor.running = false;
                actor.next(ctx);
            }),
        );
    }
}
impl Actor for Room {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(32);
        // A timer is coalesced, never appended to a growing mailbox or queue.
        ctx.run_interval(std::time::Duration::from_secs(1), |actor, ctx| {
            if !actor.running && actor.queue.is_empty() {
                actor.observe(ctx);
            }
        });
    }
}
impl Room {
    fn observe(&mut self, ctx: &mut Context<Self>) {
        self.running = true;
        let registry = self.registry.clone();
        let db = self.database.clone();
        let now = std::time::Instant::now();
        let elapsed_ms = self
            .last_tick
            .map(|t| now.duration_since(t).as_millis())
            .filter(|ms| *ms <= 3000)
            .unwrap_or(0) as i64;
        let id = self.id;
        let epoch = self.config.epoch;
        let policy = self.config.recovery.clone();
        let lobby = self.lobby.clone();
        let published = self.published_version;
        ctx.spawn(
            async move {
                let presence = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    registry.send(crate::sessions::Snapshot),
                )
                .await;
                let Ok(Ok(presence)) = presence else {
                    return (now, Err(E::Unavailable));
                };
                let observation = crate::persistence::recovery::Observation {
                    room: id,
                    epoch,
                    tick: Uuid::new_v4(),
                    elapsed_ms,
                    started: now,
                    presence,
                    policy,
                };
                let mut result = db.observe_room(observation.clone()).await;
                // The same durable tick ID resolves uncertain COMMIT without a second debit.
                while matches!(result, Err(E::CommitUnknown)) {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    result = db.observe_room(observation.clone()).await;
                }
                let result = result.map(|v| v.into_inner());
                if let Ok(outcome) = &result
                    && published != Some(outcome.room.version)
                {
                    let _ = lobby.send(super::IndexChanged(outcome.room.clone())).await;
                    let _ = registry.send(Publish(outcome.clone())).await;
                }
                (now, result)
            }
            .into_actor(self)
            .map(|(started, result), actor, ctx| {
                actor.last_tick = if result.is_ok() { Some(started) } else { None };
                if let Ok(outcome) = result {
                    if actor.published_version != Some(outcome.room.version) {
                        actor.published_version = Some(outcome.room.version);
                    }
                    if outcome.room.members.is_empty() {
                        ctx.stop();
                    }
                } else if result == Err(E::Permission) {
                    ctx.stop();
                }
                actor.running = false;
                actor.next(ctx);
            }),
        );
    }
}
#[derive(Message)]
#[rtype(result = "Result<Resolved,E>")]
pub struct Execute(pub RoomMutation);
impl Handler<Execute> for Room {
    type Result = ResponseFuture<Result<Resolved, E>>;
    fn handle(&mut self, m: Execute, ctx: &mut Context<Self>) -> Self::Result {
        if m.0.room != self.id || self.queue.len() >= 32 {
            return Box::pin(async { Err(E::Unavailable) });
        }
        let (reply, receive) = oneshot::channel();
        self.queue.push_back(Job {
            mutation: m.0,
            reply,
        });
        self.next(ctx);
        Box::pin(async move { receive.await.unwrap_or(Err(E::Unavailable)) })
    }
}
