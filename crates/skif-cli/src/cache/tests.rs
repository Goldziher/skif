use super::*;

fn env() -> CacheEnv {
    CacheEnv {
        user_cache_dir: Some(PathBuf::from("/user/cache")),
        ..CacheEnv::empty()
    }
}

fn settings(flags: &CacheFlags, env: &CacheEnv, base: &Path) -> Settings {
    let (settings, warnings) = resolve(flags, env, base, &[]);
    assert_eq!(warnings, Vec::<String>::new());
    settings
}

#[test]
fn the_cache_is_on_by_default_in_the_user_cache_directory() {
    let base = tempfile::tempdir().unwrap();
    let found = settings(&CacheFlags::default(), &env(), base.path());
    assert_eq!(
        found,
        Settings {
            enabled: true,
            location: Some(PathBuf::from("/user/cache/skif")),
            max_size: DEFAULT_MAX_SIZE,
            asked: false,
            write: true,
            cap: CapScope::Location,
        }
    );
}

#[test]
fn the_default_location_prefers_bazel_then_node_modules_then_the_user_directory_then_memory() {
    let base = tempfile::tempdir().unwrap();
    let nowhere = settings(&CacheFlags::default(), &CacheEnv::empty(), base.path());
    assert_eq!(nowhere.location, None);
    assert!(nowhere.enabled);
    std::fs::create_dir(base.path().join("node_modules")).unwrap();
    let below = settings(&CacheFlags::default(), &env(), base.path());
    assert_eq!(
        below.location,
        Some(base.path().join("node_modules").join(".cache").join("skif"))
    );
    let bazel = CacheEnv {
        bazel: Some(PathBuf::from("/bazel/tmp")),
        ..env()
    };
    assert_eq!(
        settings(&CacheFlags::default(), &bazel, base.path()).location,
        Some(PathBuf::from("/bazel/tmp/skif"))
    );
}

#[test]
fn flags_beat_the_environment() {
    let base = tempfile::tempdir().unwrap();
    let variables = CacheEnv {
        enabled: Some("0".into()),
        dir: Some(PathBuf::from("/from/env")),
        max_size: Some("1MiB".into()),
        ..env()
    };
    let from_env = settings(&CacheFlags::default(), &variables, base.path());
    assert!(!from_env.enabled);
    // On Windows `/from/env` is relative to the current drive.
    assert_eq!(from_env.location, Some(absolute(Path::new("/from/env"))));
    assert_eq!(from_env.max_size, 1 << 20);

    let flags = CacheFlags {
        cache: true,
        no_cache: false,
        location: Some(PathBuf::from("/from/flag")),
        max_size: Some(42),
    };
    let from_flags = settings(&flags, &variables, base.path());
    assert!(from_flags.enabled);
    assert!(from_flags.asked);
    assert_eq!(from_flags.location, Some(absolute(Path::new("/from/flag"))));
    assert_eq!(from_flags.max_size, 42);

    let on = CacheEnv {
        enabled: Some("1".into()),
        ..env()
    };
    let off = CacheFlags {
        no_cache: true,
        ..CacheFlags::default()
    };
    assert!(!settings(&off, &on, base.path()).enabled);
    assert!(settings(&CacheFlags::default(), &on, base.path()).asked);
}

#[test]
fn an_environment_value_that_does_not_parse_is_ignored_with_a_warning() {
    let base = tempfile::tempdir().unwrap();
    let variables = CacheEnv {
        enabled: Some("maybe".into()),
        max_size: Some("lots".into()),
        ..env()
    };
    let (found, warnings) = resolve(&CacheFlags::default(), &variables, base.path(), &[]);
    assert!(found.enabled);
    assert_eq!(found.max_size, DEFAULT_MAX_SIZE);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("SKIF_CACHE=maybe"), "{warnings:?}");
    assert!(warnings[1].contains("SKIF_CACHE_MAX_SIZE"), "{warnings:?}");
}

#[test]
fn ci_reads_and_does_not_write_unless_a_location_is_given() {
    let base = tempfile::tempdir().unwrap();
    let ci = CacheEnv { ci: true, ..env() };
    let default = settings(&CacheFlags::default(), &ci, base.path());
    assert!(default.enabled);
    assert!(!default.write);
    let named = CacheFlags {
        location: Some(PathBuf::from("/restored")),
        ..CacheFlags::default()
    };
    assert!(settings(&named, &ci, base.path()).write);
    let from_env = CacheEnv {
        dir: Some(PathBuf::from("/restored")),
        ..ci
    };
    assert!(settings(&CacheFlags::default(), &from_env, base.path()).write);
}

#[test]
fn the_user_cache_directory_follows_the_platform() {
    // `/xdg` is not absolute on Windows.
    let xdg = PathBuf::from(if cfg!(windows) { r"C:\xdg" } else { "/xdg" });
    let home = Some(PathBuf::from("/home/me"));
    assert_eq!(user_cache_dir(Some(xdg.clone()), home.clone(), None), Some(xdg));
    // A relative `XDG_CACHE_HOME` is not one, as the specification says.
    let found = user_cache_dir(Some(PathBuf::from("relative")), home, None);
    if cfg!(target_os = "macos") {
        assert_eq!(found, Some(PathBuf::from("/home/me/Library/Caches")));
    } else if cfg!(not(windows)) {
        assert_eq!(found, Some(PathBuf::from("/home/me/.cache")));
    }
    if cfg!(not(windows)) {
        assert_eq!(user_cache_dir(None, None, None), None);
    }
}

#[test]
fn the_watcher_leaves_a_named_location_out_of_its_scan() {
    let dir = tempfile::tempdir().unwrap();
    let flags = CacheFlags {
        location: Some(dir.path().to_path_buf()),
        ..CacheFlags::default()
    };
    let written = written_dirs(&flags, &CacheEnv::empty());
    assert!(written.contains(&dir.path().to_path_buf()));
    assert!(written.contains(&std::fs::canonicalize(dir.path()).unwrap()));
    let off = CacheFlags {
        no_cache: true,
        ..flags
    };
    assert_eq!(written_dirs(&off, &CacheEnv::empty()), Vec::<PathBuf>::new());
}

#[test]
fn the_seed_of_a_server_is_capped_and_dropped_once_it_has_served() {
    let base = tempfile::tempdir().unwrap();
    let location = tempfile::tempdir().unwrap();
    let variables = CacheEnv {
        dir: Some(location.path().to_path_buf()),
        max_size: Some("64".into()),
        ..env()
    };
    let seeded = Seeded::new(&variables, base.path(), &[]);
    assert!(seeded.is_seeded());
    {
        let seed = seeded.seed.read().unwrap();
        assert!(format!("{:?}", seed.as_ref().unwrap()).contains("read_only: true"));
    }
    // The first lint has run: the memory holds what it parsed, and the store's file is let go.
    seeded.prune();
    assert!(!seeded.is_seeded());

    let off = CacheEnv {
        enabled: Some("0".into()),
        ..variables
    };
    assert!(!Seeded::new(&off, base.path(), &[]).is_seeded());
}

#[test]
fn a_location_stays_inside_the_directory_of_its_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    assert!(stays_inside(&root, &root.join(".cache").join("skif")));
    assert!(stays_inside(&root, &root));
    std::fs::create_dir(root.join("existing")).unwrap();
    assert!(stays_inside(&root, &root.join("existing").join("deeper")));
    let outside = tempfile::tempdir().unwrap();
    assert!(!stays_inside(&root, outside.path()));
    assert!(!stays_inside(&root, &root.parent().unwrap().join("sibling")));
    #[cfg(unix)]
    {
        // A link below the directory that leads out of it, whether or not what it names exists.
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        assert!(!stays_inside(&root, &root.join("link")));
        assert!(!stays_inside(&root, &root.join("link").join("store")));
        std::os::unix::fs::symlink(outside.path().join("missing"), root.join("dangling")).unwrap();
        assert!(!stays_inside(&root, &root.join("dangling")));
        // One that stays inside is fine.
        std::os::unix::fs::symlink(root.join("existing"), root.join("inner")).unwrap();
        assert!(stays_inside(&root, &root.join("inner").join("store")));
    }
}
