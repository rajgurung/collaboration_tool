//! The bell: a list of someone's notifications, its unread badge, and a live
//! stream that tells open pages to refresh the badge.
use std::{collections::HashMap, convert::Infallible, time::Duration};

use axum::{
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::{DateTime, FixedOffset, Utc};
use futures_util::stream::{self, Stream};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{
    data::notify_hub::{NotifyEvent, NotifyHub},
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        conversations, memberships, messages,
        notifications::{self, kind, Notice},
        projects, task_assignees, task_notes, tasks,
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
    if let Some(hub) = ctx.shared_store.get::<NotifyHub>() {
        hub.publish(&sent);
    }
    Ok(sent)
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

/// After a task is created or changed: tells people newly assigned, and the
/// project owner about a new task or one that just became blocked.
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
        },
    )
    .await?;

    // Chores have no project, so no owner to tell.
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
    if let (Some(body), Some(owner)) = (event, project.owner_id) {
        if !told.contains(&owner) {
            told.extend(
                send(
                    ctx,
                    org_id,
                    actor,
                    &[owner],
                    &Notice {
                        kind: kind::PROJECT,
                        body,
                        link,
                    },
                )
                .await?,
            );
        }
    }
    Ok(())
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
        },
    )
    .await?;
    Ok(())
}

/// After a project is saved: tells a new owner.
///
/// # Errors
/// On database errors.
pub async fn project_saved(
    ctx: &AppContext,
    member: &CurrentMember,
    owner_before: Option<i64>,
    project: &projects::Model,
) -> Result<()> {
    let Some(owner) = project.owner_id.filter(|o| Some(*o) != owner_before) else {
        return Ok(());
    };
    send(
        ctx,
        member.org.id,
        member.user.id,
        &[owner],
        &Notice {
            kind: kind::OWNER,
            body: format!("made you owner of {}", project.name),
            link: format!("/roadmap?lane={}", project.lane),
        },
    )
    .await?;
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
    let told = send(
        ctx,
        org_id,
        message.user_id,
        &visible,
        &Notice {
            kind: kind::MENTION,
            body: format!("{MENTIONED_YOU} in {place}\n{quote}"),
            link: link.clone(),
        },
    )
    .await?;
    if !everyone {
        return Ok(());
    }
    // Mentions notify their own author, so leave the sender out here.
    let rest: Vec<i64> = team
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| members.contains(id) && *id != message.user_id && !told.contains(id))
        .collect();
    send(
        ctx,
        org_id,
        message.user_id,
        &rest,
        &Notice {
            kind: kind::MENTION,
            body: format!("mentioned everyone in {place}\n{quote}"),
            link,
        },
    )
    .await?;
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
    };

    let mentioned: Vec<i64> = notifications::mentioned_ids(&reply.body, &team)
        .into_iter()
        .filter(can_see)
        .collect();
    let mut told = send(
        ctx,
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
            send(
                ctx,
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
            send(
                ctx,
                org_id,
                author,
                &[parent.user_id],
                &notice(kind::REPLY, "replied to your message".to_string()),
            )
            .await?,
        );
    }
    let repliers: Vec<i64> = messages::Model::thread_stats(&ctx.db, org_id, &[parent.id])
        .await?
        .remove(&parent.id)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.user_id)
        .filter(|id| not_yet(id, &told))
        .collect();
    send(
        ctx,
        org_id,
        author,
        &repliers,
        &notice(kind::REPLY, "replied in a thread".to_string()),
    )
    .await?;
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
                let actor = n
                    .actor_id
                    .and_then(|id| names.get(&id).cloned())
                    .unwrap_or_else(|| "Someone".to_string());
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
    // Links are written by the app, but only ever follow ones on this site.
    let link = if n.link.starts_with('/') && !n.link.starts_with("//") {
        n.link
    } else {
        "/notifications".to_string()
    };
    Ok(redirect_response(&headers, &link))
}

#[debug_handler]
async fn read_all(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Response> {
    notifications::Model::mark_all_read(&ctx.db, member.org.id, member.user.id).await?;
    Ok(redirect_response(&headers, "/notifications"))
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

/// Server-sent events: one `notify` event whenever this person gets a new
/// notification. The page then reloads its badges.
#[debug_handler]
async fn stream(
    member: CurrentMember,
    State(ctx): State<AppContext>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let hub = ctx
        .shared_store
        .get::<NotifyHub>()
        .ok_or_else(|| Error::string("notify hub missing"))?;
    Ok(Sse::new(events_for(hub.subscribe(), member.user.id))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(25))))
}

/// The events for `me`. Ends when their sessions are ended, so a signed-out
/// browser stops listening; its reconnect is then sent to sign in.
fn events_for(
    rx: broadcast::Receiver<NotifyEvent>,
    me: i64,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    let notify = || Ok(Event::default().event("notify").data("1"));
    stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(NotifyEvent::Changed(id)) if id == me => return Some((notify(), rx)),
                Ok(NotifyEvent::SignedOut(id)) if id == me => return None,
                Ok(_) => {}
                // Missed events may include a sign-out: end the stream. The
                // browser reconnects through the same checks.
                Err(RecvError::Lagged(_) | RecvError::Closed) => return None,
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
    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn the_stream_ends_when_its_user_is_signed_out() {
        let hub = NotifyHub::new();
        let mut events = Box::pin(events_for(hub.subscribe(), 7));
        hub.publish(&[8, 7]);
        hub.sign_out(8);
        assert!(events.next().await.is_some(), "a notice for 7");
        hub.sign_out(7);
        assert!(events.next().await.is_none(), "closed for 7");
    }
}
