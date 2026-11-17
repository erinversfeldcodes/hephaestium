//! Writing the files a repository is missing from the baseline pack.
//!
//! The templates and the items that name them are the ones in the Athenaeum
//! release this crate pins. Everything that can go wrong is found before
//! anything is written, so a run either has a plan it can carry out or writes
//! nothing. A file is created exclusively, so one that exists is never
//! overwritten, and nothing is written through a symbolic link.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::baseline::{self, CouldNotCheck};

/// A file written, or that would be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The baseline item it satisfies.
    pub item: String,
    /// Where, relative to the repository root.
    pub destination: String,
}

/// A file that was not written, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub item: String,
    pub destination: String,
    pub reason: String,
}

/// What applying did.
#[derive(Debug)]
pub struct Applied {
    /// Whether any required item is still unsatisfied.
    pub failed: bool,
    /// The report a gate result carries.
    pub report: Value,
    /// One line for each thing done, for a person to read.
    pub table: String,
}

/// Why nothing could be applied.
#[derive(Debug)]
pub struct CouldNotApply(pub String);

impl std::fmt::Display for CouldNotApply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<CouldNotCheck> for CouldNotApply {
    fn from(error: CouldNotCheck) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct Items {
    items: BTreeMap<String, ItemApply>,
}

#[derive(Debug, Deserialize)]
struct ItemApply {
    apply: Option<Apply>,
}

#[derive(Debug, Deserialize)]
struct Apply {
    files: Vec<FileSpec>,
}

#[derive(Debug, Deserialize)]
struct FileSpec {
    template: String,
    destination: String,
}

/// The answers a template can use. Whether they are valid is the baseline
/// check's business, which has already read them.
#[derive(Debug, Deserialize)]
struct Answers {
    licence: String,
    holder: Option<String>,
    contact: String,
}

/// One file to write, fully worked out.
struct Planned {
    item: String,
    destination: String,
    contents: String,
}

/// Apply the baseline's templates to the repository at `root`.
///
/// # Errors
///
/// Returns why nothing was applied: the answers are missing or not valid, a
/// template needs a value that is not given, the licence has no templates, a
/// path goes through a symbolic link, or a file could not be written.
///
/// # Panics
///
/// Only if the pinned baseline cannot be read, which is a defect in this crate
/// and is ruled out by a test that reads it.
pub fn apply(
    root: &Path,
    answers_path: &Path,
    repository: &str,
    year: u32,
    dry_run: bool,
) -> Result<Applied, CouldNotApply> {
    let answers_text = std::fs::read_to_string(answers_path).map_err(|error| {
        CouldNotApply(format!(
            "the repository's answers are needed to fill in the templates, and {}: {error}",
            answers_path.display()
        ))
    })?;

    let before = baseline::check(root, answers_path, None)?;
    let answers: Answers = toml::from_str(&answers_text)
        .map_err(|error| CouldNotApply(format!("{}: {error}", answers_path.display())))?;
    let items: Items = toml::from_str(athenaeum::baseline::ITEMS)
        .expect("the pinned baseline to be readable as a list of items");

    let variables = variables_for(repository, year, &answers);

    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    for (id, item) in &items.items {
        let Some(apply) = &item.apply else { continue };
        if before.report["items"][id]["outcome"] != "unsatisfied" {
            continue;
        }
        for file in &apply.files {
            let contents = render_file(id, file, &variables, &answers.licence)?;
            match destination_state(root, &file.destination)? {
                Destination::Free => planned.push(Planned {
                    item: id.clone(),
                    destination: file.destination.clone(),
                    contents,
                }),
                Destination::Taken(reason) => skipped.push(Skipped {
                    item: id.clone(),
                    destination: file.destination.clone(),
                    reason,
                }),
            }
        }
    }

    if !dry_run {
        for plan in &planned {
            write_new(root, &plan.destination, &plan.contents)?;
        }
    }

    let written: Vec<Written> = planned
        .iter()
        .map(|plan| Written {
            item: plan.item.clone(),
            destination: plan.destination.clone(),
        })
        .collect();

    // What is left, after the files are written or, for a dry run, as they
    // would be once written.
    let after = baseline::check(root, answers_path, None)?;
    let covered: Vec<&str> = written.iter().map(|w| w.item.as_str()).collect();
    let remaining: Vec<Value> = after.report["items"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, item)| {
            item["outcome"] == "unsatisfied" && !(dry_run && covered.contains(&id.as_str()))
        })
        .map(|(id, item)| {
            json!({"item": id, "severity": item["severity"], "detail": item["detail"]})
        })
        .collect();
    let failed = remaining
        .iter()
        .any(|entry| entry["severity"] == "required");

    Ok(Applied {
        failed,
        table: table_of(&written, &skipped, &remaining, dry_run),
        report: json!({
            "dry_run": dry_run,
            "written": written.iter().map(|w| json!({"item": w.item, "destination": w.destination})).collect::<Vec<_>>(),
            "skipped": skipped.iter().map(|s| json!({"item": s.item, "destination": s.destination, "reason": s.reason})).collect::<Vec<_>>(),
            "remaining": remaining,
        }),
    })
}

/// The values a template may use, by name. A value that is not given is absent.
fn variables_for(
    repository: &str,
    year: u32,
    answers: &Answers,
) -> BTreeMap<&'static str, Option<String>> {
    let (owner, name) = repository
        .split_once('/')
        .unwrap_or((repository, repository));
    BTreeMap::from([
        ("owner", Some(owner.to_owned())),
        ("repository", Some(name.to_owned())),
        ("year", Some(year.to_string())),
        ("licence", Some(answers.licence.clone())),
        ("holder", answers.holder.clone()),
        ("contact", Some(answers.contact.clone())),
    ])
}

/// The text of one file, from its template.
fn render_file(
    item: &str,
    file: &FileSpec,
    variables: &BTreeMap<&'static str, Option<String>>,
    licence: &str,
) -> Result<String, CouldNotApply> {
    let path = fill(&file.template, variables, item, &file.template)?;
    let template = athenaeum::baseline::TEMPLATES
        .iter()
        .find(|template| template.path == path)
        .ok_or_else(|| {
            CouldNotApply(format!(
                "{item}: there is no template {path}; the pack has none for the licence {licence}"
            ))
        })?;
    fill(template.text, variables, item, &path)
}

/// Replace each `{{name}}` with its value.
fn fill(
    text: &str,
    variables: &BTreeMap<&'static str, Option<String>>,
    item: &str,
    template: &str,
) -> Result<String, CouldNotApply> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(CouldNotApply(format!(
                "{item}: the template {template} has a '{{{{' with no '}}}}'"
            )));
        };
        let name = &after[..end];
        match variables.get(name) {
            Some(Some(value)) => out.push_str(value),
            Some(None) => {
                return Err(CouldNotApply(format!(
                    "{item}: the template {template} needs {{{{{name}}}}}, which the repository's answers do not give"
                )));
            }
            None => {
                return Err(CouldNotApply(format!(
                    "{item}: the template {template} uses {{{{{name}}}}}, which is not a variable this tool knows"
                )));
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

enum Destination {
    /// Nothing is there, and every directory on the way is a real directory or absent.
    Free,
    /// Something is there, with what it is.
    Taken(String),
}

/// What is at `relative` under `root`, refusing a path that goes through a
/// symbolic link or through something that is not a directory.
fn destination_state(root: &Path, relative: &str) -> Result<Destination, CouldNotApply> {
    let parts: Vec<&str> = relative.split('/').collect();
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        if matches!(
            Path::new(part).components().next(),
            Some(Component::ParentDir | Component::RootDir)
        ) {
            return Err(CouldNotApply(format!("{relative}: leaves the repository")));
        }
        current.push(part);
        let last = index + 1 == parts.len();
        match current.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Destination::Free);
            }
            Err(error) => {
                return Err(CouldNotApply(format!("{}: {error}", current.display())));
            }
            Ok(found) if last => {
                let what = if found.is_symlink() {
                    "a link"
                } else if found.is_dir() {
                    "a directory"
                } else {
                    "a file"
                };
                return Ok(Destination::Taken(format!("already exists, as {what}")));
            }
            Ok(found) if found.is_symlink() => {
                return Err(CouldNotApply(format!(
                    "{relative}: {} is a symbolic link, and nothing is written through one",
                    current.strip_prefix(root).unwrap_or(&current).display()
                )));
            }
            Ok(found) if !found.is_dir() => {
                return Err(CouldNotApply(format!(
                    "{relative}: {} is not a directory",
                    current.strip_prefix(root).unwrap_or(&current).display()
                )));
            }
            Ok(_) => {}
        }
    }
    Ok(Destination::Free)
}

/// Create the file, and the directories it needs, and write it. `create_new`
/// fails if anything is already there, a link included, so a file is never
/// written over.
fn write_new(root: &Path, relative: &str, contents: &str) -> Result<(), CouldNotApply> {
    let path: PathBuf = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CouldNotApply(format!("{}: {error}", parent.display())))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| CouldNotApply(format!("{relative}: {error}")))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| CouldNotApply(format!("{relative}: {error}")))
}

fn table_of(
    written: &[Written],
    skipped: &[Skipped],
    remaining: &[Value],
    dry_run: bool,
) -> String {
    use std::fmt::Write as _;
    let mut table = String::new();
    let verb = if dry_run { "would write" } else { "wrote" };
    for file in written {
        let _ = writeln!(table, "{verb:<12} {}  ({})", file.destination, file.item);
    }
    for file in skipped {
        let _ = writeln!(
            table,
            "{:<12} {}  ({}): {}",
            "skipped", file.destination, file.item, file.reason
        );
    }
    for entry in remaining {
        let _ = writeln!(
            table,
            "{:<12} {}  [{}]",
            "unsatisfied",
            entry["item"].as_str().unwrap_or_default(),
            entry["severity"].as_str().unwrap_or_default()
        );
    }
    table
}

/// The current year in UTC, from the clock.
#[must_use]
pub fn current_year() -> u32 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    year_of(i64::try_from(seconds / 86_400).unwrap_or(0))
}

/// The year of a day, counted from 1970-01-01, by the civil-calendar rule.
fn year_of(days_since_epoch: i64) -> u32 {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;
    let year = year_of_era + era * 400 + i64::from(month >= 10);
    u32::try_from(year).unwrap_or(1970)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_year_of_a_day_is_right_at_the_edges() {
        assert_eq!(year_of(0), 1970);
        assert_eq!(year_of(365), 1971);
        // 2000-02-29 and 2000-03-01, 2024-12-31 and 2025-01-01.
        assert_eq!(year_of(11_016), 2000);
        assert_eq!(year_of(11_017), 2000);
        assert_eq!(year_of(20_088), 2024);
        assert_eq!(year_of(20_089), 2025);
        assert_eq!(year_of(20_453), 2025);
        assert_eq!(year_of(20_454), 2026);
    }

    #[test]
    fn a_template_is_filled_and_a_missing_or_unknown_value_is_an_error() {
        let variables = BTreeMap::from([("owner", Some("o".to_owned())), ("holder", None)]);
        assert_eq!(
            fill("by {{owner}}!", &variables, "i", "t").expect("filled"),
            "by o!"
        );
        let missing = fill("{{holder}}", &variables, "i", "t").expect_err("a missing value");
        assert!(missing.0.contains("holder"), "{missing}");
        let unknown = fill("{{nope}}", &variables, "i", "t").expect_err("an unknown name");
        assert!(unknown.0.contains("nope"), "{unknown}");
        let open = fill("{{owner", &variables, "i", "t").expect_err("an unclosed variable");
        assert!(open.0.contains("no '}}'"), "{open}");
    }
}
