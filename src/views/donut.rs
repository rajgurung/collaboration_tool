//! Home's progress donut: the tasks in the open "now" projects by status,
//! drawn on the server as SVG circles. Pure maths, no database.
use std::f64::consts::PI;

use serde::Serialize;

const RADIUS: f64 = 50.0;
/// The space between two arcs, in the card's colour.
const GAP: f64 = 2.0;
/// The shortest arc drawn, so one task among hundreds still shows.
const MIN_DASH: f64 = 0.5;

/// One status's arc: `dash` long, then a gap of the whole ring so it never
/// repeats, starting `offset` along the ring from 12 o'clock.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Arc {
    pub key: &'static str,
    pub dash: String,
    pub gap: String,
    pub offset: String,
}

/// A legend row. Every status has one, with 0 when it has no tasks.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Slice {
    pub key: &'static str,
    pub label: &'static str,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Donut {
    pub total: usize,
    pub done: usize,
    /// Rounded down, like a project's progress, so 100% means all done.
    pub pct: usize,
    pub arcs: Vec<Arc>,
    pub legend: Vec<Slice>,
    /// What a screen reader says for the picture.
    pub label: String,
}

fn fixed(n: f64) -> String {
    format!("{n:.2}")
}

/// The donut for these counts, clockwise from 12: done, in progress,
/// blocked, to do. `None` with no tasks, when Home shows no donut.
#[must_use]
pub fn of(done: usize, progress: usize, blocked: usize, todo: usize) -> Option<Donut> {
    let total = done + progress + blocked + todo;
    if total == 0 {
        return None;
    }
    let legend = vec![
        Slice {
            key: "done",
            label: "Done",
            count: done,
        },
        Slice {
            key: "progress",
            label: "In progress",
            count: progress,
        },
        Slice {
            key: "blocked",
            label: "Blocked",
            count: blocked,
        },
        Slice {
            key: "todo",
            label: "To do",
            count: todo,
        },
    ];
    let ring = 2.0 * PI * RADIUS;
    let drawn: Vec<&Slice> = legend.iter().filter(|s| s.count > 0).collect();
    let mut before = 0.0;
    let arcs = drawn
        .iter()
        .map(|s| {
            #[allow(clippy::cast_precision_loss)]
            let len = s.count as f64 / total as f64 * ring;
            let dash = if drawn.len() > 1 {
                (len - GAP).max(MIN_DASH)
            } else {
                ring
            };
            // `0.0 - 0.0` is 0, where `-0.0` would print as "-0.00".
            let arc = Arc {
                key: s.key,
                dash: fixed(dash),
                gap: fixed(ring),
                offset: fixed(0.0 - before),
            };
            before += len;
            arc
        })
        .collect();
    let pct = done * 100 / total;
    Some(Donut {
        total,
        done,
        pct,
        arcs,
        label: format!(
            "Now: {pct}% done. {done} done, {progress} in progress, {blocked} blocked, {todo} to do."
        ),
        legend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_is_62_percent() {
        let d = of(31, 9, 3, 7).unwrap();
        assert_eq!((d.total, d.done, d.pct), (50, 31, 62));
        let offsets: Vec<&str> = d.arcs.iter().map(|a| a.offset.as_str()).collect();
        assert_eq!(offsets, ["0.00", "-194.78", "-251.33", "-270.18"]);
        let dashes: Vec<&str> = d.arcs.iter().map(|a| a.dash.as_str()).collect();
        assert_eq!(dashes, ["192.78", "54.55", "16.85", "41.98"]);
        assert!(d.arcs.iter().all(|a| a.gap == "314.16"));
        let keys: Vec<&str> = d.arcs.iter().map(|a| a.key).collect();
        assert_eq!(keys, ["done", "progress", "blocked", "todo"]);
    }

    #[test]
    fn no_tasks_is_no_donut() {
        assert_eq!(of(0, 0, 0, 0), None);
    }

    #[test]
    fn all_done_is_one_full_ring() {
        let d = of(19, 0, 0, 0).unwrap();
        assert_eq!(d.pct, 100);
        assert_eq!(d.arcs.len(), 1);
        assert_eq!(d.arcs[0].dash, "314.16", "the whole ring, no gap");
        assert_eq!(d.arcs[0].offset, "0.00");
        let zeros = d.legend.iter().filter(|s| s.count == 0).count();
        assert_eq!(zeros, 3);
    }

    #[test]
    fn rounds_down() {
        assert_eq!(of(199, 1, 0, 0).unwrap().pct, 99);
        assert_eq!(of(1, 2, 0, 0).unwrap().pct, 33);
    }

    #[test]
    fn a_single_task_still_shows() {
        let d = of(0, 0, 1, 499).unwrap();
        assert_eq!(d.arcs[0].key, "blocked");
        assert_eq!(d.arcs[0].dash, "0.50");
    }

    #[test]
    fn legend_keeps_its_shape() {
        for d in [of(1, 0, 0, 0), of(0, 0, 0, 1), of(2, 3, 4, 5)] {
            let keys: Vec<&str> = d.unwrap().legend.iter().map(|s| s.key).collect();
            assert_eq!(keys, ["done", "progress", "blocked", "todo"]);
        }
    }

    #[test]
    fn label_reads_well() {
        assert_eq!(
            of(31, 9, 3, 7).unwrap().label,
            "Now: 62% done. 31 done, 9 in progress, 3 blocked, 7 to do."
        );
    }
}
