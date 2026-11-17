use std::time::{Duration, SystemTime};

use skif_core::cache::{KeyBuilder, salt_digest, seal};

use super::*;

const MODULES: CacheNamespace = CacheNamespace::new("modules", 1);
const OTHER: CacheNamespace = CacheNamespace::new("other.facts", 1);

fn key(text: &str) -> [u8; 32] {
    KeyBuilder::new(MODULES, &salt_digest(b""))
        .content(text.as_bytes())
        .finish()
}

fn value(namespace: CacheNamespace, text: &str) -> Arc<[u8]> {
    seal(namespace, &key(text), text.as_bytes())
}

fn open_store(location: &Path, base: &Path) -> DiskStore {
    DiskStore::open(location, base, DiskStoreOptions::default())
}

fn file(store: &DiskStore, namespace: CacheNamespace) -> PathBuf {
    store.dir().join(namespace_file(namespace.name))
}

#[test]
fn a_written_store_loads_with_the_same_entries() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let store = open_store(&location, temp.path());
    store.put(MODULES, &key("b"), value(MODULES, "b"));
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert_eq!(store.finish_run().unwrap(), Finish::Written);
    assert!(fs::read(file(&store, MODULES)).unwrap().starts_with(&MAGIC));

    let loaded = open_store(&location, temp.path());
    assert_eq!(loaded.get(MODULES, &key("a")), Some(value(MODULES, "a")));
    assert_eq!(loaded.get(MODULES, &key("b")), Some(value(MODULES, "b")));
    assert!(loaded.get(MODULES, &key("c")).is_none());
}

#[test]
fn created_directories_are_tagged_and_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let store = open_store(&location, temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.finish_run().unwrap();
    for dir in [&location, &store.dir().to_path_buf()] {
        assert!(
            fs::read_to_string(dir.join("CACHEDIR.TAG"))
                .unwrap()
                .starts_with("Signature: 8a477f597d28d172789f06886806bc55")
        );
        assert_eq!(fs::read_to_string(dir.join(".gitignore")).unwrap(), "*\n");
    }
}

#[test]
fn an_existing_location_is_not_tagged() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.finish_run().unwrap();
    assert!(!temp.path().join("CACHEDIR.TAG").exists());
}

#[test]
fn a_missing_node_modules_is_never_created() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("node_modules").join(".cache").join("skif");
    let store = open_store(&location, temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert!(matches!(store.finish_run(), Err(StoreError::NodeModules { .. })));
    assert!(!temp.path().join("node_modules").exists());
    // Below an existing node_modules the store creates its own directories.
    fs::create_dir(temp.path().join("node_modules")).unwrap();
    let store = open_store(&location, temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert_eq!(store.finish_run().unwrap(), Finish::Written);
}

#[test]
fn the_default_location_never_creates_node_modules() {
    let temp = tempfile::tempdir().unwrap();
    let user = temp.path().join("user-cache");
    assert_eq!(default_location(temp.path(), Some(&user)), Some(user.join("skif")));
    assert_eq!(default_location(temp.path(), None), None);
    fs::create_dir(temp.path().join("node_modules")).unwrap();
    assert_eq!(
        default_location(temp.path(), Some(&user)),
        Some(temp.path().join("node_modules").join(".cache").join("skif"))
    );
}

#[test]
fn a_fully_warm_run_does_not_rewrite_the_store() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.finish_run().unwrap();
    let path = file(&store, MODULES);
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();

    let warm = open_store(temp.path(), temp.path());
    assert!(warm.get(MODULES, &key("a")).is_some());
    assert_eq!(warm.finish_run().unwrap(), Finish::Unchanged);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    // A run that used nothing leaves the store alone too.
    assert_eq!(
        open_store(temp.path(), temp.path()).finish_run().unwrap(),
        Finish::Unchanged
    );
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), stamp);
}

#[test]
fn a_write_keeps_exactly_the_entries_the_run_used_per_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let first = open_store(temp.path(), temp.path());
    first.put(MODULES, &key("a"), value(MODULES, "a"));
    first.put(MODULES, &key("b"), value(MODULES, "b"));
    first.put(OTHER, &key("t"), value(OTHER, "t"));
    first.finish_run().unwrap();

    // A run that uses one module and adds one, and never asks for the other namespace.
    let second = open_store(temp.path(), temp.path());
    assert!(second.get(MODULES, &key("a")).is_some());
    second.put(MODULES, &key("c"), value(MODULES, "c"));
    second.finish_run().unwrap();
    // The in-memory state is pruned like the file.
    assert!(second.get(MODULES, &key("b")).is_none());

    let third = open_store(temp.path(), temp.path());
    assert!(third.get(MODULES, &key("a")).is_some());
    assert!(third.get(MODULES, &key("b")).is_none());
    assert!(third.get(MODULES, &key("c")).is_some());
    assert!(
        third.get(OTHER, &key("t")).is_some(),
        "a run pruned a namespace it never asked for"
    );
}

#[test]
fn a_damaged_or_foreign_file_loads_empty() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.put(MODULES, &key("b"), value(MODULES, "b"));
    store.finish_run().unwrap();
    let path = file(&store, MODULES);
    let good = fs::read(&path).unwrap();
    assert!(decode(good.clone(), MODULES).is_ok());

    let records_at = usize::try_from(header_size(MODULES.name)).unwrap();
    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    let mut wrong_format = good.clone();
    wrong_format[MAGIC.len()] = 9;
    let mut long_length = good.clone();
    long_length[records_at + 32..records_at + 36].copy_from_slice(&u32::MAX.to_le_bytes());
    let first_length = u32::from_le_bytes(good[records_at + 32..records_at + 36].try_into().unwrap()) as usize;
    let second_at = records_at + 36 + first_length;
    let mut unsorted = good[..records_at].to_vec();
    unsorted.extend_from_slice(&good[second_at..]);
    unsorted.extend_from_slice(&good[records_at..second_at]);
    let mut trailing = good.clone();
    trailing.push(0);
    for (name, bytes) in [
        ("truncated", good[..good.len() - 1].to_vec()),
        ("empty", Vec::new()),
        ("wrong magic", wrong_magic),
        ("wrong format", wrong_format),
        ("length past the end", long_length),
        ("unsorted", unsorted),
        ("trailing bytes", trailing),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(decode(bytes, MODULES).is_err(), "{name}");
        let loaded = open_store(temp.path(), temp.path());
        assert!(loaded.get(MODULES, &key("a")).is_none(), "{name}");
        assert!(!loaded.take_notes().is_empty(), "{name}");
    }
    // Another schema or namespace.
    fs::write(&path, &good).unwrap();
    assert!(decode(good.clone(), CacheNamespace::new("modules", 2)).is_err());
    assert!(decode(good, OTHER).is_err());
    let other_schema = open_store(temp.path(), temp.path());
    assert!(other_schema.get(CacheNamespace::new("modules", 2), &key("a")).is_none());
}

#[test]
fn an_invalid_namespace_name_is_never_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    let bad = CacheNamespace::new("../escape", 1);
    store.put(bad, &key("a"), value(bad, "a"));
    assert!(store.get(bad, &key("a")).is_none());
    assert_eq!(store.finish_run().unwrap(), Finish::Unchanged);
}

#[test]
fn a_read_only_store_serves_the_run_and_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let store = DiskStore::open(&location, temp.path(), DiskStoreOptions::default().with_write(false));
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert!(store.get(MODULES, &key("a")).is_some());
    assert_eq!(store.finish_run().unwrap(), Finish::ReadOnly);
    assert!(!location.exists());
}

#[test]
fn a_write_is_skipped_while_another_process_holds_the_lock() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.finish_run().unwrap();
    let held = try_lock(store.dir()).unwrap().unwrap();
    let other = open_store(temp.path(), temp.path());
    other.put(MODULES, &key("b"), value(MODULES, "b"));
    assert_eq!(other.finish_run().unwrap(), Finish::Contended);
    drop(held);
    assert!(open_store(temp.path(), temp.path()).get(MODULES, &key("b")).is_none());
}

#[test]
fn a_write_failure_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    // A file where the cache directory should be.
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    let store = open_store(&blocked, temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert!(matches!(store.finish_run(), Err(StoreError::Io { .. })));
}

/// A repository at `top` with a linked worktree at `worktree`, laid out as `git worktree add` leaves them.
fn repository_with_worktree(top: &Path, worktree: &Path) {
    let private = top.join(".git").join("worktrees").join("linked");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    fs::create_dir_all(worktree).unwrap();
    fs::write(worktree.join(".git"), format!("gitdir: {}\n", private.display())).unwrap();
}

#[test]
fn worktrees_of_one_repository_share_a_store_and_two_repositories_never_do() {
    let temp = tempfile::tempdir().unwrap();
    let (one, linked, two) = (
        temp.path().join("one"),
        temp.path().join("linked"),
        temp.path().join("two"),
    );
    repository_with_worktree(&one, &linked);
    fs::create_dir_all(two.join(".git")).unwrap();
    for checkout in [&one, &linked, &two] {
        fs::create_dir_all(checkout.join("app")).unwrap();
    }
    assert_eq!(base_key(&one), base_key(&linked));
    assert_eq!(base_key(&one.join("app")), base_key(&linked.join("app")));
    assert_ne!(base_key(&one.join("app")), base_key(&one));
    // Two repositories linted at their roots, or at the same path below them, are two stores.
    assert_ne!(base_key(&one), base_key(&two));
    assert_ne!(base_key(&one.join("app")), base_key(&two.join("app")));
    let outside = tempfile::tempdir().unwrap();
    assert_ne!(base_key(outside.path()), base_key(&one));

    let location = temp.path().join("cache");
    for (base, text) in [(&one, "a"), (&two, "b")] {
        let store = open_store(&location, base);
        store.put(MODULES, &key(text), value(MODULES, text));
        store.finish_run().unwrap();
    }
    // The second repository's run pruned nothing of the first, and the worktree reads the first's entries.
    assert!(open_store(&location, &linked).get(MODULES, &key("a")).is_some());
    assert!(open_store(&location, &linked).get(MODULES, &key("b")).is_none());
    assert!(open_store(&location, &two).get(MODULES, &key("b")).is_some());
    assert_eq!(stats(&location).unwrap().stores.len(), 2);

    // Cleaning one repository leaves the other's store.
    assert_eq!(clean(&location, Some(&one)).unwrap().stores, 1);
    assert!(open_store(&location, &one).get(MODULES, &key("a")).is_none());
    assert!(open_store(&location, &two).get(MODULES, &key("b")).is_some());
}

#[test]
fn a_failed_run_adds_what_it_computed_and_drops_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let first = open_store(temp.path(), temp.path());
    for text in ["a", "b", "c"] {
        first.put(MODULES, &key(text), value(MODULES, text));
    }
    first.finish_run().unwrap();
    let size = fs::metadata(file(&first, MODULES)).unwrap().len();

    // A run that fails after reading one entry and computing another.
    let failed = open_store(temp.path(), temp.path());
    assert!(failed.get(MODULES, &key("a")).is_some());
    failed.put(MODULES, &key("d"), value(MODULES, "d"));
    assert_eq!(failed.finish_failed_run().unwrap(), Finish::Written);
    assert!(fs::metadata(file(&first, MODULES)).unwrap().len() > size);
    let loaded = open_store(temp.path(), temp.path());
    for text in ["a", "b", "c", "d"] {
        assert!(loaded.get(MODULES, &key(text)).is_some(), "{text}");
    }

    // One that computed nothing writes nothing.
    let idle = open_store(temp.path(), temp.path());
    assert!(idle.get(MODULES, &key("a")).is_some());
    assert_eq!(idle.finish_failed_run().unwrap(), Finish::Unchanged);
    // The run after it prunes as ever.
    let complete = open_store(temp.path(), temp.path());
    assert!(complete.get(MODULES, &key("a")).is_some());
    assert_eq!(complete.finish_run().unwrap(), Finish::Written);
    assert!(open_store(temp.path(), temp.path()).get(MODULES, &key("b")).is_none());
}

#[test]
fn an_empty_location_is_an_error_and_writes_nothing() {
    let store = open_store(Path::new(""), Path::new("/project"));
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert!(matches!(store.finish_run(), Err(StoreError::EmptyLocation)));
    assert!(matches!(create_tagged(Path::new("")), Err(StoreError::EmptyLocation)));
}

#[test]
fn tag_files_never_replace_a_file_that_is_there() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    // A directory that appeared between the check and the tagging, already holding a file of the project's.
    let ignore = location.join(IGNORE_FILE);
    fs::create_dir(&location).unwrap();
    fs::write(&ignore, "node_modules\n").unwrap();
    tag(&location);
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "node_modules\n");
    assert_eq!(fs::read_to_string(location.join(TAG_FILE)).unwrap(), CACHEDIR_TAG);
}

#[test]
fn a_namespace_file_over_the_cap_is_not_read() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    let big = "z".repeat(4096);
    store.put(MODULES, &key(&big), value(MODULES, &big));
    store.finish_run().unwrap();

    let capped = DiskStore::open(
        temp.path(),
        temp.path(),
        DiskStoreOptions::default().with_max_size(1024),
    );
    assert!(capped.get(MODULES, &key(&big)).is_none());
    let notes = capped.take_notes();
    assert!(
        notes
            .iter()
            .any(|note| note.contains("larger than the cap of 1024 bytes")),
        "{notes:?}"
    );
    // Under a cap that holds it, it is read.
    assert!(open_store(temp.path(), temp.path()).get(MODULES, &key(&big)).is_some());
    let size = fs::metadata(file(&store, MODULES)).unwrap().len();
    assert_eq!(
        read_at_most(&file(&store, MODULES), size)
            .unwrap()
            .map(|bytes| bytes.len() as u64),
        Some(size)
    );
    assert_eq!(read_at_most(&file(&store, MODULES), size - 1).unwrap(), None);
}

#[test]
fn temporary_files_of_killed_writers_go_with_the_next_write() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(temp.path(), temp.path());
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    store.finish_run().unwrap();
    let stale = [
        store.dir().join("modules.bin.4242.tmp"),
        store.dir().join("modules.bin.00ff00ff00ff00ff.tmp"),
        store.dir().join("base.1f.tmp"),
    ];
    for path in &stale {
        fs::write(path, "left behind").unwrap();
    }
    let foreign = store.dir().join("notes.tmp");
    fs::write(&foreign, "keep").unwrap();
    // A temporary file counts towards no size.
    let sized = |location: &Path| stats(location).unwrap().bytes;
    let with_stale = sized(temp.path());
    for path in &stale {
        fs::remove_file(path).unwrap();
    }
    assert_eq!(with_stale, sized(temp.path()));
    for path in &stale {
        fs::write(path, "left behind").unwrap();
    }

    let next = open_store(temp.path(), temp.path());
    next.put(MODULES, &key("b"), value(MODULES, "b"));
    assert_eq!(next.finish_run().unwrap(), Finish::Written);
    for path in &stale {
        assert!(!path.exists(), "{}", path.display());
    }
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "keep");
    // The write left no temporary file of its own.
    let left: Vec<String> = fs::read_dir(store.dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| is_temporary(name))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[cfg(unix)]
#[test]
fn a_write_never_goes_through_a_planted_link() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let victim = temp.path().join("victim");
    fs::write(&victim, "precious").unwrap();
    let store = open_store(&temp.path().join("cache"), temp.path());
    fs::create_dir_all(store.dir()).unwrap();
    // Links at the names a write used to take (`<file>.<pid>.tmp`) and at the files it renames onto.
    let planted = [
        store.dir().join(format!("modules.bin.{}.tmp", std::process::id())),
        store.dir().join(format!("base.{}.tmp", std::process::id())),
    ];
    for link in &planted {
        symlink(&victim, link).unwrap();
    }
    store.put(MODULES, &key("a"), value(MODULES, "a"));
    assert_eq!(store.finish_run().unwrap(), Finish::Written);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    for link in &planted {
        assert!(fs::symlink_metadata(link).is_err(), "{} is still there", link.display());
    }
    assert!(
        open_store(&temp.path().join("cache"), temp.path())
            .get(MODULES, &key("a"))
            .is_some()
    );

    // Creating a temporary file where a link is fails instead of following it.
    let link = temp.path().join("link.tmp");
    symlink(&victim, &link).unwrap();
    assert_eq!(create_new(&link).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
}

#[cfg(unix)]
#[test]
fn an_unreadable_store_does_not_hide_the_others() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let bases = [temp.path().join("a"), temp.path().join("b"), temp.path().join("c")];
    let mut dirs = Vec::new();
    for base in &bases {
        fs::create_dir(base).unwrap();
        let store = open_store(&location, base);
        let big = "z".repeat(4096);
        store.put(MODULES, &key(&big), value(MODULES, &big));
        store.finish_run().unwrap();
        dirs.push(store.dir().to_path_buf());
    }
    let chmod = |dir: &Path, mode: u32| fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
    // One store that cannot be entered at all, one that can be entered but not listed.
    chmod(&dirs[0], 0o000);
    chmod(&dirs[1], 0o100);
    if fs::read_dir(&dirs[0]).is_ok() {
        // A user the modes do not bind (root): nothing to test.
        chmod(&dirs[0], 0o755);
        chmod(&dirs[1], 0o755);
        return;
    }

    let found = stats(&location).unwrap();
    assert_eq!(found.stores.len(), 3);
    let unreadable: Vec<&StoreStats> = found.stores.iter().filter(|store| !store.readable).collect();
    assert_eq!(unreadable.len(), 2);
    assert!(
        unreadable
            .iter()
            .all(|store| store.bytes == 0 && store.namespaces.is_empty())
    );

    // The cap still drops the readable store of another base.
    let fourth = temp.path().join("d");
    fs::create_dir(&fourth).unwrap();
    let capped = DiskStore::open(&location, &fourth, DiskStoreOptions::default().with_max_size(4500));
    capped.put(MODULES, &key("small"), value(MODULES, "small"));
    assert_eq!(capped.finish_run().unwrap(), Finish::Written);
    assert!(
        !dirs[2].join("modules.bin").exists(),
        "the cap did not evict the readable store"
    );

    // Cleaning this base's store works; cleaning everything removes what it can and reports what it cannot.
    assert_eq!(clean(&location, Some(&fourth)).unwrap().stores, 1);
    let fifth = temp.path().join("e");
    fs::create_dir(&fifth).unwrap();
    let last = open_store(&location, &fifth);
    last.put(MODULES, &key("x"), value(MODULES, "x"));
    last.finish_run().unwrap();
    assert!(clean(&location, None).is_err());
    assert!(!last.dir().exists());

    chmod(&dirs[0], 0o755);
    chmod(&dirs[1], 0o755);
    assert_eq!(clean(&location, None).unwrap().stores, 2);
}

#[test]
fn a_store_told_not_to_evict_leaves_the_other_bases_alone() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let (old, new) = (temp.path().join("old"), temp.path().join("new"));
    for base in [&old, &new] {
        fs::create_dir(base).unwrap();
    }
    let big = "z".repeat(4096);
    let first = open_store(&location, &old);
    first.put(MODULES, &key(&big), value(MODULES, &big));
    first.finish_run().unwrap();

    let options = DiskStoreOptions::default().with_max_size(0).with_evict_others(false);
    let second = DiskStore::open(&location, &new, options);
    second.put(MODULES, &key("small"), value(MODULES, "small"));
    assert_eq!(second.finish_run().unwrap(), Finish::Written);
    // The cap emptied this base's own store and nothing else.
    assert!(open_store(&location, &new).get(MODULES, &key("small")).is_none());
    assert!(open_store(&location, &old).get(MODULES, &key(&big)).is_some());
}

#[test]
fn a_location_over_its_cap_drops_other_bases_least_recently_written_first() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    let big = "x".repeat(4096);
    let mut bases = Vec::new();
    for (index, name) in ["old", "middle", "new"].iter().enumerate() {
        let base = temp.path().join(name);
        fs::create_dir(&base).unwrap();
        let store = open_store(&location, &base);
        store.put(
            MODULES,
            &key(&format!("{name}{big}")),
            value(MODULES, &format!("{name}{big}")),
        );
        store.finish_run().unwrap();
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + index as u64 * 1000);
        for entry in fs::read_dir(store.dir()).unwrap() {
            File::options()
                .write(true)
                .open(entry.unwrap().path())
                .unwrap()
                .set_modified(stamp)
                .unwrap();
        }
        bases.push(store.dir().to_path_buf());
    }
    let total = stats(&location).unwrap().bytes;
    let one = total / 3;
    // Room for two stores: writing a fourth drops the two oldest.
    let fourth = temp.path().join("fourth");
    fs::create_dir(&fourth).unwrap();
    let store = DiskStore::open(
        &location,
        &fourth,
        DiskStoreOptions::default().with_max_size(one * 2 + one / 2),
    );
    store.put(
        MODULES,
        &key(&format!("fourth{big}")),
        value(MODULES, &format!("fourth{big}")),
    );
    assert_eq!(store.finish_run().unwrap(), Finish::Written);
    assert!(!bases[0].exists());
    assert!(!bases[1].exists());
    assert!(bases[2].exists());
    assert!(store.dir().exists());
}

#[test]
fn a_store_over_the_cap_alone_keeps_modules_first() {
    let temp = tempfile::tempdir().unwrap();
    let big = "y".repeat(2048);
    let store = DiskStore::open(
        temp.path(),
        temp.path(),
        DiskStoreOptions::default().with_max_size(3000),
    );
    store.put(OTHER, &key(&format!("t{big}")), value(OTHER, &format!("t{big}")));
    store.put(MODULES, &key(&format!("m{big}")), value(MODULES, &format!("m{big}")));
    store.finish_run().unwrap();
    let loaded = open_store(temp.path(), temp.path());
    assert!(loaded.get(MODULES, &key(&format!("m{big}"))).is_some());
    assert!(loaded.get(OTHER, &key(&format!("t{big}"))).is_none());
}

#[test]
fn stats_and_clean_cover_the_stores_of_a_location() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("cache");
    assert_eq!(stats(&location).unwrap(), CacheStats::default());
    assert_eq!(clean(&location, None).unwrap(), CleanReport::default());
    let (a, b) = (temp.path().join("a"), temp.path().join("b"));
    for base in [&a, &b] {
        fs::create_dir(base).unwrap();
        let store = open_store(&location, base);
        store.put(MODULES, &key("x"), value(MODULES, "x"));
        store.put(MODULES, &key("y"), value(MODULES, "y"));
        store.finish_run().unwrap();
    }
    fs::write(location.join("unrelated.txt"), "keep").unwrap();
    let found = stats(&location).unwrap();
    assert_eq!(found.stores.len(), 2);
    let store = &found.stores[0];
    assert_eq!(store.namespaces.len(), 1);
    assert_eq!(store.namespaces[0].name, "modules");
    assert_eq!(store.namespaces[0].entries, Some(2));
    assert!(store.base.as_deref().is_some_and(|base| base.starts_with("path:")));
    assert_eq!(found.bytes, found.stores.iter().map(|store| store.bytes).sum::<u64>());

    let cleaned = clean(&location, Some(&a)).unwrap();
    assert_eq!(cleaned.stores, 1);
    assert_eq!(stats(&location).unwrap().stores.len(), 1);
    assert_eq!(clean(&location, None).unwrap().stores, 1);
    // A file that is not skif's keeps the location.
    assert!(location.join("unrelated.txt").exists());
    fs::remove_file(location.join("unrelated.txt")).unwrap();
    clean(&location, None).unwrap();
    assert!(!location.exists());
}

#[test]
fn clean_and_the_cap_only_remove_what_a_store_wrote() {
    let temp = tempfile::tempdir().unwrap();
    let location = temp.path().join("shared");
    // Another tool's files, as the pre-commit launcher keeps its downloads below the same variable.
    let foreign = location.join("v0.8.0").join("skif");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(&foreign, "binary").unwrap();
    // A directory named like a base key that no store wrote.
    let lookalike = location.join("0123456789abcdef0123456789abcdef");
    fs::create_dir(&lookalike).unwrap();
    fs::write(lookalike.join("modules.bin"), "not a store").unwrap();

    let (old, new) = (temp.path().join("old"), temp.path().join("new"));
    for base in [&old, &new] {
        fs::create_dir(base).unwrap();
    }
    let big = "z".repeat(4096);
    let first = open_store(&location, &old);
    first.put(MODULES, &key(&big), value(MODULES, &big));
    first.finish_run().unwrap();
    // A file skif did not write, inside a store's directory.
    let stray = first.dir().join("notes.txt");
    fs::write(&stray, "keep").unwrap();

    // Neither the look-alike nor the foreign files count as stores.
    assert_eq!(stats(&location).unwrap().stores.len(), 1);

    // The cap evicts the old store's files and nothing else.
    let second = DiskStore::open(&location, &new, DiskStoreOptions::default().with_max_size(4500));
    second.put(MODULES, &key("small"), value(MODULES, "small"));
    assert_eq!(second.finish_run().unwrap(), Finish::Written);
    assert!(!file(&first, MODULES).exists(), "the cap did not evict the old store");
    assert_eq!(fs::read_to_string(&stray).unwrap(), "keep");
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "binary");
    assert_eq!(
        fs::read_to_string(lookalike.join("modules.bin")).unwrap(),
        "not a store"
    );

    // Cleaning everything removes the stores and leaves every foreign file, and so the location.
    fs::write(second.dir().join("other.txt"), "keep").unwrap();
    let cleaned = clean(&location, None).unwrap();
    assert_eq!(cleaned.stores, 1);
    assert!(!file(&second, MODULES).exists());
    assert_eq!(fs::read_to_string(second.dir().join("other.txt")).unwrap(), "keep");
    assert_eq!(fs::read_to_string(&stray).unwrap(), "keep");
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "binary");
    assert_eq!(
        fs::read_to_string(lookalike.join("modules.bin")).unwrap(),
        "not a store"
    );
    assert_eq!(stats(&location).unwrap().stores.len(), 0);
}

#[test]
fn only_files_a_store_writes_are_store_files() {
    for name in [
        ".lock",
        "base",
        "CACHEDIR.TAG",
        ".gitignore",
        "modules.bin",
        "other.facts.bin",
        "modules.bin.123.tmp",
        "modules.bin.00ff00ff00ff00ff.tmp",
        "base.9.tmp",
    ] {
        assert!(is_store_file(name), "{name}");
    }
    for name in [
        "skif",
        "notes.txt",
        "Modules.bin",
        ".bin",
        "modules.bin.tmp",
        "modules.bin.x.tmp",
        "skif.exe",
        "a.tmp",
    ] {
        assert!(!is_store_file(name), "{name}");
    }
}
