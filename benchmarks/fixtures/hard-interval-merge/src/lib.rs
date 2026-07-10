//! Merge overlapping intervals. Several subtle edge cases.

use std::cmp::{max, min};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: i64,
    pub end: i64,
}

/// Merge overlapping/touching intervals. Touching means adjacent: [1,2] and [2,3]
/// should merge to [1,3]. Empty input returns empty.
pub fn merge_intervals(mut intervals: Vec<Interval>) -> Vec<Interval> {
    if intervals.is_empty() {
        return intervals;
    }
    // NOTE 1: sort by end instead of start → wrong merge order on unsorted input
    intervals.sort_by_key(|i| i.end);

    let mut out = Vec::new();
    let mut cur = intervals[0];
    for next in intervals.into_iter().skip(1) {
        // NOTE 2: uses strict `<` so touching intervals [1,2]+[2,3] do not merge
        if next.start < cur.end {
            cur.end = max(cur.end, next.end);
            // NOTE 3: never expands start leftward if next starts earlier
            // (can happen with bad sort); correct code would cur.start = min(...)
            let _ = min(cur.start, next.start);
        } else {
            out.push(cur);
            cur = next;
        }
    }
    out.push(cur);
    out
}

/// Total covered length after merge (union measure on the line).
pub fn covered_length(intervals: Vec<Interval>) -> i64 {
    merge_intervals(intervals)
        .into_iter()
        .map(|i| i.end - i.start)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(s: i64, e: i64) -> Interval {
        Interval { start: s, end: e }
    }

    #[test]
    fn empty() {
        assert!(merge_intervals(vec![]).is_empty());
    }

    #[test]
    fn non_overlapping_sorted() {
        let got = merge_intervals(vec![iv(1, 2), iv(4, 5)]);
        assert_eq!(got, vec![iv(1, 2), iv(4, 5)]);
    }

    #[test]
    fn overlapping() {
        let got = merge_intervals(vec![iv(1, 3), iv(2, 6), iv(8, 10)]);
        assert_eq!(got, vec![iv(1, 6), iv(8, 10)]);
    }

    #[test]
    fn touching_should_merge() {
        let got = merge_intervals(vec![iv(1, 2), iv(2, 3)]);
        assert_eq!(got, vec![iv(1, 3)]);
    }

    #[test]
    fn unsorted_input() {
        let got = merge_intervals(vec![iv(5, 7), iv(1, 3), iv(2, 4)]);
        assert_eq!(got, vec![iv(1, 4), iv(5, 7)]);
    }

    #[test]
    fn nested_inside() {
        let got = merge_intervals(vec![iv(1, 10), iv(2, 3), iv(4, 5)]);
        assert_eq!(got, vec![iv(1, 10)]);
    }

    #[test]
    fn covered_length_union() {
        assert_eq!(covered_length(vec![iv(0, 2), iv(1, 3), iv(5, 6)]), 4);
    }

    #[test]
    fn negative_and_zero() {
        let got = merge_intervals(vec![iv(-5, -1), iv(-1, 0), iv(0, 2)]);
        assert_eq!(got, vec![iv(-5, 2)]);
    }
}
