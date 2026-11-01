use crate::{
    plural::Pluralize,
    rules::{ERROR, IssueLevel, IssuesList, PackageType, SUCCESS, WARNING},
};
use anyhow::Result;
use colored::Colorize;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};
use serde::Serialize;
use std::io::Write;
use std::time::Instant;

pub fn print_success() {
    println!();
    println!("{}", format!("{SUCCESS} No issues found").green());
}

pub fn print_error(title: &str, message: &str) {
    eprintln!();
    eprintln!(" {} {}", IssueLevel::Error, title.bold());
    eprintln!("   {}", message.bright_black());
}

pub fn print_issues(issues: IssuesList) -> Result<()> {
    // Lock stdout manually instead of in every `println`
    // calls, since we might have a lot of them.
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();

    for (package_type, issues) in issues {
        writeln!(lock)?;
        writeln!(
            lock,
            "{} found in {}:",
            "issue".plural(issues.len()),
            package_type.to_string().bold(),
        )?;

        for issue in issues {
            writeln!(lock)?;
            writeln!(
                lock,
                " {} {} {}",
                issue.level().to_string().bold(),
                issue.why().bold(),
                issue.name().bright_black(),
            )?;
            writeln!(lock, "{}", issue.message())?;
        }
    }

    Ok(())
}

pub fn print_footer(
    total_issues: usize,
    total_packages: usize,
    warnings: usize,
    errors: usize,
    fixed: usize,
    start: Instant,
) {
    println!();
    println!(
        "{} found {} across {} in {:?}.",
        "issue".plural(total_issues),
        format!("({errors} {ERROR}, {warnings} {WARNING}, {fixed} {SUCCESS})").bright_black(),
        "package".plural(total_packages),
        start.elapsed(),
    );
    println!(
        "{}",
        " Note: use `-i` to ignore dependencies, `-r` to ignore rules, `-p` to ignore packages, and `-f` to autofix fixable issues."
            .bright_black()
    );
}

#[derive(Debug, Serialize)]
pub struct JsonSummary {
    pub errors: usize,
    pub warnings: usize,
    pub fixed: usize,
    pub packages: usize,
}

#[derive(Debug, Serialize)]
pub struct JsonIssue {
    pub rule: String,
    pub level: IssueLevel,
    pub package: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct JsonReport {
    pub summary: JsonSummary,
    pub issues: Vec<JsonIssue>,
}

pub fn build_report(issues: IssuesList, total_packages: usize) -> JsonReport {
    let summary = JsonSummary {
        errors: issues.len_by_level(IssueLevel::Error),
        warnings: issues.len_by_level(IssueLevel::Warning),
        fixed: issues.len_by_level(IssueLevel::Fixed),
        packages: total_packages,
    };

    let issues = issues
        .into_iter()
        .flat_map(|(package_type, issues)| {
            let package = match package_type {
                PackageType::None => None,
                package_type => Some(package_type.to_string()),
            };

            issues.into_iter().map(move |issue| JsonIssue {
                rule: issue.name().to_string(),
                level: issue.level(),
                package: package.clone(),
                message: issue.message(),
            })
        })
        .collect();

    JsonReport { summary, issues }
}

pub fn print_json(report: &JsonReport) -> Result<()> {
    let mut lock = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut lock, report)?;
    writeln!(lock)?;

    Ok(())
}

pub fn get_render_config() -> RenderConfig<'static> {
    let mut render_config = RenderConfig::default_colored()
        .with_prompt_prefix(Styled::new("✓").with_fg(Color::DarkGrey))
        .with_help_message(StyleSheet::new().with_fg(Color::DarkGrey))
        .with_highlighted_option_prefix(Styled::new(" →").with_fg(Color::LightCyan))
        .with_canceled_prompt_indicator(Styled::new("✗").with_fg(Color::LightRed))
        .with_scroll_up_prefix(Styled::new("↑").with_fg(Color::DarkGrey))
        .with_scroll_down_prefix(Styled::new("↓").with_fg(Color::DarkGrey));
    render_config.answered_prompt_prefix = Styled::new("✓").with_fg(Color::LightGreen);
    render_config
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::rules::{
        non_existant_packages::NonExistantPackagesIssue, root_package_dependencies::RootPackageDependenciesIssue,
        root_package_manager_field::RootPackageManagerFieldIssue,
    };
    use serde_json::{Value, json};

    #[test]
    fn report_empty() {
        let ignored = Vec::new();
        let report = build_report(IssuesList::new(&ignored), 3);
        let value = serde_json::to_value(&report).unwrap();

        assert_eq!(
            value,
            json!({
                "summary": { "errors": 0, "warnings": 0, "fixed": 0, "packages": 3 },
                "issues": [],
            })
        );
    }

    #[test]
    fn report_issues() {
        colored::control::set_override(false);

        let ignored = Vec::new();
        let mut issues = IssuesList::new(&ignored);
        issues.add_raw(PackageType::Root, RootPackageManagerFieldIssue::new());
        issues.add_raw(PackageType::Root, RootPackageDependenciesIssue::new());
        issues.add_raw(
            PackageType::None,
            NonExistantPackagesIssue::new(false, vec!["a".to_string()], vec!["b".to_string()]),
        );
        issues.add_raw(
            PackageType::Package("packages/a".to_string()),
            RootPackageDependenciesIssue::new(),
        );

        let report = build_report(issues, 2);
        let value = serde_json::to_value(&report).unwrap();

        assert_eq!(
            value["summary"],
            json!({ "errors": 1, "warnings": 3, "fixed": 0, "packages": 2 })
        );

        let issues = value["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 4);
        assert_eq!(issues[0]["rule"], "root-package-manager-field");
        assert_eq!(issues[0]["level"], "error");
        assert_eq!(issues[0]["package"], "./package.json");
        assert_eq!(issues[1]["level"], "warning");
        assert_eq!(issues[2]["package"], Value::Null);
        assert_eq!(issues[3]["package"], "packages/a/package.json");
        assert!(
            issues
                .iter()
                .all(|issue| issue["message"].as_str().is_some_and(|m| !m.is_empty()))
        );
    }

    #[test]
    fn level_serializes_lowercase() {
        assert_eq!(serde_json::to_value(IssueLevel::Fixed).unwrap(), "fixed");
    }
}
