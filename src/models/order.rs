//! Card order inside a board column or a roadmap lane, shared by tasks and
//! projects. `sort_order` is one number per row, organisation-wide; a column
//! is the rows with one status (or one lane), in `sort_order`, then `id`.
use loco_rs::prelude::*;
use sea_orm::{sqlx, DbBackend, RuntimeErr, Statement};

/// Room left between neighbours after a renumber, and the step for "the bottom".
pub const STEP: i64 = 1024;

/// What a move answers when it waited too long for another one.
pub const BUSY: &str = "Someone else is moving cards. Try again.";

/// Where a card lands, relative to a card the person can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Before(i64),
    After(i64),
    End,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Slot {
    /// Give the moved card this number. Nothing else changes.
    At(i64),
    /// No room: these `(id, sort_order)` pairs are the column's new numbers,
    /// only those that change, and always the moved card.
    Renumber(Vec<(i64, i64)>),
}

/// Picks the moved card's number. `column` is the target column without the
/// moved card, as `(id, sort_order)` in display order. `None` when the
/// anchor isn't in it.
#[must_use]
pub fn slot(column: &[(i64, i64)], moved: i64, place: Place) -> Option<Slot> {
    let at = |anchor: i64| column.iter().position(|(id, _)| *id == anchor);
    let (lo, hi, index) = match place {
        Place::Before(anchor) => {
            let i = at(anchor)?;
            let lo = i.checked_sub(1).map(|j| column[j].1);
            (lo, Some(column[i].1), i)
        }
        Place::After(anchor) => {
            let i = at(anchor)?;
            (Some(column[i].1), column.get(i + 1).map(|c| c.1), i + 1)
        }
        Place::End => (column.last().map(|c| c.1), None, column.len()),
    };
    let number = match (lo, hi) {
        (None, None) => Some(STEP),
        (Some(lo), None) => lo.checked_add(STEP),
        (None, Some(hi)) => hi.checked_sub(STEP),
        // A gap of 0 is a tie (two creates at once); 1 has no room.
        (Some(lo), Some(hi)) => hi
            .checked_sub(lo)
            .filter(|gap| *gap >= 2)
            .map(|gap| lo + gap / 2),
    };
    Some(number.map_or_else(|| renumber(column, moved, index), Slot::At))
}

/// Every card at `(position + 1) * STEP`, with the moved card at `index`.
fn renumber(column: &[(i64, i64)], moved: i64, index: usize) -> Slot {
    let mut cards: Vec<(i64, Option<i64>)> = column.iter().map(|(id, n)| (*id, Some(*n))).collect();
    cards.insert(index, (moved, None));
    let mut number = 0_i64;
    Slot::Renumber(
        cards
            .into_iter()
            .filter_map(|(id, old)| {
                number = number.saturating_add(STEP);
                (old != Some(number)).then_some((id, number))
            })
            .collect(),
    )
}

/// Serializes moves in one organisation's tasks (or projects) for the rest
/// of the transaction. Reads after it see every earlier move committed. A
/// wait of more than 5 seconds, or a statement of more than 10, fails with
/// an error [`is_busy`] spots, rather than holding a connection.
///
/// # Errors
/// On database errors, including those timeouts.
pub async fn lock<C: ConnectionTrait>(txn: &C, what: &str, org_id: i64) -> ModelResult<()> {
    txn.execute_unprepared("SET LOCAL lock_timeout = '5s'")
        .await?;
    txn.execute_unprepared("SET LOCAL statement_timeout = '10s'")
        .await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [format!("{what}-order:{org_id}").into()],
    ))
    .await?;
    Ok(())
}

/// Whether `err` is a lock or statement timeout (SQLSTATE `55P03` or
/// `57014`): someone else's move held the lock too long.
#[must_use]
pub fn is_busy(err: &ModelError) -> bool {
    let ModelError::DbErr(DbErr::Exec(runtime) | DbErr::Query(runtime)) = err else {
        return false;
    };
    let RuntimeErr::SqlxError(err) = runtime else {
        return false;
    };
    match err.as_ref() {
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|code| code == "55P03" || code == "57014"),
        _ => false,
    }
}

/// Writes a renumber in one statement, for rows still in the column it was
/// worked out for: `column` is SQL on `t` (e.g. `t.status = $4`), with its
/// values from `$4` on. A row that left the column meanwhile keeps its new
/// place.
///
/// # Errors
/// On database errors.
pub async fn write_renumber<C: ConnectionTrait>(
    db: &C,
    table: &str,
    org_id: i64,
    rows: &[(i64, i64)],
    column: &str,
    values: Vec<sea_orm::Value>,
) -> ModelResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let orders: Vec<i64> = rows.iter().map(|(_, n)| *n).collect();
    let sql = format!(
        "UPDATE {table} t SET sort_order = v.s \
           FROM unnest($1::bigint[], $2::bigint[]) AS v(id, s) \
          WHERE t.id = v.id AND t.organisation_id = $3 AND {column}"
    );
    let mut all: Vec<sea_orm::Value> = vec![ids.into(), orders.into(), org_id.into()];
    all.extend(values);
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        all,
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COL: [(i64, i64); 3] = [(10, 1024), (11, 2048), (12, 2050)];

    #[test]
    fn ends_and_empty() {
        assert_eq!(slot(&COL, 9, Place::Before(10)), Some(Slot::At(0)));
        assert_eq!(slot(&COL, 9, Place::After(12)), Some(Slot::At(2050 + STEP)));
        assert_eq!(slot(&COL, 9, Place::End), Some(Slot::At(2050 + STEP)));
        assert_eq!(slot(&[], 9, Place::End), Some(Slot::At(STEP)));
    }

    #[test]
    fn midpoint_when_there_is_room() {
        assert_eq!(slot(&COL, 9, Place::After(10)), Some(Slot::At(1536)));
        assert_eq!(slot(&COL, 9, Place::Before(12)), Some(Slot::At(2049)));
    }

    #[test]
    fn renumbers_when_there_is_none() {
        let tight = [(10, 1), (11, 2), (12, 3)];
        assert_eq!(
            slot(&tight, 9, Place::Before(11)),
            Some(Slot::Renumber(vec![
                (10, 1024),
                (9, 2048),
                (11, 3072),
                (12, 4096)
            ]))
        );
        // A tie left by two creates at once.
        let tie = [(10, 5), (11, 5)];
        assert_eq!(
            slot(&tie, 9, Place::After(10)),
            Some(Slot::Renumber(vec![(10, 1024), (9, 2048), (11, 3072)]))
        );
        // Rows already on their number aren't written.
        let spaced = [(10, 1024), (11, 2048), (12, 2049)];
        assert_eq!(
            slot(&spaced, 9, Place::After(11)),
            Some(Slot::Renumber(vec![(9, 3072), (12, 4096)]))
        );
    }

    #[test]
    fn an_anchor_not_in_the_column() {
        assert_eq!(slot(&COL, 9, Place::Before(99)), None);
        assert_eq!(slot(&COL, 9, Place::After(99)), None);
        assert_eq!(slot(&[], 9, Place::Before(10)), None);
    }

    #[test]
    fn overflow_falls_back_to_a_renumber() {
        let high = [(10, i64::MAX - 1)];
        assert_eq!(
            slot(&high, 9, Place::End),
            Some(Slot::Renumber(vec![(10, 1024), (9, 2048)]))
        );
        let low = [(10, i64::MIN + 1)];
        assert_eq!(
            slot(&low, 9, Place::Before(10)),
            Some(Slot::Renumber(vec![(9, 1024), (10, 2048)]))
        );
        let wide = [(10, i64::MIN + 1), (11, i64::MAX - 1)];
        assert!(matches!(
            slot(&wide, 9, Place::After(10)),
            Some(Slot::Renumber(_))
        ));
    }

    /// Moving into the same gap again and again never gives two cards the
    /// same number.
    #[test]
    fn a_long_run_of_moves_stays_distinct() {
        let mut column: Vec<(i64, i64)> = vec![(1, 1024), (2, 2048)];
        for moved in 100..200 {
            let anchor = column[1].0;
            match slot(&column, moved, Place::Before(anchor)).unwrap() {
                Slot::At(n) => column.insert(1, (moved, n)),
                Slot::Renumber(rows) => {
                    column.insert(1, (moved, 0));
                    for (id, n) in rows {
                        column.iter_mut().find(|c| c.0 == id).unwrap().1 = n;
                    }
                }
            }
            let mut numbers: Vec<i64> = column.iter().map(|c| c.1).collect();
            let sorted = numbers.windows(2).all(|w| w[0] < w[1]);
            assert!(sorted, "in order and distinct after {moved}: {column:?}");
            numbers.dedup();
            assert_eq!(numbers.len(), column.len());
        }
    }
}
