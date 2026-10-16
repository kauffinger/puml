mod assemble;
mod collect;
mod diff;
mod diff_report;
mod git;
mod graph;
mod laravel;
mod project;
mod report;
mod verdict;

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use rayon::prelude::*;

use crate::collect::FileFacts;
use crate::diff::Diff;
use crate::diff_report::Format;
use crate::git::Changes;
use crate::git::FileStatus;
use crate::graph::Graph;
use crate::project::Project;

const USAGE: &str = "usage:
  puml-extract <project-dir> [--out graph.json] [--html viewer.html] [--dev]
  puml-extract report <project-dir> [--dev]
  puml-extract show <project-dir> <module|namespace|class> [--dev]
  puml-extract show <project-dir> <from> <to> [--dev]
  puml-extract diff <project-dir> <base-ref> [--format text|markdown|json] [--strict] [--dev]";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments.iter().any(|argument| argument == "-h" || argument == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match arguments.first().map(String::as_str) {
        Some("report") => report_command(&arguments[1..]),
        Some("show") => show_command(&arguments[1..]),
        Some("diff") => diff_command(&arguments[1..]),
        _ => extract_command(&arguments),
    }
}

fn extract_command(arguments: &[String]) -> ExitCode {
    let mut project_dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut html: Option<PathBuf> = None;
    let mut include_dev = false;

    let mut arguments = arguments.iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out = arguments.next().map(PathBuf::from),
            "--html" => html = arguments.next().map(PathBuf::from),
            "--dev" => include_dev = true,
            _ if project_dir.is_none() => project_dir = Some(PathBuf::from(argument)),
            _ => return usage_error(),
        }
    }

    let Some(project_dir) = project_dir else {
        return usage_error();
    };

    let graph = match extract(&project_dir, include_dev) {
        Ok(graph) => graph,
        Err(error) => {
            eprintln!("puml-extract: {error}");
            return ExitCode::FAILURE;
        }
    };

    let json = serde_json::to_string(&graph).expect("graph serializes to JSON");

    match &out {
        Some(path) => {
            if let Err(error) = std::fs::write(path, &json) {
                eprintln!("puml-extract: {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        }
        None if html.is_none() => println!("{json}"),
        None => {}
    }

    if let Some(path) = &html {
        if let Err(error) = std::fs::write(path, viewer_html(&json)) {
            eprintln!("puml-extract: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }

    eprintln!(
        "puml-extract: {} files, {} classes, {} external classes, {} edges, {} unresolved in {} ms",
        graph.stats.files,
        graph.stats.classes,
        graph.stats.external_classes,
        graph.stats.edges,
        graph.unresolved.len(),
        graph.stats.extract_ms,
    );

    ExitCode::SUCCESS
}

fn report_command(arguments: &[String]) -> ExitCode {
    let (positional, include_dev) = split_flags(arguments);

    let [project_dir] = positional.as_slice() else {
        return usage_error();
    };

    match extract(Path::new(project_dir), include_dev) {
        Ok(graph) => {
            print!("{}", report::report(&graph, project_dir));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("puml-extract: {error}");
            ExitCode::from(2)
        }
    }
}

fn show_command(arguments: &[String]) -> ExitCode {
    let (positional, include_dev) = split_flags(arguments);

    let (project_dir, target, to) = match positional.as_slice() {
        [project_dir, target] => (project_dir, target, None),
        [project_dir, from, to] => (project_dir, from, Some(to.as_str())),
        _ => return usage_error(),
    };

    match extract(Path::new(project_dir), include_dev) {
        Ok(graph) => {
            let (text, found) = report::show(&graph, target, to);
            print!("{text}");

            if found { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(error) => {
            eprintln!("puml-extract: {error}");
            ExitCode::from(2)
        }
    }
}

fn diff_command(arguments: &[String]) -> ExitCode {
    let mut positional: Vec<&str> = Vec::new();
    let mut format = Format::Text;
    let mut strict = false;
    let mut include_dev = false;

    let mut arguments = arguments.iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--format" => match arguments.next().map(String::as_str) {
                Some("text") => format = Format::Text,
                Some("markdown") => format = Format::Markdown,
                Some("json") => format = Format::Json,
                _ => return usage_error(),
            },
            "--strict" => strict = true,
            "--dev" => include_dev = true,
            _ => positional.push(argument),
        }
    }

    let [project_dir, base] = positional.as_slice() else {
        return usage_error();
    };

    let result = Changes::read(Path::new(project_dir), base)
        .and_then(|changes| extract_both(Path::new(project_dir), &changes, include_dev).map(|graphs| (changes, graphs)));

    match result {
        Ok((changes, (base, head))) => {
            let diff = Diff::of(&base, &head, &changes);
            print!("{}", diff_report::render(&diff, format));

            if strict && diff.worsens() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
        }
        Err(error) => {
            eprintln!("puml-extract: {error}");
            ExitCode::from(2)
        }
    }
}

fn split_flags(arguments: &[String]) -> (Vec<String>, bool) {
    let include_dev = arguments.iter().any(|argument| argument == "--dev");
    let positional = arguments.iter().filter(|argument| *argument != "--dev").cloned().collect();

    (positional, include_dev)
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");

    ExitCode::from(2)
}

fn extract(project_dir: &Path, include_dev: bool) -> Result<Graph, String> {
    let started = Instant::now();
    let project = Project::load(project_dir, include_dev)?;
    let facts = collect_files(&project, project.php_files(), &HashMap::new());

    let mut graph = assemble::assemble(&project, facts);
    graph.stats.extract_ms = started.elapsed().as_millis();

    Ok(graph)
}

fn collect_files(project: &Project, paths: Vec<PathBuf>, known: &HashMap<PathBuf, FileFacts>) -> Vec<(PathBuf, FileFacts)> {
    paths
        .into_par_iter()
        .map(|path| {
            let facts = known.get(&path).cloned().unwrap_or_else(|| collect::collect_file(&path, &project.relative(&path)));

            (path, facts)
        })
        .collect()
}

/// The graphs at the base and at the head. The head is read from disk. The base reuses what was read
/// for every file the pull request leaves alone and reads the base version of the others from git, so
/// it costs one parse of the changed files. Both resolve vendor classes against the vendor/ on disk.
fn extract_both(project_dir: &Path, changes: &Changes, include_dev: bool) -> Result<(Graph, Graph), String> {
    let head_project = Project::load(project_dir, include_dev)?;
    let root = head_project.root.clone();

    let base_project = match changes.files.iter().find(|file| file.path == "composer.json") {
        Some(file) if file.status != FileStatus::Modified => {
            return Err("composer.json was added, deleted or renamed, so there is no base project to compare with".to_string());
        }
        Some(_) => {
            let contents = changes.base_contents(project_dir, "composer.json")?;
            let composer = serde_json::from_slice(&contents).map_err(|error| format!("composer.json at the base: {error}"))?;

            Project::with_composer(root.clone(), &composer, include_dev)
        }
        None => Project::load(project_dir, include_dev)?,
    };

    let head_facts = collect_files(&head_project, head_project.php_files(), &HashMap::new());
    let head = assemble::assemble(&head_project, head_facts.clone());

    let touched: HashSet<PathBuf> = changes
        .files
        .iter()
        .flat_map(|file| [file.base_path(), file.head_path()])
        .flatten()
        .map(|path| root.join(path))
        .collect();
    let unchanged: Vec<PathBuf> = base_project.php_files().into_iter().filter(|path| !touched.contains(path)).collect();

    let mut base_facts = collect_files(&base_project, unchanged, &head_facts.into_iter().collect());

    for path in changes.files.iter().filter_map(|file| file.base_path()) {
        let absolute = root.join(path);

        if base_project.owns(&absolute) {
            let contents = changes.base_contents(project_dir, path)?;
            base_facts.push((absolute.clone(), collect::collect_source(&absolute, path, contents)));
        }
    }

    base_facts.sort_by(|a, b| a.0.cmp(&b.0));

    Ok((assemble::assemble(&base_project, base_facts), head))
}

const VIEWER_INDEX: &str = include_str!("../../viewer/index.html");
const VIEWER_VIZ: &str = include_str!("../../viewer/vendor/viz-global.js");
const VIEWER_MODEL: &str = include_str!("../../viewer/model.js");
const VIEWER_APP: &str = include_str!("../../viewer/app.js");

fn viewer_html(json: &str) -> String {
    let model = VIEWER_MODEL.replace("\nexport ", "\n");
    let app: String = VIEWER_APP
        .lines()
        .filter(|line| !line.starts_with("import "))
        .collect::<Vec<_>>()
        .join("\n");

    VIEWER_INDEX
        .replace(
            r#"<script id="graph" type="application/json"></script>"#,
            &format!(r#"<script id="graph" type="application/json">{}</script>"#, json.replace("</", "<\\/")),
        )
        .replace(
            r#"<script src="vendor/viz-global.js"></script>"#,
            &format!("<script>{VIEWER_VIZ}</script>"),
        )
        .replace(
            r#"<script type="module" src="app.js"></script>"#,
            &format!("<script type=\"module\">\n{model}\n{app}\n</script>"),
        )
}
