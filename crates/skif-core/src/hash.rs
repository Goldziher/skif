//! The hash map and set every skif crate uses: ahash under fixed keys.
//!
//! Fixed keys keep iteration order the same in every process, so nothing that leaks map order (a cache key, a
//! tie in a sort) changes between two runs on the same input. skif hashes paths and names read from the user's own
//! repository, so a per-process random seed would buy no protection worth that.

use std::hash::BuildHasher;

/// The keys of every skif hash: arbitrary constants, fixed so a run's hashes never depend on the process.
const STATE: ahash::RandomState = ahash::RandomState::with_seeds(
    0x243f_6a88_85a3_08d3,
    0x1319_8a2e_0370_7344,
    0xa409_3822_299f_31d0,
    0x082e_fa98_ec4e_6c89,
);

/// The hasher of [`HashMap`] and [`HashSet`]: ahash under the fixed keys above.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashState;

impl BuildHasher for HashState {
    type Hasher = ahash::AHasher;

    fn build_hasher(&self) -> Self::Hasher {
        STATE.build_hasher()
    }
}

/// A [`std::collections::HashMap`] hashed with [`HashState`].
pub type HashMap<K, V> = std::collections::HashMap<K, V, HashState>;

/// A [`std::collections::HashSet`] hashed with [`HashState`].
pub type HashSet<T> = std::collections::HashSet<T, HashState>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const PROBE: &str = "SKIF_HASH_PROBE";

    #[test]
    fn hashes_are_the_same_in_every_process() {
        let local = HashState.hash_one("skif");
        if std::env::var_os(PROBE).is_some() {
            println!("probe={local}");
            return;
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "hash::tests::hashes_are_the_same_in_every_process",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let probed = stdout.lines().find_map(|line| line.strip_prefix("probe=")).unwrap();
        assert_eq!(probed, local.to_string());
    }
}
