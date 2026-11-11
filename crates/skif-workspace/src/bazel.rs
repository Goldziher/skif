//! What a Bazel repository's root files say about it.

use std::path::Path;

use skif_core::FileSystem;

/// The `name` argument of the `module(...)` call of `MODULE.bazel` in `root`, or else of the `workspace(...)` call of
/// its `WORKSPACE` or `WORKSPACE.bazel`: the name the repository is known by.
#[must_use]
pub fn bazel_workspace_name(fs: &dyn FileSystem, root: &Path) -> Option<String> {
    ["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"]
        .iter()
        .find_map(|file| {
            let text = fs.read_to_string(&root.join(file)).ok()?;
            let call = if *file == "MODULE.bazel" {
                "module("
            } else {
                "workspace("
            };
            let at = text.match_indices(call).find_map(|(at, _)| {
                text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let body = &text[at + call.len()..];
            let body = &body[..body.find(')')?];
            let at = body.match_indices("name").find_map(|(at, _)| {
                body[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let value = body[at + "name".len()..].trim_start().strip_prefix('=')?.trim_start();
            let quote = value.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
            let value = &value[1..];
            let name = &value[..value.find(quote)?];
            (!name.is_empty()).then(|| name.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn bazel_name_is_read_from_the_module_call_only() {
        let fs = MemoryFs::new().with_file(
            "/ws/MODULE.bazel",
            "bazel_dep(name = \"rules_x\", version = \"1\")\nmodule(repo_name = \"r\", name = \"mono\")\n",
        );
        assert_eq!(bazel_workspace_name(&fs, Path::new("/ws")).as_deref(), Some("mono"));
        let none = MemoryFs::new().with_file("/ws/MODULE.bazel", "bazel_dep(name = \"rules_x\")\n");
        assert_eq!(bazel_workspace_name(&none, Path::new("/ws")), None);
    }

    #[test]
    fn a_workspace_file_names_the_repository_too() {
        let fs = MemoryFs::new().with_file("/ws/WORKSPACE", "workspace(name = 'mono')\n");
        assert_eq!(bazel_workspace_name(&fs, Path::new("/ws")).as_deref(), Some("mono"));
    }
}
