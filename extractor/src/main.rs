mod assemble;
mod collect;
mod graph;
mod laravel;
mod project;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use rayon::prelude::*;

use crate::project::Project;

const USAGE: &str = "usage: puml-extract <project-dir> [--out graph.json] [--html viewer.html] [--dev]";

fn main() -> ExitCode {
    let mut project_dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut html: Option<PathBuf> = None;
    let mut include_dev = false;

    let mut arguments = std::env::args().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out = arguments.next().map(PathBuf::from),
            "--html" => html = arguments.next().map(PathBuf::from),
            "--dev" => include_dev = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if project_dir.is_none() => project_dir = Some(PathBuf::from(argument)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(project_dir) = project_dir else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let started = Instant::now();

    let project = match Project::load(&project_dir, include_dev) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("puml-extract: {error}");
            return ExitCode::FAILURE;
        }
    };

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
