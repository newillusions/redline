//! Count groups (docs/count-polylength.md). A count is the set of `MeasurementCount` items
//! sharing one `Markup::count_group`; an item with no group is a count of one. This module
//! holds the backend side: bounded loading, group totals, and export collapsing. The
//! interactive operations (resume, split, merge) live in the frontend
//! (`src/lib/count-groups.ts`) because they are undoable edits of the markup store.

use std::collections::HashMap;

use uuid::Uuid;

use crate::markup::{Markup, MarkupType, MAX_COUNT_GROUP_ITEMS};

fn is_count(m: &Markup) -> bool {
    matches!(m.markup_type, MarkupType::MeasurementCount)
}

/// Bound untrusted input: items past `MAX_COUNT_GROUP_ITEMS` in one group (in document
/// order) are treated as ungrouped, so aggregation work per group stays bounded. Also
/// drops a `count_group` from anything that is not a count item.
pub fn cap_group_sizes(markups: &mut [Markup]) {
    let mut seen: HashMap<Uuid, usize> = HashMap::new();
    for m in markups.iter_mut() {
        let Some(g) = m.count_group else { continue };
        if !is_count(m) {
            m.count_group = None;
            continue;
        }
        let n = seen.entry(g).or_insert(0);
        *n += 1;
        if *n > MAX_COUNT_GROUP_ITEMS {
            m.count_group = None;
        }
    }
}

/// Total of `count_value` per group (ungrouped items are not included).
pub fn group_totals(markups: &[Markup]) -> HashMap<Uuid, u64> {
    let mut totals: HashMap<Uuid, u64> = HashMap::new();
    for m in markups.iter().filter(|m| is_count(m)) {
        if let Some(g) = m.count_group {
            let v = m
                .measurement
                .as_ref()
                .and_then(|x| x.count_value)
                .unwrap_or(1);
            *totals.entry(g).or_insert(0) += u64::from(v);
        }
    }
    totals
}

fn with_total(m: &Markup, total: u64) -> Markup {
    let mut row = m.clone();
    if let Some(meas) = row.measurement.as_mut() {
        let t = total.min(u64::from(u32::MAX)) as u32;
        meas.count_value = Some(t);
        meas.raw_measure = f64::from(t);
        meas.computed_quantity = f64::from(t);
    }
    row
}

/// One row per count group: the first item of each group stands in for the whole group
/// with its total; ungrouped items and every non-count markup pass through unchanged.
/// Order of first appearance is preserved.
pub fn collapse_groups(markups: &[Markup]) -> Vec<Markup> {
    let totals = group_totals(markups);
    let mut emitted: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(markups.len());
    for m in markups {
        match (is_count(m), m.count_group) {
            (true, Some(g)) => {
                if emitted.insert(g) {
                    out.push(with_total(m, totals.get(&g).copied().unwrap_or(1)));
                }
            }
            _ => out.push(m.clone()),
        }
    }
    out
}

/// The picked rows with each grouped count item carrying its WHOLE group's total (looked
/// up in `all`), for views that list a chosen subset of rows.
pub fn with_group_totals(all: &[Markup], picked: &[&Markup]) -> Vec<Markup> {
    let totals = group_totals(all);
    picked
        .iter()
        .map(|m| match (is_count(m), m.count_group) {
            (true, Some(g)) => with_total(m, totals.get(&g).copied().unwrap_or(1)),
            _ => (*m).clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, MarkupGeometry, Measurement, UserRef};

    fn item(group: Option<Uuid>) -> Markup {
        let mut m = Markup::new(
            MarkupType::MeasurementCount,
            0,
            MarkupGeometry::Point(PdfPoint { x: 1.0, y: 2.0 }),
            Appearance::default(),
            UserRef {
                user_id: Uuid::new_v4(),
                display_name: "T".into(),
            },
        );
        m.count_group = group;
        m.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "ea".into(),
            computed_quantity: 1.0,
            depth: None,
            count_value: Some(1),
            custom_columns: Default::default(),
            rise_drop: Vec::new(),
        });
        m
    }

    #[test]
    fn totals_sum_per_group_and_skip_ungrouped() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let ms = vec![item(Some(a)), item(Some(a)), item(Some(b)), item(None)];
        let t = group_totals(&ms);
        assert_eq!(t[&a], 2);
        assert_eq!(t[&b], 1);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn collapse_emits_one_row_per_group_with_the_total() {
        let a = Uuid::new_v4();
        let ms = vec![item(Some(a)), item(None), item(Some(a)), item(Some(a))];
        let rows = collapse_groups(&ms);
        assert_eq!(rows.len(), 2, "one group row + one ungrouped item");
        assert_eq!(rows[0].measurement.as_ref().unwrap().count_value, Some(3));
        assert_eq!(rows[0].measurement.as_ref().unwrap().computed_quantity, 3.0);
        assert_eq!(rows[1].measurement.as_ref().unwrap().count_value, Some(1));
    }

    #[test]
    fn with_group_totals_uses_the_whole_group_not_the_picked_subset() {
        let a = Uuid::new_v4();
        let ms = vec![item(Some(a)), item(Some(a)), item(Some(a))];
        let picked: Vec<&Markup> = vec![&ms[0]];
        let rows = with_group_totals(&ms, &picked);
        assert_eq!(rows[0].measurement.as_ref().unwrap().count_value, Some(3));
    }

    #[test]
    fn group_cap_ungroups_overflow_and_non_count_items() {
        let a = Uuid::new_v4();
        let mut ms: Vec<Markup> = (0..MAX_COUNT_GROUP_ITEMS + 5)
            .map(|_| item(Some(a)))
            .collect();
        let mut rect = item(Some(a));
        rect.markup_type = MarkupType::Rectangle;
        ms.push(rect);
        cap_group_sizes(&mut ms);
        let grouped = ms.iter().filter(|m| m.count_group.is_some()).count();
        assert_eq!(grouped, MAX_COUNT_GROUP_ITEMS);
        assert!(ms.last().unwrap().count_group.is_none());
    }

    #[test]
    fn hostile_count_value_cannot_overflow_a_total() {
        let a = Uuid::new_v4();
        let mut x = item(Some(a));
        let mut y = item(Some(a));
        x.measurement.as_mut().unwrap().count_value = Some(u32::MAX);
        y.measurement.as_mut().unwrap().count_value = Some(u32::MAX);
        let rows = collapse_groups(&[x, y]);
        assert_eq!(
            rows[0].measurement.as_ref().unwrap().count_value,
            Some(u32::MAX)
        );
    }
}
