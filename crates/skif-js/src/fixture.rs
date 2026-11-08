//! Fixture directories: test data the analysis does not judge as production code.

use std::path::Path;

use skif_config::FIXTURE_DIRECTORIES;

/// Whether a directory name marks test data, one of [`FIXTURE_DIRECTORIES`] (`fixtures` and `__fixtures__`).
fn is_fixture_directory(name: &str) -> bool {
    FIXTURE_DIRECTORIES.contains(&name)
}

/// Whether `path`, relative to the workspace root, lies in a fixture directory (see [`is_fixture_directory`]).
///
/// Fixtures are test data, often deliberately broken sources that tests load by path, so their imports are never held
/// against the project and, unless `[js] include-fixtures` is set, their files and exports are never reported as
/// unused. Other directories of test data go on `[js] ignore`.
pub(crate) fn in_fixture_directory(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_str().is_some_and(is_fixture_directory))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{in_fixture_directory, is_fixture_directory};

    #[test]
    fn fixture_directories_are_recognised_by_name() {
        for name in ["fixtures", "__fixtures__"] {
            assert!(is_fixture_directory(name), "{name}");
        }
        for name in [
            "fixture",
            "src",
            "fixtures-api",
            "test_root",
            "my-fixtures-lib",
            "__testfixtures__",
            "colorize-fixtures",
            "e2e_fixtures",
        ] {
            assert!(!is_fixture_directory(name), "{name}");
        }
    }

    #[test]
    fn a_path_is_in_a_fixture_directory_when_any_component_is_one() {
        assert!(in_fixture_directory(Path::new("test/fixtures/broken.ts")));
        assert!(in_fixture_directory(Path::new("packages/a/src/__fixtures__/deep/x.ts")));
        assert!(!in_fixture_directory(Path::new("src/fixtures.ts")));
        assert!(!in_fixture_directory(Path::new("src/fixture/x.ts")));
    }
}
