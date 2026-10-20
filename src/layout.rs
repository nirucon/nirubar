//! Priority-aware placement for the right side of the bar.
//!
//! Workspace cells keep their configured width. Everything to their right is
//! optional: when the bar is narrow, low-priority modules are removed and an
//! overflow indicator keeps their summaries reachable. The layout is pure so
//! the 800/1366/1920 and multi-output cases can be tested without X.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    /// Caller's index, used to find the text and module again.
    pub(crate) index: usize,
    pub(crate) width: i16,
    pub(crate) priority: i32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub(crate) index: usize,
    pub(crate) x: i16,
    pub(crate) width: i16,
}

#[derive(Clone, Debug)]
pub(crate) struct Fit {
    /// Visible modules, left to right, with their actual hit boxes.
    pub(crate) visible: Vec<Placement>,
    /// Indexes into `items`, in configuration order.
    pub(crate) hidden: Vec<usize>,
    /// Overflow indicator hit box when at least one item is hidden.
    pub(crate) overflow: Option<(i16, i16)>,
}

/// Fit optional items into `left .. right`, preserving their order.
///
/// Each item consumes its text width plus two gaps. The overflow indicator is
/// reserved on the right. Items are removed lowest-priority first; ties remove
/// the later configured item so the leftmost reading order is stable.
pub(crate) fn fit(left: i16, right: i16, gap: i16, overflow_width: i16, items: &[Item]) -> Fit {
    let available = (right - left).max(0);
    let cost = |width: i16| width.max(0) + gap.max(0) * 2;
    let total: i16 = items.iter().map(|item| cost(item.width)).sum();
    if total <= available {
        let visible = (0..items.len()).collect::<Vec<_>>();
        return Fit {
            visible: place(&visible, items, right, gap),
            hidden: Vec::new(),
            overflow: None,
        };
    }

    let reserve = cost(overflow_width);
    let mut visible: Vec<usize> = (0..items.len()).collect();
    let mut hidden = Vec::new();
    loop {
        let used: i16 = visible.iter().map(|slot| cost(items[*slot].width)).sum();
        if used + reserve <= available {
            break;
        }
        if visible.is_empty() {
            return Fit {
                visible: Vec::new(),
                hidden: (0..items.len()).collect(),
                overflow: None,
            };
        }
        // Lowest priority goes first; for a tie, the later item goes first.
        let drop = visible
            .iter()
            .enumerate()
            .min_by_key(|(_, slot)| (items[**slot].priority, std::cmp::Reverse(**slot)))
            .map(|(position, _)| position)
            .expect("visible is not empty");
        hidden.push(visible.remove(drop));
    }
    hidden.sort_unstable();
    let modules_right = right - reserve;
    let visible = place(&visible, items, modules_right, gap);
    let overflow = if hidden.is_empty() {
        None
    } else {
        Some((right - overflow_width.max(0), overflow_width.max(0)))
    };
    Fit {
        visible,
        hidden,
        overflow,
    }
}

fn place(slots: &[usize], items: &[Item], right: i16, gap: i16) -> Vec<Placement> {
    let mut x = right;
    let mut placements = Vec::with_capacity(slots.len());
    for slot in slots.iter().rev() {
        let item = &items[*slot];
        x -= item.width.max(0);
        placements.push(Placement {
            index: item.index,
            x,
            width: item.width.max(0),
        });
        x -= gap.max(0) * 2;
    }
    placements.reverse();
    placements
}

#[cfg(test)]
mod tests {
    use super::{fit, Item};

    fn items() -> Vec<Item> {
        vec![
            Item {
                index: 0,
                width: 90,
                priority: 90,
            },
            Item {
                index: 1,
                width: 60,
                priority: 40,
            },
            Item {
                index: 2,
                width: 110,
                priority: 70,
            },
            Item {
                index: 3,
                width: 70,
                priority: 30,
            },
            Item {
                index: 4,
                width: 120,
                priority: 100,
            },
            Item {
                index: 5,
                width: 80,
                priority: 60,
            },
        ]
    }

    #[test]
    fn widths_800_1366_1920_keep_workspaces_stable() {
        for (width, expected_hidden) in [(800, true), (1366, false), (1920, false)] {
            let workspace = 270;
            let fit = fit(workspace, width - 10, 10, 30, &items());
            assert_eq!(!fit.hidden.is_empty(), expected_hidden, "width {width}");
            for placement in &fit.visible {
                assert!(
                    placement.x >= workspace,
                    "module {} overlaps workspace cells at {width}",
                    placement.index
                );
                assert!(placement.x + placement.width <= width);
            }
            if expected_hidden {
                assert!(fit.overflow.is_some());
            }
        }
    }

    #[test]
    fn hitboxes_do_not_overlap_and_overflow_is_reachable() {
        let fit = fit(270, 790, 10, 30, &items());
        let mut previous_end = 270;
        for placement in &fit.visible {
            assert!(placement.x >= previous_end, "overlapping hitboxes");
            previous_end = placement.x + placement.width;
        }
        let (x, width) = fit.overflow.expect("narrow layout needs overflow");
        assert!(x >= previous_end);
        assert_eq!(x + width, 790);
        assert!(!fit.hidden.is_empty());
    }

    #[test]
    fn two_outputs_choose_independently() {
        let narrow = fit(270, 790, 10, 30, &items());
        let wide = fit(270, 1910, 10, 30, &items());
        assert!(!narrow.hidden.is_empty());
        assert!(wide.hidden.is_empty());
        assert!(wide.visible.len() > narrow.visible.len());
    }

    #[test]
    fn highest_priority_is_kept_longest() {
        let fit = fit(0, 200, 10, 30, &items());
        let visible: Vec<usize> = fit.visible.iter().map(|p| p.index).collect();
        assert!(
            visible.contains(&4),
            "clock/highest priority stayed visible"
        );
        assert!(!visible.contains(&3), "lowest priority was hidden first");
    }

    #[test]
    fn sparse_module_indexes_keep_the_clock_visible() {
        // Empty modules are omitted before layout, so indexes can have gaps.
        // The old by-index table dropped anything beyond the item count.
        let sparse = vec![
            Item {
                index: 0,
                width: 50,
                priority: 70,
            },
            Item {
                index: 3,
                width: 50,
                priority: 85,
            },
            Item {
                index: 5,
                width: 60,
                priority: 90,
            },
            Item {
                index: 6,
                width: 80,
                priority: 100,
            },
        ];
        let fit = fit(270, 1910, 10, 30, &sparse);
        let visible: Vec<usize> = fit.visible.iter().map(|p| p.index).collect();
        assert!(visible.contains(&6), "clock index 6 must be placed");
        assert_eq!(fit.visible.len(), 4);
    }
}
