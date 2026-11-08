//! Order-preserving map that runs on the current rayon pool or, when asked, on the calling thread alone.

use rayon::prelude::*;

/// Applies `f` to every item and returns the results in item order.
///
/// With `sequential` the items are mapped on the calling thread and rayon is not touched, so no pool is created.
/// Otherwise the map runs on the current rayon pool: the global pool, or the one a surrounding
/// `ThreadPool::install` selected.
pub fn map_ordered<T, U, F>(items: &[T], sequential: bool, f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync + Send,
{
    if sequential {
        items.iter().map(f).collect()
    } else {
        items.par_iter().map(f).collect()
    }
}

/// Runs `a` and `b` and returns both results: side by side on the current rayon pool, or one after the other on the
/// calling thread with `sequential`, as [`map_ordered`] does.
pub fn join<A, B, RA, RB>(sequential: bool, a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    if sequential { (a(), b()) } else { rayon::join(a, b) }
}

#[cfg(test)]
mod tests {
    use super::{join, map_ordered};

    #[test]
    fn keeps_item_order_in_both_modes() {
        let items: Vec<u32> = (0..64).collect();
        let expected: Vec<u32> = items.iter().map(|n| n * 2).collect();
        assert_eq!(map_ordered(&items, true, |n| n * 2), expected);
        assert_eq!(map_ordered(&items, false, |n| n * 2), expected);
    }

    #[test]
    fn join_returns_both_results_in_both_modes() {
        assert_eq!(join(true, || 1, || "two"), (1, "two"));
        assert_eq!(join(false, || 1, || "two"), (1, "two"));
    }
}
