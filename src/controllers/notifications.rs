//! The bell: a list of someone's notifications, its unread badge, and a live
//! stream that tells open pages to refresh the badge.
use std::{collections::HashMap, convert::Infallible, time::Duration};

use axum::{
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::{DateTime, FixedOffset, Utc};
use futures_util::stream::{self, Stream, StreamExt};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{
    data::notify_hub::{NotifyEvent, NotifyHub},
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        conversations, memberships, messages,
        notifications::{self, kind, Notice},
        project_owners, projects, task_assignees, task_notes, tasks, users,
    },
    views::{layout::avatar_color, time},
};

const SHOWN: u64 = 50;
/// Mention headlines start with this; the list rewrites it for your own tags.
const MENTIONED_YOU: &str = "mentioned you";

#[derive(Debug, Serialize)]
struct NotificationView {
    id: i64,
    kind: String,
    /// The avatar's name; `who` is how the line reads, "You" for your own tags.
    actor: String,
    who: String,
    color: &'static str,
    headline: String,
    excerpt: String,
    at: String,
    unread: bool,
}

/// "just now", "5m", "3h", "2d", then the date in the organisation's zone.
fn ago(at: DateTime<FixedOffset>, now: DateTime<Utc>, tz: chrono_tz::Tz) -> String {
    let minutes = (now - at.with_timezone(&Utc)).num_minutes();
    match minutes {
        ..1 => "just now".to_string(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        1440..10080 => format!("{}d", minutes / 1440),
        _ => time::local(at, tz).format("%-d %b").to_string(),
    }
}

/// Saves a notice for each recipient and tells their open pages. Skips the
/// actor. Returns who was notified.
///
/// # Errors
/// On database errors.
pub async fn send(
    ctx: &AppContext,
    org_id: i64,
    actor_id: i64,
    recipients: &[i64],
    notice: &Notice,
) -> Result<Vec<i64>> {
    let sent = notifications::Model::notify(&ctx.db, org_id, actor_id, recipients, notice).await?;
    refresh_badges(ctx, org_id, &sent);
    Ok(sent)
}

/// Tells these people's open pages to reload the bell badge.
pub fn refresh_badges(ctx: &AppContext, org_id: i64, user_ids: &[i64]) {
    if let Some(hub) = ctx.shared_store.get::<NotifyHub>() {
        hub.publish(org_id, user_ids);
    }
}

/// Cuts text to a short quote for a notification.
#[must_use]
pub fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(119).collect::<String>())
    } else {
        flat
    }
}

/// A task before a change: who was on it and its status. `None` for a new task.
pub struct TaskBefore {
    pub assignees: Vec<i64>,
    pub status: String,
}

/// After a task is created or changed: tells people newly assigned, people
/// taken off, and the project's owners about a new task or one that just
/// became blocked.
///
/// # Errors
/// On database errors.
pub async fn task_saved(
    ctx: &AppContext,
    member: &CurrentMember,
    before: Option<TaskBefore>,
    task: &tasks::Model,
) -> Result<()> {
    let org_id = member.org.id;
    let actor = member.user.id;
    let link = format!("/tasks?open={}", task.id);
    let now = task_assignees::Model::for_task(&ctx.db, org_id, task.id).await?;
    let added: Vec<i64> = match &before {
        Some(b) => now
            .iter()
            .filter(|id| !b.assignees.contains(id))
            .copied()
            .collect(),
        None => now.clone(),
    };
    let mut told = send(
        ctx,
        org_id,
        actor,
        &added,
        &Notice {
            kind: kind::ASSIGNED,
            body: format!("assigned you to “{}”", task.title),
            link: link.clone(),
            message_id: None,
        },
    )
    .await?;
    // People taken off, while on the team. A close by a project owner skips
    // tasks with others on them, so taking someone off first is never silent.
    let removed: Vec<i64> = before.as_ref().map_or_else(Vec::new, |b| {
        b.assignees
            .iter()
            .filter(|id| !now.contains(id))
            .copied()
            .collect()
    });
    if !removed.is_empty() {
        let on_team: Vec<i64> =
            memberships::Model::team_among(&ctx.db, org_id, removed.iter().copied())
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
        let removed: Vec<i64> = removed
            .into_iter()
            .filter(|id| on_team.contains(id))
            .collect();
        told.extend(
            send(
                ctx,
                org_id,
                actor,
                &removed,
                &Notice {
                    kind: kind::ASSIGNED,
                    body: format!("took you off “{}”", task.title),
                    link: link.clone(),
                    message_id: None,
                },
            )
            .await?,
        );
    }

    // Chores have no project, so no owners to tell.
    let Some(project_id) = task.project_id else {
        return Ok(());
    };
    let project = projects::Model::find_in_org(&ctx.db, org_id, project_id).await?;
    let event = match &before {
        None => Some(format!("added “{}” to {}", task.title, project.name)),
        Some(b) if b.status != "blocked" && task.status == "blocked" => Some(format!(
            "marked “{}” as blocked in {}",
            task.title, project.name
        )),
        Some(_) => None,
    };
    let Some(body) = event else {
        return Ok(());
    };
    let owners: Vec<i64> = owners_on_team(ctx, org_id, project.id)
        .await?
        .into_iter()
        .filter(|id| !told.contains(id))
        .collect();
    told.extend(
        send(
            ctx,
            org_id,
            actor,
            &owners,
            &Notice {
                kind: kind::PROJECT,
                body,
                link,
                message_id: None,
            },
        )
        .await?,
    );
    Ok(())
}

/// A project's owners who are approved members now, in the order added. A
/// removed member's row can be left behind; they are never told.
async fn owners_on_team(ctx: &AppContext, org_id: i64, project_id: i64) -> Result<Vec<i64>> {
    let owners = project_owners::Model::for_project(&ctx.db, org_id, project_id).await?;
    let team: Vec<i64> = memberships::Model::team_among(&ctx.db, org_id, owners.iter().copied())
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    Ok(owners.into_iter().filter(|id| team.contains(id)).collect())
}

/// After a note is posted: tells everyone mentioned, then the whole team for
/// @everyone, then the task's other assignees that there is a new note. Each
/// person gets one notice; a name wins over @everyone.
///
/// # Errors
/// On database errors.
pub async fn note_added(
    ctx: &AppContext,
    member: &CurrentMember,
    task: &tasks::Model,
    note: &task_notes::Model,
) -> Result<()> {
    let org_id = member.org.id;
    let team = memberships::Model::team(&ctx.db, org_id).await?;
    let link = format!("/tasks?open={}", task.id);
    let quote = excerpt(&note.body);
    let mut told = send(
        ctx,
        org_id,
        member.user.id,
        &notifications::mentioned_ids(&note.body, &team),
        &Notice {
            kind: kind::MENTION,
            body: format!("{MENTIONED_YOU} on “{}”\n{quote}", task.title),
            link: link.clone(),
            message_id: None,
        },
    )
    .await?;
    // Only a person in their own organisation, in the browser, can tell everyone.
    // Claude's bearer tokens and a super admin acting elsewhere cannot.
    let everyone_allowed = member.via_session && !member.acting;
    if everyone_allowed && notifications::mentions_everyone(&note.body) {
        let rest: Vec<i64> = team
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| *id != member.user.id && !told.contains(id))
            .collect();
        told.extend(
            send(
                ctx,
                org_id,
                member.user.id,
                &rest,
                &Notice {
                    kind: kind::MENTION,
                    body: format!("mentioned everyone on “{}”\n{quote}", task.title),
                    link: link.clone(),
                    message_id: None,
                },
            )
            .await?,
        );
    }
    let assignees: Vec<i64> = task_assignees::Model::for_task(&ctx.db, org_id, task.id)
        .await?
        .into_iter()
        .filter(|id| !told.contains(id))
        .collect();
    send(
        ctx,
        org_id,
        member.user.id,
        &assignees,
        &Notice {
            kind: kind::NOTE,
            body: format!("commented on “{}”\n{quote}", task.title),
            link,
            message_id: None,
        },
    )
    .await?;
    Ok(())
}

/// After a project is saved: tells each owner added (`owners` not in
/// `before`) who is on the team, in one go, and logs the change. People
/// taken off hear nothing.
///
/// # Errors
/// On database errors.
pub async fn project_saved(
    ctx: &AppContext,
    member: &CurrentMember,
    before: &[i64],
    project: &projects::Model,
    owners: &[i64],
) -> Result<()> {
    let added: Vec<i64> = owners
        .iter()
        .filter(|id| !before.contains(id))
        .copied()
        .collect();
    let removed: Vec<i64> = before
        .iter()
        .filter(|id| !owners.contains(id))
        .copied()
        .collect();
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    tracing::info!(
        org_id = member.org.id,
        project_id = project.id,
        user_id = member.user.id,
        acting = member.acting,
        ?added,
        ?removed,
        "changed project owners"
    );
    let on_team: Vec<i64> =
        memberships::Model::team_among(&ctx.db, member.org.id, added.iter().copied())
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    let told: Vec<i64> = added
        .into_iter()
        .filter(|id| on_team.contains(id))
        .collect();
    send(
        ctx,
        member.org.id,
        member.user.id,
        &told,
        &Notice {
            kind: kind::OWNER,
            body: format!("made you owner of {}", project.name),
            link: format!("/roadmap?lane={}", project.lane),
            message_id: None,
        },
    )
    .await?;
    Ok(())
}

/// After a project is marked done: one notice per person, not one per task.
/// Each assignee of a task the close changed hears how many of theirs it
/// closed. Each assignee of a task it left open (`left_open`, from a close by
/// a project owner who isn't a manager) hears how many of theirs it left
/// open. The project's owners, if not told already, hear the totals. Only
/// people on the team. Never `task_saved`, which would send one per task.
///
/// # Errors
/// On database errors.
pub async fn project_closed(
    ctx: &AppContext,
    member: &CurrentMember,
    project: &projects::Model,
    closed: &[tasks::Model],
    left_open: &[tasks::Model],
) -> Result<()> {
    let change = Change {
        kind: kind::CLOSED,
        verb: "closed",
        headline: format!("marked {} done", project.name),
        link: "/roadmap?lane=done".to_string(),
    };
    tell_per_person(ctx, member, project, closed, left_open, change).await
}

/// After a done project is reopened: the same shape as [`project_closed`],
/// counting the tasks that went back to their earlier status.
///
/// # Errors
/// On database errors.
pub async fn project_reopened(
    ctx: &AppContext,
    member: &CurrentMember,
    project: &projects::Model,
    reopened: &[tasks::Model],
) -> Result<()> {
    let change = Change {
        kind: kind::REOPENED,
        verb: "reopened",
        headline: format!("reopened {}", project.name),
        link: format!("/roadmap?lane={}", project.lane),
    };
    tell_per_person(ctx, member, project, reopened, &[], change).await
}

/// What a close or reopen did, for [`tell_per_person`].
struct Change {
    kind: &'static str,
    /// "closed" or "reopened", as in "2 of your tasks closed".
    verb: &'static str,
    headline: String,
    link: String,
}

/// How many of these tasks each person on the team is assigned, in the
/// order first met.
fn per_person(
    list: &[tasks::Model],
    assignees: &HashMap<i64, Vec<i64>>,
    team: &[i64],
) -> Vec<(i64, usize)> {
    let mut counts: Vec<(i64, usize)> = Vec::new();
    for user in list
        .iter()
        .filter_map(|t| assignees.get(&t.id))
        .flatten()
        .filter(|id| team.contains(id))
    {
        match counts.iter_mut().find(|(id, _)| id == user) {
            Some((_, n)) => *n += 1,
            None => counts.push((*user, 1)),
        }
    }
    counts
}

/// One notice per assignee of the changed tasks, saying how many of theirs
/// changed, then one per assignee of the tasks `left_open` not told yet,
/// then one to each of the project's owners with the totals if not told
/// yet.
async fn tell_per_person(
    ctx: &AppContext,
    member: &CurrentMember,
    project: &projects::Model,
    changed: &[tasks::Model],
    left_open: &[tasks::Model],
    change: Change,
) -> Result<()> {
    let org_id = member.org.id;
    let team: Vec<i64> = memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let assignees = task_assignees::Model::by_task(&ctx.db, org_id).await?;
    let Change {
        kind,
        verb,
        headline,
        link,
    } = change;
    let mut told = Vec::new();
    for (user, n) in per_person(changed, &assignees, &team) {
        let notice = Notice {
            kind,
            body: format!("{headline}\n{n} of your tasks {verb}"),
            link: link.clone(),
            message_id: None,
        };
        told.extend(send(ctx, org_id, member.user.id, &[user], &notice).await?);
    }
    for (user, n) in per_person(left_open, &assignees, &team) {
        if told.contains(&user) {
            continue;
        }
        let notice = Notice {
            kind,
            body: format!("{headline}\n{n} of your tasks left open"),
            link: link.clone(),
            message_id: None,
        };
        told.extend(send(ctx, org_id, member.user.id, &[user], &notice).await?);
    }
    let owners: Vec<i64> = project_owners::Model::for_project(&ctx.db, org_id, project.id)
        .await?
        .into_iter()
        .filter(|id| team.contains(id) && !told.contains(id))
        .collect();
    if !owners.is_empty() {
        let body = match (changed.len(), left_open.len()) {
            (0, 0) => headline,
            (1, 0) => format!("{headline}\n1 task {verb}"),
            (n, 0) => format!("{headline}\n{n} tasks {verb}"),
            (0, 1) => format!("{headline}\n1 task left open"),
            (0, k) => format!("{headline}\n{k} tasks left open"),
            (1, k) => format!("{headline}\n1 task {verb}, {k} left open"),
            (n, k) => format!("{headline}\n{n} tasks {verb}, {k} left open"),
        };
        let notice = Notice {
            kind,
            body,
            link,
            message_id: None,
        };
        send(ctx, org_id, member.user.id, &owners, &notice).await?;
    }
    Ok(())
}

/// After a chat message is saved: tells mentioned people who can see the
/// conversation. Mentioning someone outside a group or DM does nothing.
/// `@everyone` tells the conversation's other members, once each; people also
/// named get the "mentioned you" notice instead.
///
/// # Errors
/// On database errors.
pub async fn message_sent(
    ctx: &AppContext,
    org_id: i64,
    conversation: &conversations::Model,
    message: &messages::Model,
) -> Result<()> {
    let team = memberships::Model::team(&ctx.db, org_id).await?;
    let mentioned = notifications::mentioned_ids(&message.body, &team);
    let everyone = notifications::mentions_everyone(&message.body);
    if mentioned.is_empty() && !everyone {
        return Ok(());
    }
    let members = conversation.member_ids(&ctx.db).await?;
    let visible: Vec<i64> = mentioned
        .into_iter()
        .filter(|id| members.contains(id))
        .collect();
    let place = match (conversation.kind.as_str(), &conversation.name) {
        (conversations::kind::DM, _) | (_, None) => "a message".to_string(),
        (conversations::kind::CHANNEL, Some(name)) => format!("#{name}"),
        (_, Some(name)) => name.clone(),
    };
    let quote = excerpt(&message.body);
    let link = format!("/chat/{}", conversation.id);
    // Saved under a lock on the message: one deleted meanwhile tells nobody.
    let txn = ctx.db.begin().await?;
    if messages::Model::lock_live(&txn, org_id, message.id)
        .await?
        .is_none()
    {
        // Already deleted. Dropping `txn` rolls it back; nothing was saved.
        return Ok(());
    }
    let mut told = notifications::Model::notify(
        &txn,
        org_id,
        message.user_id,
        &visible,
        &Notice {
            kind: kind::MENTION,
            body: format!("{MENTIONED_YOU} in {place}\n{quote}"),
            link: link.clone(),
            message_id: Some(message.id),
        },
    )
    .await?;
    if everyone {
        // Mentions notify their own author, so leave the sender out here.
        let rest: Vec<i64> = team
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| members.contains(id) && *id != message.user_id && !told.contains(id))
            .collect();
        told.extend(
            notifications::Model::notify(
                &txn,
                org_id,
                message.user_id,
                &rest,
                &Notice {
                    kind: kind::MENTION,
                    body: format!("mentioned everyone in {place}\n{quote}"),
                    link,
                    message_id: Some(message.id),
                },
            )
            .await?,
        );
    }
    txn.commit().await?;
    refresh_badges(ctx, org_id, &told);
    Ok(())
}

/// After a thread reply is saved: tells mentioned people who can see the
/// conversation ("mentioned you in a thread"), everyone for `@everyone`, then
/// the parent's author ("replied to your message") and earlier repliers
/// ("replied in a thread"). Each person is told once, and the first of those
/// wins. The reply's author gets no reply or `@everyone` notice, but tagging
/// yourself is still a reminder, as in the channel.
///
/// # Errors
/// On database errors.
pub async fn reply_sent(
    ctx: &AppContext,
    org_id: i64,
    conversation: &conversations::Model,
    parent: &messages::Model,
    reply: &messages::Model,
) -> Result<()> {
    let team = memberships::Model::team(&ctx.db, org_id).await?;
    let members = conversation.member_ids(&ctx.db).await?;
    // On the team and in the conversation, so removed people are never told.
    let can_see = |id: &i64| members.contains(id) && team.iter().any(|(t, _)| t == id);
    let author = reply.user_id;
    let place = match (conversation.kind.as_str(), &conversation.name) {
        (conversations::kind::DM, _) | (_, None) => String::new(),
        (conversations::kind::CHANNEL, Some(name)) => format!(" in #{name}"),
        (_, Some(name)) => format!(" in {name}"),
    };
    let quote = excerpt(&reply.body);
    let link = format!("/chat/{}/threads/{}", conversation.id, parent.id);
    let notice = |kind, headline: String| Notice {
        kind,
        body: format!("{headline}{place}\n{quote}"),
        link: link.clone(),
        message_id: Some(reply.id),
    };
    let mentioned: Vec<i64> = notifications::mentioned_ids(&reply.body, &team)
        .into_iter()
        .filter(can_see)
        .collect();
    let repliers: Vec<i64> = messages::Model::thread_stats(&ctx.db, org_id, &[parent.id])
        .await?
        .remove(&parent.id)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.user_id)
        .collect();

    // Saved under a lock on the reply: one deleted meanwhile, or whose
    // parent was, tells nobody.
    let txn = ctx.db.begin().await?;
    if messages::Model::lock_live(&txn, org_id, reply.id)
        .await?
        .is_none()
    {
        // Already deleted. Dropping `txn` rolls it back; nothing was saved.
        return Ok(());
    }
    let mut told = notifications::Model::notify(
        &txn,
        org_id,
        author,
        &mentioned,
        &notice(kind::MENTION, format!("{MENTIONED_YOU} in a thread")),
    )
    .await?;
    let not_yet = |id: &i64, told: &[i64]| *id != author && can_see(id) && !told.contains(id);

    if notifications::mentions_everyone(&reply.body) {
        let rest: Vec<i64> = members
            .iter()
            .copied()
            .filter(|id| not_yet(id, &told))
            .collect();
        told.extend(
            notifications::Model::notify(
                &txn,
                org_id,
                author,
                &rest,
                &notice(kind::MENTION, "mentioned everyone in a thread".to_string()),
            )
            .await?,
        );
    }
    if not_yet(&parent.user_id, &told) {
        told.extend(
            notifications::Model::notify(
                &txn,
                org_id,
                author,
                &[parent.user_id],
                &notice(kind::REPLY, "replied to your message".to_string()),
            )
            .await?,
        );
    }
    let repliers: Vec<i64> = repliers
        .into_iter()
        .filter(|id| not_yet(id, &told))
        .collect();
    told.extend(
        notifications::Model::notify(
            &txn,
            org_id,
            author,
            &repliers,
            &notice(kind::REPLY, "replied in a thread".to_string()),
        )
        .await?,
    );
    txn.commit().await?;
    refresh_badges(ctx, org_id, &told);
    Ok(())
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let names: HashMap<i64, String> = memberships::Model::team(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .collect();
    let now = Utc::now();
    let items: Vec<NotificationView> =
        notifications::Model::recent(&ctx.db, member.org.id, member.user.id, SHOWN)
            .await?
            .into_iter()
            .map(|n| {
                // The app's own notices have no person behind them.
                let from_app = n.kind == kind::CONNECTOR;
                let actor = if from_app {
                    "Collab Tool".to_string()
                } else {
                    n.actor_id
                        .and_then(|id| names.get(&id).cloned())
                        .unwrap_or_else(|| "Someone".to_string())
                };
                let (mut headline, excerpt) = n
                    .body
                    .split_once('\n')
                    .map_or((n.body.clone(), String::new()), |(h, e)| {
                        (h.to_string(), e.to_string())
                    });
                let mine = n.actor_id == Some(member.user.id);
                if mine {
                    headline = headline.replacen(MENTIONED_YOU, "mentioned yourself", 1);
                }
                NotificationView {
                    id: n.id,
                    kind: n.kind,
                    color: avatar_color(&actor),
                    who: if mine {
                        "You".to_string()
                    } else if from_app {
                        String::new()
                    } else {
                        actor.clone()
                    },
                    actor,
                    headline,
                    excerpt,
                    at: ago(n.created_at, now, member.tz()),
                    unread: n.read_at.is_none(),
                }
            })
            .collect();
    let unread = items.iter().any(|n| n.unread);
    format::render().view(
        &v,
        "notifications/index.html",
        member.page(
            "notifications",
            serde_json::json!({ "items": items, "unread": unread }),
        ),
    )
}

/// Marks one notification read and goes to what it is about.
#[debug_handler]
async fn open(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    let n = notifications::Model::mark_read(&ctx.db, member.org.id, member.user.id, id).await?;
    // Links are written by the app as paths inside its org; `url` refuses
    // anything else.
    Ok(redirect_response(&headers, &member.url(&n.link)))
}

#[debug_handler]
async fn read_all(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Response> {
    notifications::Model::mark_all_read(&ctx.db, member.org.id, member.user.id).await?;
    Ok(redirect_response(&headers, &member.url("/notifications")))
}

#[derive(Debug, Default, Deserialize)]
struct UnreadQuery {
    style: Option<String>,
}

/// The unread count as a badge (or nothing). `style=dot` is the small one on
/// the phone top bar.
#[debug_handler]
async fn unread(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Query(query): Query<UnreadQuery>,
) -> Result<Response> {
    let total = notifications::Model::unread_count(&ctx.db, member.org.id, member.user.id).await?;
    if total == 0 {
        return format::html("");
    }
    let shown = if total > 99 {
        "99+".to_string()
    } else {
        total.to_string()
    };
    let class = if query.style.as_deref() == Some("dot") {
        "bell-badge"
    } else {
        "badge"
    };
    format::html(&format!(
        r#"<span class="{class}" aria-label="{total} unread notifications">{shown}</span>"#
    ))
}

/// Server-sent events for one organisation: one `notify` event whenever
/// this person gets a new notification there. The page then reloads its
/// badges.
#[debug_handler]
async fn stream(
    member: CurrentMember,
    State(ctx): State<AppContext>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let hub = ctx
        .shared_store
        .get::<NotifyHub>()
        .ok_or_else(|| Error::string("notify hub missing"))?;
    // Subscribe first, then check again: a close published between the
    // extractor's checks and the subscribe would otherwise be missed.
    let rx = hub.subscribe();
    let events = if still_allowed(&ctx, &member).await {
        events_for(rx, member.org.id, member.user.id).left_stream()
    } else {
        stream::empty().right_stream()
    };
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(25))))
}

/// Whether the stream's person still has the session and the membership
/// the request was let in with, as for a chat socket. A super admin acting
/// in an org has no membership there, so only the session is checked.
async fn still_allowed(ctx: &AppContext, member: &CurrentMember) -> bool {
    let current = users::Model::find_by_id(&ctx.db, member.user.id)
        .await
        .is_ok_and(|u| u.session_version == member.user.session_version);
    current
        && (member.acting
            || memberships::Model::active_in(&ctx.db, member.org.id, member.user.id)
                .await
                .is_ok_and(|m| m.is_some()))
}

/// The events for `me` in `org_id`. Ends on a
/// close that matches (see `CloseScope`), so a signed-out or removed browser
/// stops listening; its reconnect then goes through the same checks.
fn events_for(
    rx: broadcast::Receiver<NotifyEvent>,
    org_id: i64,
    me: i64,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    let notify = || Ok(Event::default().event("notify").data("1"));
    stream::unfold(rx, move |mut rx| {
        async move {
            loop {
                match rx.recv().await {
                    Ok(NotifyEvent::Changed {
                        org_id: org,
                        user_id,
                    }) if org == org_id && user_id == me => return Some((notify(), rx)),
                    Ok(NotifyEvent::Close(scope)) if scope.closes(org_id, me) => return None,
                    Ok(_) => {}
                    // Missed events may include a close: end the stream. The
                    // browser reconnects through the same checks.
                    Err(RecvError::Lagged(_) | RecvError::Closed) => return None,
                }
            }
        }
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/notifications", get(index))
        .add("/notifications/unread", get(unread))
        .add("/notifications/stream", get(stream))
        .add("/notifications/read", post(read_all))
        .add("/notifications/{id}/open", post(open))
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::data::live::CloseScope;

    /// Whether the stream has an event ready now, without waiting.
    async fn has_next(
        events: &mut (impl Stream<Item = std::result::Result<Event, Infallible>> + Unpin),
    ) -> Option<bool> {
        tokio::time::timeout(Duration::from_millis(50), events.next())
            .await
            .ok()
            .map(|e| e.is_some())
    }

    #[tokio::test]
    async fn the_stream_forwards_only_its_own_orgs_notices() {
        let hub = NotifyHub::new();
        let mut events = Box::pin(events_for(hub.subscribe(), 1, 7));
        hub.publish(2, &[7]);
        hub.publish(1, &[8]);
        assert_eq!(has_next(&mut events).await, None, "nothing for org 1");
        hub.publish(1, &[7]);
        assert_eq!(has_next(&mut events).await, Some(true));
    }

    #[tokio::test]
    async fn the_stream_ends_on_removal_from_its_org_only() {
        let hub = NotifyHub::new();
        let mut events = Box::pin(events_for(hub.subscribe(), 1, 7));
        hub.close(CloseScope::Org {
            org_id: 2,
            user_id: 7,
        });
        hub.close(CloseScope::Org {
            org_id: 1,
            user_id: 8,
        });
        hub.publish(1, &[7]);
        assert_eq!(has_next(&mut events).await, Some(true), "still open");
        hub.close(CloseScope::Org {
            org_id: 1,
            user_id: 7,
        });
        assert_eq!(has_next(&mut events).await, Some(false), "closed");
    }

    #[tokio::test]
    async fn the_stream_ends_when_the_account_is_signed_out() {
        let hub = NotifyHub::new();
        let mut events = Box::pin(events_for(hub.subscribe(), 1, 7));
        hub.close(CloseScope::Account { user_id: 8 });
        hub.publish(1, &[7]);
        assert_eq!(has_next(&mut events).await, Some(true), "a notice for 7");
        hub.close(CloseScope::Account { user_id: 7 });
        assert_eq!(has_next(&mut events).await, Some(false), "closed for 7");
    }
}
