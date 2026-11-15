//! Workers of rayon's global pool run deep recursion once the CLI has configured the pool.

use std::hint::black_box;

/// Recurses `depth` frames of about 16 KiB each.
fn recurse(depth: usize) -> u8 {
    let frame = black_box([1_u8; 16 << 10]);
    if depth == 0 {
        frame[0]
    } else {
        recurse(depth - 1).wrapping_add(black_box(frame[depth % frame.len()]))
    }
}

#[test]
fn a_global_pool_worker_has_room_for_a_16_mib_recursion() {
    skif_cli::configure_global_pool();
    let (sender, receiver) = std::sync::mpsc::channel();
    rayon::spawn(move || {
        let name = std::thread::current().name().map(ToOwned::to_owned);
        sender
            .send((recurse(1024), name))
            .expect("the test waits for the worker");
    });
    let (value, name) = receiver.recv().expect("the worker finishes its recursion");
    assert_eq!(value, 1);
    assert_eq!(name.as_deref().map(|name| name.starts_with("skif-worker-")), Some(true));
}
