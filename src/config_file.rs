use crate::packages::Config;
use anyhow::{Result, anyhow};
use std::{fs, path::Path};

/// Config files looked up next to the root `package.json`, in order. The first one found is used.
const CONFIG_FILES: [&str; 2] = ["sherif.config.jsonc", ".sherifrc.jsonc"];

/// Loads the config from `sherif.config.jsonc` or `.sherifrc.jsonc` in `root`, if one exists.
pub fn load(root: &Path) -> Result<Option<Config>> {
    for name in CONFIG_FILES {
        let path = root.join(name);

        if path.is_file() {
            let content = fs::read_to_string(&path)?;
            let config = serde_json::from_str(&strip_jsonc(&content))
                .map_err(|err| anyhow!("Error while parsing {path:?}: {err}"))?;

            return Ok(Some(config));
        }
    }

    Ok(None)
}

/// Turns JSONC into JSON by removing `//` and `/* */` comments and trailing commas, leaving
/// string contents untouched.
fn strip_jsonc(input: &str) -> String {
    strip_trailing_commas(&strip_comments(input))
}

fn strip_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;

    while let Some(char) = chars.next() {
        if in_string {
            output.push(char);

            if char == '\\' {
                output.extend(chars.next());
            } else if char == '"' {
                in_string = false;
            }

            continue;
        }

        match (char, chars.peek()) {
            ('/', Some('/')) => {
                while chars.next_if(|next| *next != '\n').is_some() {}
            }
            ('/', Some('*')) => {
                chars.next();

                let mut previous = '\0';

                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }

                    if next == '\n' {
                        output.push('\n');
                    }

                    previous = next;
                }

                output.push(' ');
            }
            _ => {
                in_string = char == '"';
                output.push(char);
            }
        }
    }

    output
}

fn strip_trailing_commas(input: &str) -> String {
    let chars = input.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for (index, char) in chars.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if char == '\\' {
                escaped = true;
            } else if char == '"' {
                in_string = false;
            }
        } else if char == '"' {
            in_string = true;
        } else if char == ','
            && chars[index + 1..]
                .iter()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'))
        {
            continue;
        }

        output.push(char);
    }

    output
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn strips_line_and_block_comments() {
        let input = "{\n  // line\n  \"a\": 1, /* block\n  spanning */ \"b\": 2\n}";
        let value = serde_json::from_str::<serde_json::Value>(&strip_jsonc(input)).unwrap();

        assert_eq!(value, serde_json::json!({ "a": 1, "b": 2 }));
    }

    #[test]
    fn keeps_comment_markers_inside_strings() {
        let input = r#"{ "url": "http://x/*not a comment*/", "q": "a \" // b" } // end"#;
        let value = serde_json::from_str::<serde_json::Value>(&strip_jsonc(input)).unwrap();

        assert_eq!(
            value,
            serde_json::json!({ "url": "http://x/*not a comment*/", "q": "a \" // b" })
        );
    }

    #[test]
    fn strips_trailing_commas_but_not_in_strings() {
        let input = "{ \"a\": [1, 2, ], \"b\": \"x,}\", }";
        let value = serde_json::from_str::<serde_json::Value>(&strip_jsonc(input)).unwrap();

        assert_eq!(value, serde_json::json!({ "a": [1, 2], "b": "x,}" }));
    }

    #[test]
    fn block_comment_start_is_not_its_own_end() {
        assert_eq!(strip_comments("/*/ x */1").trim(), "1");
    }

    #[test]
    fn load_missing_file() {
        assert!(load(Path::new("fixtures/basic")).unwrap().is_none());
    }

    #[test]
    fn load_config_file() {
        let config = load(Path::new("fixtures/config-file")).unwrap().unwrap();

        assert_eq!(config.ignore_rule, vec!["empty-dependencies".to_string()]);
        assert_eq!(
            config.ignore_rule_for_package.get("unordered-dependencies"),
            Some(&vec!["abc".to_string()])
        );
    }
}
