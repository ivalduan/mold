//! Pose table view model, headless (every OS): sort, filter, row ⇄ PoseId,
//! Selection ⇄ rows, clicks, virtualization window.

use mold_mol::pose_columns as col;
use mold_mol::{PoseId, Selection};
use mold_ui_panels::table::{
    CellStyle, ClickKind, Filter, FilterError, PoseTableModel, Predicate, RowLookup, Sort,
    TableError, scroll_target, visible_rows,
};
use polars::prelude::{Column, DataFrame};

/// Six poses with ties, nulls and a NaN, in a deliberately unsorted order.
fn small() -> PoseTableModel {
    let df = DataFrame::new(
        6,
        vec![
            Column::new(col::POSE_ID.into(), vec![10u32, 11, 12, 13, 14, 15]),
            Column::new(
                col::LIGAND_ID.into(),
                vec![Some("b"), Some("a"), None, Some("c"), Some("a"), Some("B")],
            ),
            Column::new(
                col::PLDDT.into(),
                vec![
                    Some(70.0),
                    None,
                    Some(90.0),
                    Some(70.0),
                    Some(f64::NAN),
                    Some(50.0),
                ],
            ),
            Column::new(
                col::CLASH_COUNT.into(),
                vec![Some(2u32), Some(0), Some(2), None, Some(1), Some(5)],
            ),
            Column::new(
                col::GATE_PASS.into(),
                vec![
                    Some(true),
                    Some(false),
                    Some(true),
                    None,
                    Some(true),
                    Some(false),
                ],
            ),
        ],
    )
    .unwrap();
    PoseTableModel::from_frame(&df).unwrap()
}

fn poses(m: &PoseTableModel) -> Vec<u32> {
    (0..m.row_count())
        .map(|r| m.pose_at(r).unwrap().0)
        .collect()
}

fn sel(ids: &[u32]) -> Selection {
    Selection {
        poses: ids.iter().map(|&i| PoseId(i)).collect(),
        atom_mask: None,
    }
}

#[test]
fn only_contract_columns_present_are_shown_in_schema_order() {
    let m = small();
    let names: Vec<&str> = m.columns().iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        [
            col::POSE_ID,
            col::LIGAND_ID,
            col::PLDDT,
            col::CLASH_COUNT,
            col::GATE_PASS
        ]
    );
    assert_eq!(
        poses(&m),
        [10, 11, 12, 13, 14, 15],
        "table order by default"
    );
}

#[test]
fn sort_by_float_is_stable_with_missing_values_last_both_ways() {
    let mut m = small();
    m.sort_by(col::PLDDT, false);
    // 50, 70 (pose 10), 70 (pose 13, tie keeps table order), 90, then null and NaN
    // in table order.
    assert_eq!(poses(&m), [15, 10, 13, 12, 11, 14]);
    m.sort_by(col::PLDDT, true);
    assert_eq!(poses(&m), [12, 10, 13, 15, 11, 14]);
}

#[test]
fn sort_by_uint_bool_and_string() {
    let mut m = small();
    m.sort_by(col::CLASH_COUNT, false);
    assert_eq!(poses(&m), [11, 14, 10, 12, 15, 13]);
    m.sort_by(col::CLASH_COUNT, true);
    assert_eq!(poses(&m), [15, 10, 12, 14, 11, 13]);

    m.sort_by(col::GATE_PASS, false); // fail < pass, null last
    assert_eq!(poses(&m), [11, 15, 10, 12, 14, 13]);
    m.sort_by(col::GATE_PASS, true);
    assert_eq!(poses(&m), [10, 12, 14, 11, 15, 13]);

    m.sort_by(col::LIGAND_ID, false); // byte order: "B" < "a" < "b" < "c"
    assert_eq!(poses(&m), [15, 11, 14, 10, 13, 12]);
    m.sort_by(col::POSE_ID, true);
    assert_eq!(poses(&m), [15, 14, 13, 12, 11, 10]);

    m.set_sort(None);
    assert_eq!(poses(&m), [10, 11, 12, 13, 14, 15]);
    // A column index out of range means table order, not a panic.
    m.set_sort(Some(Sort {
        column: 99,
        descending: false,
    }));
    assert_eq!(m.sort(), None);
}

#[test]
fn filter_gate_pass_and_numeric_ranges() {
    let mut m = small();
    m.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap();
    assert_eq!(poses(&m), [10, 12, 14], "null gate_pass is not a pass");

    // NaN and null fail a range; bounds are inclusive.
    m.set_filter(Filter::none().and(Predicate::between(col::PLDDT, Some(50.0), Some(70.0))))
        .unwrap();
    assert_eq!(poses(&m), [10, 13, 15]);

    m.set_filter(
        Filter::none()
            .and(Predicate::at_most(col::CLASH_COUNT, 2.0))
            .and(Predicate::is(col::GATE_PASS, true)),
    )
    .unwrap();
    assert_eq!(poses(&m), [10, 12, 14]);

    m.set_filter(Filter::none().and(Predicate::contains(col::LIGAND_ID, "b")))
        .unwrap();
    assert_eq!(poses(&m), [10, 15], "case-insensitive");

    m.set_filter(Filter::none()).unwrap();
    assert_eq!(m.row_count(), 6);
}

#[test]
fn bad_filters_are_refused_and_the_old_one_stays() {
    let mut m = small();
    let gate = Filter::none().and(Predicate::is(col::GATE_PASS, true));
    m.set_filter(gate.clone()).unwrap();
    assert_eq!(
        m.set_filter(Filter::none().and(Predicate::is(col::PLDDT, true))),
        Err(FilterError::WrongType {
            column: col::PLDDT.into(),
            ty: mold_mol::pose_table::ColumnType::Float64
        })
    );
    assert_eq!(
        m.set_filter(Filter::none().and(Predicate::at_least(col::IPTM, 0.5))),
        Err(FilterError::UnknownColumn(col::IPTM.into())),
        "iptm is not in this table"
    );
    assert_eq!(m.filter(), &gate);
    assert_eq!(poses(&m), [10, 12, 14]);
}

#[test]
fn sort_and_filter_compose_in_either_order() {
    let mut a = small();
    a.sort_by(col::PLDDT, true);
    a.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap();
    let mut b = small();
    b.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap();
    b.sort_by(col::PLDDT, true);
    assert_eq!(poses(&a), poses(&b));
    assert_eq!(poses(&a), [12, 10, 14]);
}

#[test]
fn row_to_pose_mapping_survives_sort_and_filter() {
    let mut m = small();
    m.sort_by(col::CLASH_COUNT, true);
    m.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap();
    for r in 0..m.row_count() {
        let p = m.pose_at(r).unwrap();
        assert_eq!(m.row_of(p), RowLookup::Visible(r));
        // The cell at that row is the pose's own row in the frame.
        assert_eq!(m.cell(r, 0).text, p.0.to_string());
    }
    assert_eq!(m.pose_at(m.row_count()), None);
    assert_eq!(m.row_of(PoseId(11)), RowLookup::Hidden);
    assert_eq!(m.row_of(PoseId(999)), RowLookup::Absent);
}

#[test]
fn an_incoming_selection_maps_to_visible_rows_and_reports_hidden_ones() {
    let mut m = small();
    m.sort_by(col::PLDDT, true); // 12, 10, 13, 15, 11, 14
    m.set_filter(Filter::none().and(Predicate::at_most(col::CLASH_COUNT, 2.0)))
        .unwrap(); // 12, 10, 11, 14
    let rows = m.map_selection(&sel(&[14, 12, 13, 15, 999]));
    assert_eq!(rows.visible, [0, 3]);
    assert_eq!(rows.hidden, [PoseId(13), PoseId(15)]);
    assert_eq!(rows.absent, [PoseId(999)]);
    assert_eq!(
        m.selection_of_rows(rows.visible.iter().copied()),
        sel(&[12, 14])
    );
    // Empty selection, empty result.
    assert_eq!(m.map_selection(&Selection::default()), Default::default());
}

#[test]
fn clicks_replace_toggle_and_extend() {
    let mut m = small();
    m.sort_by(col::POSE_ID, false);
    m.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap(); // 10, 12, 14
    let (s, a) = m
        .click(&Selection::default(), None, 1, ClickKind::Replace)
        .unwrap();
    assert_eq!((s.clone(), a), (sel(&[12]), PoseId(12)));
    let (s, a) = m.click(&s, Some(a), 2, ClickKind::Extend).unwrap();
    assert_eq!((s.clone(), a), (sel(&[12, 14]), PoseId(12)), "anchor stays");
    // Toggle keeps poses the filter hides (11 here) and removes a clicked one.
    let with_hidden = sel(&[11, 12, 14]);
    let (s, a) = m
        .click(&with_hidden, Some(a), 2, ClickKind::Toggle)
        .unwrap();
    assert_eq!((s.clone(), a), (sel(&[11, 12]), PoseId(14)));
    let (s, _) = m.click(&s, None, 0, ClickKind::Toggle).unwrap();
    assert_eq!(s, sel(&[10, 11, 12]));
    // Extend without a visible anchor acts as a plain click.
    let (s, _) = m.click(&s, Some(PoseId(11)), 0, ClickKind::Extend).unwrap();
    assert_eq!(s, sel(&[10]));
    assert!(m.click(&s, None, 3, ClickKind::Replace).is_none());
}

#[test]
fn cells_are_formatted_per_column_and_window_is_clamped() {
    let m = small();
    let gate = m.table().column_index(col::GATE_PASS).unwrap();
    let plddt = m.table().column_index(col::PLDDT).unwrap();
    assert_eq!(m.cell(0, gate).style, CellStyle::Pass);
    assert_eq!(m.cell(1, gate).style, CellStyle::Fail);
    assert_eq!(m.cell(3, gate).style, CellStyle::Null);
    assert_eq!(m.cell(0, plddt).text, "70.0");
    assert_eq!(
        m.cell(4, plddt).style,
        CellStyle::Null,
        "NaN shows as missing"
    );
    let w = m.window(4..100);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].len(), m.columns().len());
    assert!(m.window(10..20).is_empty());
}

#[test]
fn virtualization_window_and_scroll_target() {
    // 24 px rows, 240 px viewport scrolled to 100 px: rows 4..15, ±2 overscan.
    assert_eq!(visible_rows(100.0, 240.0, 24.0, 10_000, 2), 2..17);
    assert_eq!(visible_rows(0.0, 240.0, 24.0, 5, 2), 0..5);
    assert_eq!(visible_rows(0.0, 240.0, 24.0, 0, 2), 0..0);
    // A selected row already on screen: don't scroll.
    assert_eq!(scroll_target(&[3, 500], 0..20), None);
    // None on screen: scroll to the first.
    assert_eq!(scroll_target(&[500, 40], 0..20), Some(40));
    assert_eq!(scroll_target(&[], 0..20), None);
}

#[test]
fn malformed_tables_are_refused() {
    let no_id = DataFrame::new(1, vec![Column::new(col::PLDDT.into(), vec![1.0f64])]).unwrap();
    assert_eq!(
        PoseTableModel::from_frame(&no_id).unwrap_err(),
        TableError::MissingPoseId
    );
    let dup = DataFrame::new(2, vec![Column::new(col::POSE_ID.into(), vec![1u32, 1])]).unwrap();
    assert_eq!(
        PoseTableModel::from_frame(&dup).unwrap_err(),
        TableError::DuplicatePoseId(PoseId(1))
    );
    let null = DataFrame::new(
        2,
        vec![Column::new(col::POSE_ID.into(), vec![Some(1u32), None])],
    )
    .unwrap();
    assert_eq!(
        PoseTableModel::from_frame(&null).unwrap_err(),
        TableError::NullPoseId { row: 1 }
    );
    let wrong = DataFrame::new(
        1,
        vec![
            Column::new(col::POSE_ID.into(), vec![1u32]),
            Column::new(col::PLDDT.into(), vec!["high"]),
        ],
    )
    .unwrap();
    assert!(matches!(
        PoseTableModel::from_frame(&wrong).unwrap_err(),
        TableError::WrongType {
            column: col::PLDDT,
            ..
        }
    ));
    // The empty contract table is fine.
    let empty = PoseTableModel::from_frame(&mold_mol::pose_table::empty()).unwrap();
    assert_eq!((empty.row_count(), empty.columns().len()), (0, 13));
}
