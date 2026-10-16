mod assemble;
mod collect;
mod graph;
mod laravel;
mod project;
mod report;
mod verdict;

use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use rayon::prelude::*;

use crate::graph::Graph;
use crate::project::Project;

const USAGE: &str = "usage:
  puml-extract <project-dir> [--out graph.json] [--html viewer.html] [--dev]
  puml-extract report <project-dir> [--dev]
  puml-extract show <project-dir> <module|namespace|class> [--dev]
  puml-extract show <project-dir> <from> <to> [--dev]";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments.iter().any(|argument| argument == "-h" || argument == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match arguments.first().map(String::as_str) {
        Some("report") => report_command(&arguments[1..]),
        Some("show") => show_command(&arguments[1..]),
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

    let facts = project
        .php_files()
        .into_par_iter()
        .map(|path| {
            let relative = project.relative(&path);
            let facts = collect::collect_file(&path, &relative);

            (path, facts)
        })
        .collect();

    let mut graph = assemble::assemble(&project, facts);
    graph.stats.extract_ms = started.elapsed().as_millis();

    Ok(graph)
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
