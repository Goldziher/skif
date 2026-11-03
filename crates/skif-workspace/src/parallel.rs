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

#[cfg(test)]
mod tests {
    use super::map_ordered;

    #[test]
    fn keeps_item_order_in_both_modes() {
        let items: Vec<u32> = (0..64).collect();
        let expected: Vec<u32> = items.iter().map(|n| n * 2).collect();
        assert_eq!(map_ordered(&items, true, |n| n * 2), expected);
        assert_eq!(map_ordered(&items, false, |n| n * 2), expected);
    }
}
