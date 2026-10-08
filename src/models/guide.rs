//! The "Getting started" checklist on Home. Each step ticks itself from what
//! the person has already done, so nothing is marked by hand.
use loco_rs::prelude::*;
use serde::Serialize;

use super::{
    conversations, memberships, messages, notifications, projects, task_assignees, task_notes,
    tasks,
};

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub title: &'static str,
    pub how: &'static str,
    pub action: &'static str,
    pub href: String,
    pub done: bool,
}

/// The steps for this person. Owners and admins set the team up; people who
/// joined find their way around.
///
/// # Errors
/// On database errors.
pub async fn steps<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    user_id: i64,
    sets_up_team: bool,
) -> ModelResult<Vec<Step>> {
    let general = conversations::Model::find_general(db, org_id).await?;
    let chat = format!("/chat/{}", general.id);
    let said_hello = messages::Entity::find()
        .in_tenant(org_id)
        .filter(messages::Column::ConversationId.eq(general.id))
        .filter(messages::Column::UserId.eq(user_id))
        .count(db)
        .await?
        > 0;
    let tagged = notifications::Entity::find()
        .in_tenant(org_id)
        .filter(notifications::Column::ActorId.eq(user_id))
        .filter(notifications::Column::Kind.eq(notifications::kind::MENTION))
        .count(db)
        .await?
        > 0;
    let hello = Step {
        title: "Say hello in #general",
        how: "Your team's shared chat channel.",
        action: "Open chat",
        href: chat.clone(),
        done: said_hello,
    };
    let tag = Step {
        title: "Tag a teammate",
        how: "Type @ in chat or a task note. They get a notification.",
        action: "Try it",
        href: chat,
        done: tagged,
    };

    if sets_up_team {
        let teammates = memberships::Entity::find()
            .in_tenant(org_id)
            .filter(memberships::Column::UserId.ne(user_id))
            .count(db)
            .await?;
        let has_project = projects::Entity::find().in_tenant(org_id).count(db).await? > 0;
        let has_task = tasks::Entity::find().in_tenant(org_id).count(db).await? > 0;
        Ok(vec![
            Step {
                title: "Invite your team",
                how: "Share your join link. You approve people as they arrive.",
                action: "Get the link",
                href: "/members".into(),
                done: teammates > 0,
            },
            Step {
                title: "Put a project on the roadmap",
                how: "Something the team is working on now.",
                action: "Add project",
                href: "/roadmap".into(),
                done: has_project,
            },
            Step {
                title: "Add a task",
                how: "Give it to someone, or keep it as a small chore.",
                action: "New task",
                href: "/tasks?new=1".into(),
                done: has_task,
            },
            hello,
            tag,
        ])
    } else {
        let on_a_task = task_assignees::Entity::find()
            .in_tenant(org_id)
            .filter(task_assignees::Column::UserId.eq(user_id))
            .count(db)
            .await?
            > 0;
        let wrote_note = task_notes::Entity::find()
            .in_tenant(org_id)
            .filter(task_notes::Column::AuthorId.eq(user_id))
            .count(db)
            .await?
            > 0;
        Ok(vec![
            Step {
                title: "Take on a task",
                how: "Add one on the board and put yourself on it, or ask to be added.",
                action: "Open tasks",
                href: "/tasks".into(),
                done: on_a_task,
            },
            Step {
                title: "Add a note to a task",
                how: "An update, a question or a decision. Notes live with the task.",
                action: "Your tasks",
                href: "/tasks?view=list&filter=mine".into(),
                done: wrote_note,
            },
            hello,
            tag,
        ])
    }
}
