use std::collections::HashMap;

use axum::http::HeaderMap;
use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        conversation_members::{self, ReadMark},
        conversations::{self, kind, GroupParams},
        field_error, memberships,
        messages::{self, MessageParams, ReplierStat},
        notifications::{mention_parts, Part, EVERYONE},
        reactions,
    },
    views::{
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
        reactions::FORMER_MEMBER,
        time,
    },
};

pub use crate::views::reactions::{chips, picks, Pick, ReactionChip, ReactionRow, Reactor};

const GROUP_FORM_ID: &str = "group-form";

#[derive(Debug, Default, Deserialize)]
struct LegacyQuery {
    c: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct UnreadQuery {
    style: Option<String>,
}

/// `?reply=1` opens a thread with the composer focused.
#[derive(Debug, Default, Deserialize)]
struct ThreadQuery {
    reply: Option<String>,
}

/// `?thread=1` when the sheet is opened from a thread page, where there is
/// no "Reply in thread".
#[derive(Debug, Default, Deserialize)]
struct ActionsQuery {
    thread: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DmForm {
    user_id: i64,
}

/// The state the client wants: `on` is "1" to add `emoji`, "0" to remove it.
#[derive(Debug, Default, Deserialize)]
struct ReactionForm {
    #[serde(default)]
    emoji: String,
    #[serde(default)]
    on: String,
}

/// One row in the conversation list.
#[derive(Debug, Serialize)]
pub struct ListItem {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub color: &'static str,
    pub last: Option<String>,
    pub time: String,
    pub unread: u64,
}

/// How many faces a thread's reply line shows before "+N".
const THREAD_FACES: usize = 3;

/// How many reader avatars a group receipt shows before "+N".
const RECEIPT_AVATARS: usize = 3;
/// Past this many readers, the screen-reader label names two and counts the rest.
const RECEIPT_LABEL_NAMES: usize = 5;

/// Who has read a message, for its author. A DM shows `text` ("Read"); a
/// group or channel shows reader avatars and uses `text` as the screen-reader
/// label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Receipt {
    pub dm: bool,
    pub text: String,
    /// All readers, by name.
    pub readers: Vec<Person>,
    /// The first few readers, shown as avatars.
    pub shown: Vec<Person>,
    /// Readers beyond `shown`.
    pub more: usize,
}

/// Reaction rows for the given messages, keyed by message id. People no
/// longer on the team show as "Former member"; their reactions still count.
///
/// # Errors
/// On database errors.
pub async fn reaction_rows(
    ctx: &AppContext,
    org_id: i64,
    message_ids: &[i64],
    names: &HashMap<i64, String>,
) -> Result<HashMap<i64, Vec<ReactionRow>>> {
    Ok(reactions::Model::for_messages(&ctx.db, org_id, message_ids)
        .await?
        .into_iter()
        .map(|(id, list)| {
            let rows = list
                .into_iter()
                .map(|r| ReactionRow::named(r.emoji, r.user_id, names))
                .collect();
            (id, rows)
        })
        .collect())
}

/// "3 replies · 14:05" under a message that has a thread. The same for every
/// viewer, so a live update can carry it as is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadLine {
    pub url: String,
    pub count: i64,
    /// "1 reply" or "N replies".
    pub label: String,
    /// When the last reply was sent.
    pub last: String,
    /// Who replied, in the order they first replied.
    pub faces: Vec<Person>,
    /// Repliers beyond `faces`.
    pub more: usize,
}

/// A thread's own page.
#[must_use]
pub fn thread_url(conversation_id: i64, parent_id: i64) -> String {
    format!("/chat/{conversation_id}/threads/{parent_id}")
}

fn replies_label(count: i64) -> String {
    if count == 1 {
        "1 reply".to_string()
    } else {
        format!("{count} replies")
    }
}

/// The reply line for a parent, from its [`ReplierStat`]s (in first-reply
/// order). `None` when nobody has replied.
#[must_use]
pub fn thread_line(
    conversation_id: i64,
    parent_id: i64,
    stats: &[ReplierStat],
    names: &HashMap<i64, String>,
    tz: Tz,
) -> Option<ThreadLine> {
    let last = stats.iter().map(|s| s.last_at).max()?;
    let count = stats.iter().map(|s| s.replies).sum();
    let faces: Vec<Person> = stats
        .iter()
        .take(THREAD_FACES)
        .map(|s| {
            let username = names
                .get(&s.user_id)
                .cloned()
                .unwrap_or_else(|| FORMER_MEMBER.to_string());
            Person {
                id: s.user_id,
                color: avatar_color(&username),
                username,
            }
        })
        .collect();
    Some(ThreadLine {
        url: thread_url(conversation_id, parent_id),
        count,
        label: replies_label(count),
        last: reply_when(&last, tz),
        more: stats.len() - faces.len(),
        faces,
    })
}

/// One rendered chat message. `own` decides the bubble style for the viewer;
/// `start` and `show_time` group consecutive messages from the same person.
/// Every bubble shows its time; `show_time` now only marks the last message
/// in a run, which is the one that shows the author's avatar.
/// `read_receipts` is set on the viewer's own messages, so the page has a slot
/// for `receipt` even before anyone has read it.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: i64,
    pub conversation_id: i64,
    pub author_id: i64,
    pub author: String,
    pub color: &'static str,
    pub body: String,
    /// The body split so mentions of teammates can be highlighted.
    pub parts: Vec<Part>,
    pub at: String,
    pub own: bool,
    pub start: bool,
    pub show_time: bool,
    pub read_receipts: bool,
    pub receipt: Option<Receipt>,
    pub reactions: Vec<ReactionChip>,
    /// The six choices for the hover toolbar and the sheet.
    pub picks: Vec<Pick>,
    /// Set on a thread reply.
    pub parent_id: Option<i64>,
    /// The reply line, on a main-feed message that has replies.
    pub thread: Option<ThreadLine>,
    /// Offers "Reply in thread": top-level messages in the main feed only.
    pub can_reply: bool,
}

impl MessageView {
    #[must_use]
    pub fn new(
        message: &messages::Model,
        names: &HashMap<i64, String>,
        viewer_id: i64,
        tz: Tz,
    ) -> Self {
        let author = names
            .get(&message.user_id)
            .cloned()
            .unwrap_or_else(|| FORMER_MEMBER.to_string());
        Self {
            id: message.id,
            conversation_id: message.conversation_id,
            author_id: message.user_id,
            color: avatar_color(&author),
            author,
            parts: mention_parts(
                &message.body,
                &names
                    .iter()
                    .map(|(id, n)| (*id, n.clone()))
                    // Not a person; only its name is used here.
                    .chain([(0, EVERYONE.to_string())])
                    .collect::<Vec<_>>(),
            ),
            body: message.body.clone(),
            at: time::local(message.created_at, tz)
                .format("%H:%M")
                .to_string(),
            own: message.user_id == viewer_id,
            start: true,
            show_time: true,
            read_receipts: false,
            receipt: None,
            reactions: Vec::new(),
            picks: picks(&[]),
            parent_id: message.parent_id,
            thread: None,
            can_reply: message.parent_id.is_none(),
        }
    }

    /// Fills in the reactions as `viewer_id` sees them.
    pub fn set_reactions(&mut self, rows: &[ReactionRow], viewer_id: i64) {
        self.reactions = chips(rows, viewer_id);
        self.picks = picks(&self.reactions);
    }
}

/// Who has read a message. Reading is in order, so a member has read it once
/// their `last_read_at` reaches its `created_at`. The author never counts, nor
/// do people no longer on the team. Nothing shows until someone has read it.
/// There is no "everyone": people who join later would make it misleading.
#[must_use]
pub fn receipt(
    conversation_kind: &str,
    author_id: i64,
    created_at: DateTime<FixedOffset>,
    marks: &[ReadMark],
    names: &HashMap<i64, String>,
) -> Option<Receipt> {
    let mut readers: Vec<Person> = marks
        .iter()
        .filter(|(id, at)| *id != author_id && at.is_some_and(|at| at >= created_at))
        .filter_map(|(id, _)| {
            names.get(id).map(|name| Person {
                id: *id,
                color: avatar_color(name),
                username: name.clone(),
            })
        })
        .collect();
    if readers.is_empty() {
        return None;
    }
    readers.sort_by(|a, b| a.username.cmp(&b.username));
    let dm = conversation_kind == kind::DM;
    let text = if dm {
        "Read".to_string()
    } else {
        let names: Vec<&str> = readers.iter().map(|p| p.username.as_str()).collect();
        if names.len() > RECEIPT_LABEL_NAMES {
            format!(
                "Read by {} and {} others",
                names[..2].join(", "),
                names.len() - 2
            )
        } else {
            format!("Read by {}", names.join(", "))
        }
    };
    let shown: Vec<Person> = readers.iter().take(RECEIPT_AVATARS).cloned().collect();
    Some(Receipt {
        dm,
        text,
        more: readers.len() - shown.len(),
        shown,
        readers,
    })
}

/// The main feed: marks where each run of messages from one person starts
/// and ends, and adds receipts to the viewer's own messages, reactions to
/// every message and reply lines to those with threads.
#[allow(clippy::too_many_arguments)]
fn grouped(
    conversation: &conversations::Model,
    list: &[messages::Model],
    marks: &[ReadMark],
    reactions: &HashMap<i64, Vec<ReactionRow>>,
    threads: &HashMap<i64, Vec<ReplierStat>>,
    names: &HashMap<i64, String>,
    viewer_id: i64,
    tz: Tz,
) -> Vec<MessageView> {
    let mut views: Vec<MessageView> = list
        .iter()
        .map(|m| {
            let mut view = MessageView::new(m, names, viewer_id, tz);
            if let Some(rows) = reactions.get(&m.id) {
                view.set_reactions(rows, viewer_id);
            }
            if let Some(stats) = threads.get(&m.id) {
                view.thread = thread_line(conversation.id, m.id, stats, names, tz);
            }
            if view.own {
                view.read_receipts = true;
                view.receipt = receipt(&conversation.kind, m.user_id, m.created_at, marks, names);
            }
            view
        })
        .collect();
    mark_runs(&mut views);
    views
}

/// Marks where each run of messages from one person starts and ends.
fn mark_runs(views: &mut [MessageView]) {
    for i in 0..views.len() {
        let same_as_prev = i > 0 && views[i - 1].author_id == views[i].author_id;
        let same_as_next = i + 1 < views.len() && views[i + 1].author_id == views[i].author_id;
        views[i].start = !same_as_prev;
        views[i].show_time = !same_as_next;
    }
}

/// Usernames of approved members, keyed by user id.
pub async fn names(ctx: &AppContext, org_id: i64) -> Result<HashMap<i64, String>> {
    Ok(memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .collect())
}

/// What a conversation is called for this viewer: the channel name, or the other person in a DM.
fn label(
    conversation: &conversations::Model,
    members: &[i64],
    names: &HashMap<i64, String>,
    viewer_id: i64,
) -> String {
    if conversation.kind == kind::DM {
        members
            .iter()
            .find(|id| **id != viewer_id)
            .and_then(|id| names.get(id).cloned())
            .unwrap_or_else(|| FORMER_MEMBER.to_string())
    } else {
        conversation.name.clone().unwrap_or_default()
    }
}

/// Like [`when`], with "Yesterday" for the day before. For a thread's last
/// reply; the chat list keeps [`when`].
fn reply_when(at: &DateTime<FixedOffset>, tz: Tz) -> String {
    let day = time::local(*at, tz).date_naive();
    if day.succ_opt() == Some(time::today(tz)) {
        "Yesterday".to_string()
    } else {
        when(at, tz)
    }
}

/// "14:05" today, "Mon" this week, otherwise "3 Oct", in the organisation's zone.
fn when(at: &DateTime<FixedOffset>, tz: Tz) -> String {
    let age = Utc::now().signed_duration_since(at.with_timezone(&Utc));
    let at = time::local(*at, tz);
    if at.date_naive() == time::today(tz) {
        at.format("%H:%M").to_string()
    } else if age.num_days() < 6 {
        at.format("%a").to_string()
    } else {
        at.format("%-d %b").to_string()
    }
}

/// The viewer's conversations with last message and unread count, most recent first.
pub async fn list_items(
    ctx: &AppContext,
    member: &CurrentMember,
    names: &HashMap<i64, String>,
) -> Result<Vec<ListItem>> {
    let me = member.user.id;
    let list = conversations::Model::list_for_user(&ctx.db, member.org.id, me).await?;
    let ids: Vec<i64> = list.iter().map(|c| c.id).collect();
    let latest = messages::Model::latest_per_conversation(&ctx.db, member.org.id, &ids).await?;
    let unread = conversation_members::Model::unread_counts(&ctx.db, member.org.id, me).await?;
    let mut items = Vec::with_capacity(list.len());
    for c in &list {
        let members = c.member_ids(&ctx.db).await?;
        let label = label(c, &members, names, me);
        let last = latest.get(&c.id);
        items.push((
            last.map(|m| m.created_at),
            ListItem {
                id: c.id,
                kind: c.kind.clone(),
                color: avatar_color(&label),
                last: last.map(|m| {
                    let who = if m.user_id == me {
                        "You".to_string()
                    } else {
                        names
                            .get(&m.user_id)
                            .cloned()
                            .unwrap_or_else(|| FORMER_MEMBER.to_string())
                    };
                    let body: String = m.body.chars().take(80).collect();
                    if c.kind == kind::DM && m.user_id != me {
                        body
                    } else {
                        format!("{who}: {body}")
                    }
                }),
                time: last
                    .map(|m| when(&m.created_at, member.tz()))
                    .unwrap_or_default(),
                unread: unread.get(&c.id).copied().unwrap_or(0),
                label,
            },
        ));
    }
    // Channels with activity first by recency; quiet ones keep their order at the end.
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    Ok(items.into_iter().map(|(_, item)| item).collect())
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(legacy): Query<LegacyQuery>,
) -> Result<Response> {
    if let Some(id) = legacy.c {
        return format::redirect(&format!("/chat/{id}"));
    }
    let names = names(&ctx, member.org.id).await?;
    let items = list_items(&ctx, &member, &names).await?;
    format::render().view(
        &v,
        "chat/index.html",
        member.page("chat", data!({ "items": items })),
    )
}

#[debug_handler]
async fn show(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let me = member.user.id;
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, me).await?;
    super::chat_ws::mark_read(&ctx, member.org.id, conversation.id, &conversation.kind, me).await?;
    let names = names(&ctx, member.org.id).await?;
    let items = list_items(&ctx, &member, &names).await?;
    let members = conversation.member_ids(&ctx.db).await?;
    let people = people(&members, &names);
    let feed = main_feed(&ctx, &member, &conversation, &names).await?;
    let label = label(&conversation, &members, &names, me);
    let mention_names = mention_names(&conversation, &people, me);
    format::render().view(
        &v,
        "chat/show.html",
        member.page(
            "chat",
            data!({
                "items": items,
                "conversation": {
                    "id": conversation.id,
                    "kind": conversation.kind,
                    "color": avatar_color(&label),
                    "label": label,
                    "members": people,
                    "mention_names": mention_names,
                },
                "messages": feed,
            }),
        ),
    )
}

/// A conversation's approved members, for avatars and @mentions.
fn people(members: &[i64], names: &HashMap<i64, String>) -> Vec<Person> {
    members
        .iter()
        .filter_map(|id| names.get(id).map(|n| (*id, n.clone())))
        .map(|(id, username)| Person {
            id,
            color: avatar_color(&username),
            username,
        })
        .collect()
}

/// Who the composer offers after "@": everyone else, and `everyone` outside DMs.
fn mention_names(conversation: &conversations::Model, people: &[Person], me: i64) -> String {
    let names = super::tasks::mention_names(
        &people
            .iter()
            .map(|p| (p.id, p.username.clone()))
            .collect::<Vec<_>>(),
        me,
    );
    if conversation.kind == kind::DM {
        names
    } else {
        format!("{EVERYONE},{names}")
    }
}

/// The main feed as the viewer sees it: three queries (messages, reactions,
/// threads) whatever the number of messages, plus the read marks.
async fn main_feed(
    ctx: &AppContext,
    member: &CurrentMember,
    conversation: &conversations::Model,
    names: &HashMap<i64, String>,
) -> Result<Vec<MessageView>> {
    let list = messages::Model::recent(&ctx.db, conversation).await?;
    let ids: Vec<i64> = list.iter().map(|m| m.id).collect();
    Ok(grouped(
        conversation,
        &list,
        &conversation_members::Model::read_marks(&ctx.db, member.org.id, conversation.id).await?,
        &reaction_rows(ctx, member.org.id, &ids, names).await?,
        &messages::Model::thread_stats(&ctx.db, member.org.id, &ids).await?,
        names,
        member.user.id,
        member.tz(),
    ))
}

/// Total unread across the viewer's conversations, as a badge (or nothing).
#[debug_handler]
async fn unread(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Query(query): Query<UnreadQuery>,
) -> Result<Response> {
    let total: u64 =
        conversation_members::Model::unread_counts(&ctx.db, member.org.id, member.user.id)
            .await?
            .values()
            .sum();
    if total == 0 {
        return format::html("");
    }
    let class = if query.style.as_deref() == Some("tab") {
        "tab-badge"
    } else {
        "badge"
    };
    let shown = if total > 99 {
        "99+".to_string()
    } else {
        total.to_string()
    };
    format::html(&format!(
        r#"<span class="{class}" aria-label="{total} unread">{shown}</span>"#
    ))
}

/// Plain HTTP send, used when the live connection is not available.
#[debug_handler]
async fn send(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(params): Form<MessageParams>,
) -> Result<Response> {
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    let result = messages::Model::create(&ctx.db, &conversation, member.user.id, &params).await;
    if let Ok(message) = &result {
        super::chat_ws::publish(&ctx, member.org.id, message).await?;
        super::notifications::message_sent(&ctx, member.org.id, &conversation, message).await?;
    }
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(
            &headers,
            &format!("/chat/{}", conversation.id),
        )),
        Ok(message) => {
            let names = names(&ctx, member.org.id).await?;
            let mut view = MessageView::new(&message, &names, member.user.id, member.tz());
            view.read_receipts = true;
            format::render().view(&v, "chat/_message.html", data!({ "message": view }))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let message = errors.values().next().cloned().unwrap_or_default();
            format::render()
                .status(422)
                .header("HX-Reswap", "none")
                .header("HX-Trigger", toast("error", &message))
                .empty()
        }
    }
}

/// The message list on its own, for reloading after a reconnect.
#[debug_handler]
async fn feed(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    super::chat_ws::mark_read(
        &ctx,
        member.org.id,
        conversation.id,
        &conversation.kind,
        member.user.id,
    )
    .await?;
    let names = names(&ctx, member.org.id).await?;
    let feed = main_feed(&ctx, &member, &conversation, &names).await?;
    format::render().view(&v, "chat/_feed.html", data!({ "messages": feed }))
}

/// Adds or removes the viewer's reaction, tells every open socket, and returns
/// the message's chips and toolbar as the viewer now sees them.
#[debug_handler]
async fn react(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, message_id)): Path<(i64, i64)>,
    Form(form): Form<ReactionForm>,
) -> Result<Response> {
    let me = member.user.id;
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, me).await?;
    let message = messages::Model::find_in_conversation(&ctx.db, &conversation, message_id).await?;
    let result = match form.on.as_str() {
        "1" | "0" => {
            reactions::Model::set(&ctx.db, &message, me, &form.emoji, form.on == "1").await
        }
        _ => Err(field_error("on", "Couldn't change your reaction.")),
    };
    if let Err(err) = result {
        let errors = field_errors(&err).ok_or(err)?;
        let text = errors.values().next().cloned().unwrap_or_default();
        return format::render()
            .status(422)
            .header("HX-Reswap", "none")
            .header("HX-Trigger", toast("error", &text))
            .empty();
    }
    let names = names(&ctx, member.org.id).await?;
    let rows = std::sync::Arc::new(
        reaction_rows(&ctx, member.org.id, &[message.id], &names)
            .await?
            .remove(&message.id)
            .unwrap_or_default(),
    );
    // The change is saved, so answer with it even if the broadcast fails;
    // other screens catch up on their next reload.
    if let Err(err) = super::chat_ws::publish_reactions(&ctx, &message, rows.clone()) {
        tracing::error!(error = %err, message_id = message.id, "could not broadcast reactions");
    }
    format::html(&render_reactions(
        &v,
        conversation.id,
        message.id,
        &rows,
        me,
    )?)
}

/// A message's chips and hover toolbar for one viewer, as out-of-band swaps of
/// `#reacts-{id}` and `#tools-{id}`.
///
/// # Errors
/// When the template fails to render.
pub fn render_reactions(
    v: &TeraView,
    conversation_id: i64,
    message_id: i64,
    rows: &[ReactionRow],
    viewer_id: i64,
) -> Result<String> {
    let reactions = chips(rows, viewer_id);
    v.render(
        "chat/_reactions_update.html",
        data!({
            "message": {
                "id": message_id,
                "conversation_id": conversation_id,
                "picks": picks(&reactions),
                "reactions": reactions,
            },
            "oob": true,
        }),
    )
}

/// The message's action sheet: which message, the six reactions, who reacted
/// and, for a top-level message outside its thread, "Reply in thread".
#[debug_handler]
async fn actions(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, message_id)): Path<(i64, i64)>,
    Query(query): Query<ActionsQuery>,
) -> Result<Response> {
    let me = member.user.id;
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, me).await?;
    let message = messages::Model::find_in_conversation(&ctx.db, &conversation, message_id).await?;
    let names = names(&ctx, member.org.id).await?;
    let rows = reaction_rows(&ctx, member.org.id, &[message.id], &names).await?;
    let mut view = MessageView::new(&message, &names, me, member.tz());
    view.set_reactions(rows.get(&message.id).map_or(&[], Vec::as_slice), me);
    view.can_reply = view.can_reply && query.thread.is_none();
    if view.can_reply {
        let stats = messages::Model::thread_stats(&ctx.db, member.org.id, &[message.id]).await?;
        view.thread = stats
            .get(&message.id)
            .and_then(|s| thread_line(conversation.id, message.id, s, &names, member.tz()));
    }
    let reaction_total: usize = view.reactions.iter().map(|c| c.count).sum();
    format::render().view(
        &v,
        "chat/_actions.html",
        data!({ "message": view, "reaction_total": reaction_total }),
    )
}

/// A thread's parent and replies as its page shows them: no reply line, no
/// "Reply in thread" and no read receipts, which threads do not have.
async fn thread_feed_data(
    ctx: &AppContext,
    member: &CurrentMember,
    parent: &messages::Model,
    names: &HashMap<i64, String>,
) -> Result<serde_json::Value> {
    let me = member.user.id;
    let tz = member.tz();
    let replies = messages::Model::replies(&ctx.db, parent).await?;
    let ids: Vec<i64> = std::iter::once(parent.id)
        .chain(replies.iter().map(|m| m.id))
        .collect();
    let reactions = reaction_rows(ctx, member.org.id, &ids, names).await?;
    let view = |m: &messages::Model| {
        let mut view = MessageView::new(m, names, me, tz);
        view.can_reply = false;
        if let Some(rows) = reactions.get(&m.id) {
            view.set_reactions(rows, me);
        }
        view
    };
    let mut reply_views: Vec<MessageView> = replies.iter().map(view).collect();
    mark_runs(&mut reply_views);
    // The true total: the page shows only the latest replies.
    let count: i64 = messages::Model::thread_stats(&ctx.db, member.org.id, &[parent.id])
        .await?
        .get(&parent.id)
        .map_or(0, |stats| stats.iter().map(|s| s.replies).sum());
    // Who the empty thread says will be told: the parent's author, if that
    // is someone else still on the team.
    let notify = (parent.user_id != me)
        .then(|| names.get(&parent.user_id).cloned())
        .flatten();
    Ok(serde_json::json!({
        "parent": view(parent),
        "replies": reply_views,
        "count": count,
        "count_label": replies_label(count),
        "notify": notify,
        "in_thread": true,
    }))
}

/// The conversation and the thread's parent, both checked for this viewer.
async fn find_thread(
    ctx: &AppContext,
    member: &CurrentMember,
    id: i64,
    message_id: i64,
) -> ModelResult<(conversations::Model, messages::Model)> {
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    let parent = messages::Model::find_thread_parent(&ctx.db, &conversation, message_id).await?;
    Ok((conversation, parent))
}

/// A thread's page: the parent, its replies and a composer. Does not mark the
/// conversation read; replies are not part of the unread count. A missing
/// thread and one the viewer cannot see get the same 404 page, so it never
/// confirms that a private conversation exists.
#[debug_handler]
async fn thread(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, message_id)): Path<(i64, i64)>,
    Query(query): Query<ThreadQuery>,
) -> Result<Response> {
    let me = member.user.id;
    let found = find_thread(&ctx, &member, id, message_id).await;
    let names = names(&ctx, member.org.id).await?;
    let items = list_items(&ctx, &member, &names).await?;
    let (conversation, parent) = match found {
        Ok(found) => found,
        Err(ModelError::EntityNotFound) => {
            return format::render().status(404).view(
                &v,
                "chat/thread_missing.html",
                member.page("chat", data!({ "items": items })),
            );
        }
        Err(err) => return Err(err.into()),
    };
    let members = conversation.member_ids(&ctx.db).await?;
    let people = people(&members, &names);
    let label = label(&conversation, &members, &names, me);
    let (subtitle, back) = match conversation.kind.as_str() {
        kind::DM => (format!("With {label}"), label.clone()),
        kind::CHANNEL => (format!("# {label}"), format!("#{label}")),
        _ => (label.clone(), label.clone()),
    };
    let mut page = thread_feed_data(&ctx, &member, &parent, &names).await?;
    page["items"] = serde_json::json!(items);
    page["conversation"] = serde_json::json!({
        "id": conversation.id,
        "kind": conversation.kind,
        "label": label,
        "mention_names": mention_names(&conversation, &people, me),
    });
    page["thread"] = serde_json::json!({
        "parent_id": parent.id,
        "subtitle": subtitle,
        "back": back,
        "focus": query.reply.as_deref() == Some("1"),
    });
    format::render().view(&v, "chat/thread.html", member.page("chat", page))
}

/// The thread's parent and replies on their own, for reloading after a reconnect.
#[debug_handler]
async fn thread_feed(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, message_id)): Path<(i64, i64)>,
) -> Result<Response> {
    let (_, parent) = find_thread(&ctx, &member, id, message_id).await?;
    let names = names(&ctx, member.org.id).await?;
    let data = thread_feed_data(&ctx, &member, &parent, &names).await?;
    format::render().view(&v, "chat/_thread_feed.html", data)
}

/// Plain HTTP reply in a thread, used when the live connection is not available.
#[debug_handler]
async fn reply(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path((id, message_id)): Path<(i64, i64)>,
    Form(params): Form<MessageParams>,
) -> Result<Response> {
    let (conversation, parent) = find_thread(&ctx, &member, id, message_id).await?;
    let result =
        messages::Model::create_reply(&ctx.db, &conversation, &parent, member.user.id, &params)
            .await;
    if let Ok(reply) = &result {
        super::chat_ws::publish_reply(&ctx, member.org.id, &parent, reply).await?;
        super::notifications::reply_sent(&ctx, member.org.id, &conversation, &parent, reply)
            .await?;
    }
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(
            &headers,
            &thread_url(conversation.id, parent.id),
        )),
        Ok(reply) => {
            let names = names(&ctx, member.org.id).await?;
            let view = MessageView::new(&reply, &names, member.user.id, member.tz());
            format::render().view(
                &v,
                "chat/_message.html",
                data!({ "message": view, "in_thread": true }),
            )
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let message = errors.values().next().cloned().unwrap_or_default();
            format::render()
                .status(422)
                .header("HX-Reswap", "none")
                .header("HX-Trigger", toast("error", &message))
                .empty()
        }
    }
}

async fn team_except_me(ctx: &AppContext, member: &CurrentMember) -> Result<Vec<Person>> {
    Ok(
        Person::from_team(memberships::Model::team(&ctx.db, member.org.id).await?)
            .into_iter()
            .filter(|p| p.id != member.user.id)
            .collect(),
    )
}

#[debug_handler]
async fn new_group(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let team = team_except_me(&ctx, &member).await?;
    format::render().view(
        &v,
        "chat/_group_form.html",
        data!({ "team": team, "form": { "name": "", "member_ids": [] }, "errors": FieldErrors::new() }),
    )
}

#[debug_handler]
async fn create_group(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    axum_extra::extract::Form(params): axum_extra::extract::Form<GroupParams>,
) -> Result<Response> {
    match conversations::Model::create_group(&ctx.db, member.org.id, member.user.id, &params).await
    {
        Ok(group) => Ok(redirect_response(&headers, &format!("/chat/{}", group.id))),
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let team = team_except_me(&ctx, &member).await?;
            invalid_form(
                &v,
                "chat/_group_form.html",
                GROUP_FORM_ID,
                data!({ "team": team, "form": { "name": params.name, "member_ids": params.member_ids }, "errors": errors }),
            )
        }
    }
}

#[debug_handler]
async fn new_dm(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let team = team_except_me(&ctx, &member).await?;
    format::render().view(&v, "chat/_dm_picker.html", data!({ "team": team }))
}

#[debug_handler]
async fn start_dm(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<DmForm>,
) -> Result<Response> {
    match conversations::Model::start_dm(&ctx.db, member.org.id, member.user.id, form.user_id).await
    {
        Ok(dm) => Ok(redirect_response(&headers, &format!("/chat/{}", dm.id))),
        Err(ModelError::Message(msg)) => Err(Error::BadRequest(msg)),
        Err(err) => Err(err.into()),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/chat", get(index))
        .add("/chat/unread", get(unread))
        .add("/chat/groups/new", get(new_group))
        .add("/chat/groups", post(create_group))
        .add("/chat/dms/new", get(new_dm))
        .add("/chat/dms", post(start_dm))
        .add("/chat/{id}", get(show))
        .add("/chat/{id}/messages", post(send))
        .add("/chat/{id}/feed", get(feed))
        .add("/chat/{id}/messages/{message_id}/reactions", post(react))
        .add("/chat/{id}/messages/{message_id}/actions", get(actions))
        .add("/chat/{id}/threads/{message_id}", get(thread))
        .add("/chat/{id}/threads/{message_id}/feed", get(thread_feed))
        .add("/chat/{id}/threads/{message_id}/replies", post(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: u32) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(&format!("2026-10-08T10:{minute:02}:00+00:00")).unwrap()
    }

    fn team() -> HashMap<i64, String> {
        [
            (1, "alice"),
            (2, "bob"),
            (3, "carol"),
            (4, "dev"),
            (5, "eve"),
        ]
        .into_iter()
        .map(|(id, n)| (id, n.to_string()))
        .collect()
    }

    fn names(people: &[Person]) -> Vec<&str> {
        people.iter().map(|p| p.username.as_str()).collect()
    }

    #[test]
    fn dms_say_read_once_the_other_person_has_read() {
        let sent = at(5);
        let unread = [(1, Some(at(9))), (2, Some(at(4)))];
        assert_eq!(receipt(kind::DM, 1, sent, &unread, &team()), None);
        let never_opened = [(1, Some(at(9))), (2, None)];
        assert_eq!(receipt(kind::DM, 1, sent, &never_opened, &team()), None);
        let read = [(1, None), (2, Some(at(5)))];
        let r = receipt(kind::DM, 1, sent, &read, &team()).unwrap();
        assert_eq!(r.text, "Read");
        assert!(r.dm);
        assert_eq!(names(&r.readers), vec!["bob"]);
    }

    #[test]
    fn groups_count_readers_and_never_the_author() {
        let sent = at(5);
        let some = [(1, Some(at(9))), (2, Some(at(6))), (3, Some(at(1)))];
        let r = receipt(kind::GROUP, 1, sent, &some, &team()).unwrap();
        assert_eq!(r.text, "Read by bob");
        assert_eq!(names(&r.readers), vec!["bob"]);
        assert_eq!(r.readers[0].color, avatar_color("bob"));
        assert!(!r.dm);

        // Even when every other member has read it, the label names them.
        let all = [(3, Some(at(7))), (1, None), (2, Some(at(6)))];
        let r = receipt(kind::GROUP, 1, sent, &all, &team()).unwrap();
        assert_eq!(r.text, "Read by bob, carol");
        assert_eq!(names(&r.readers), vec!["bob", "carol"]);
    }

    #[test]
    fn people_off_the_team_are_ignored() {
        let sent = at(5);
        let marks = [(1, None), (2, Some(at(6))), (99, Some(at(6)))];
        let r = receipt(kind::GROUP, 1, sent, &marks, &team()).unwrap();
        assert_eq!(r.text, "Read by bob");
        assert_eq!(names(&r.readers), vec!["bob"]);
    }

    #[test]
    fn groups_show_three_avatars_then_a_count() {
        let marks = [
            (1, None),
            (5, Some(at(9))),
            (4, Some(at(9))),
            (3, Some(at(9))),
            (2, Some(at(9))),
        ];
        let r = receipt(kind::GROUP, 1, at(5), &marks, &team()).unwrap();
        assert_eq!(names(&r.shown), vec!["bob", "carol", "dev"]);
        assert_eq!(r.more, 1);
        assert_eq!(r.readers.len(), 4);

        let partial = [(1, None), (2, Some(at(9))), (3, Some(at(9))), (4, None)];
        let r = receipt(kind::GROUP, 1, at(5), &partial, &team()).unwrap();
        assert_eq!(r.text, "Read by bob, carol");
        assert_eq!(r.more, 0);
    }

    #[test]
    fn big_groups_name_two_readers_and_count_the_rest() {
        let team: HashMap<i64, String> = (1..=8).map(|id| (id, format!("u{id}"))).collect();
        let mut marks: Vec<ReadMark> = (2..=7).map(|id| (id, Some(at(9)))).collect();
        marks.push((1, None));
        marks.push((8, None));
        let r = receipt(kind::GROUP, 1, at(5), &marks, &team).unwrap();
        assert_eq!(r.text, "Read by u2, u3 and 4 others");
        assert_eq!(r.readers.len(), 6);
        assert_eq!(r.more, 3);
    }

    #[test]
    fn nobody_left_to_read_means_no_receipt() {
        // Everyone else has left the group.
        let marks = [(1, Some(at(9))), (99, Some(at(9)))];
        assert_eq!(receipt(kind::GROUP, 1, at(5), &marks, &team()), None);
    }

    fn stat(user_id: i64, replies: i64, first: u32, last: u32) -> ReplierStat {
        ReplierStat {
            parent_id: 9,
            user_id,
            replies,
            first_at: at(first),
            last_at: at(last),
        }
    }

    #[test]
    fn reply_lines_count_every_reply_and_show_three_faces() {
        // In first-reply order: bob, alice, someone who left, carol.
        let stats = [
            stat(2, 3, 1, 20),
            stat(1, 1, 2, 2),
            stat(99, 1, 3, 3),
            stat(3, 2, 4, 30),
        ];
        let line = thread_line(7, 9, &stats, &team(), chrono_tz::UTC).unwrap();
        assert_eq!(line.url, "/chat/7/threads/9");
        assert_eq!(line.count, 7);
        assert_eq!(line.label, "7 replies");
        assert_eq!(names(&line.faces), vec!["bob", "alice", "Former member"]);
        assert_eq!(line.more, 1);
        assert_eq!(
            line.last,
            reply_when(&at(30), chrono_tz::UTC),
            "the latest reply"
        );

        let one = thread_line(7, 9, &[stat(2, 1, 1, 1)], &team(), chrono_tz::UTC).unwrap();
        assert_eq!(one.label, "1 reply");
        assert_eq!(one.more, 0);
        assert_eq!(thread_line(7, 9, &[], &team(), chrono_tz::UTC), None);
    }

    #[test]
    fn reply_times_say_yesterday() {
        let now = Utc::now();
        let today: DateTime<FixedOffset> = now.into();
        assert_eq!(
            reply_when(&today, chrono_tz::UTC),
            today.format("%H:%M").to_string()
        );
        let yesterday: DateTime<FixedOffset> = (now - chrono::Duration::days(1)).into();
        assert_eq!(reply_when(&yesterday, chrono_tz::UTC), "Yesterday");
        let older: DateTime<FixedOffset> = (now - chrono::Duration::days(3)).into();
        assert_eq!(
            reply_when(&older, chrono_tz::UTC),
            when(&older, chrono_tz::UTC)
        );
    }

    #[test]
    fn channels_show_receipts_like_groups() {
        let marks = [(1, None), (2, Some(at(9))), (3, Some(at(9)))];
        let r = receipt(kind::CHANNEL, 1, at(5), &marks, &team()).unwrap();
        assert!(!r.dm);
        assert_eq!(r.text, "Read by bob, carol");
        assert_eq!(names(&r.shown), vec!["bob", "carol"]);
    }
}
