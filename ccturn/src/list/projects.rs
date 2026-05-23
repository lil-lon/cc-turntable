use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::anyhow;
use chrono::{DateTime, Utc};

use crate::locator::{CwdSource, read_first_cwd_in_session, reconstruct_cwd_from_encoded};

pub struct ProjectListing {
    pub log_root: PathBuf,
    pub projects: Vec<ProjectRow>,
}

pub struct ProjectRow {
    pub encoded_cwd: String,
    pub project_cwd: Option<PathBuf>,
    pub cwd_source: CwdSource,
    pub session_count: usize,
    pub latest_session_at: Option<DateTime<Utc>>,
}

pub fn list_projects(log_root: &Path) -> anyhow::Result<ProjectListing> {
    if !log_root.exists() {
        return Err(anyhow!("log root {} does not exist", log_root.display()));
    }

    let mut projects: Vec<ProjectRow> = read_project_dirs(log_root)
        .into_iter()
        .map(|(encoded_cwd, project_dir)| build_project_row(&encoded_cwd, &project_dir))
        .collect();

    projects.sort_by(|a, b| {
        Reverse(a.latest_session_at)
            .cmp(&Reverse(b.latest_session_at))
            .then_with(|| a.encoded_cwd.cmp(&b.encoded_cwd))
    });

    Ok(ProjectListing {
        log_root: log_root.to_path_buf(),
        projects,
    })
}

fn build_project_row(encoded_cwd: &str, project_dir: &Path) -> ProjectRow {
    let jsonl_paths = read_jsonl_children(project_dir);
    let session_count = jsonl_paths.len();

    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for jsonl in &jsonl_paths {
        let Ok(metadata) = fs::metadata(jsonl) else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        match &newest {
            Some((current, _)) if *current >= modified => {}
            _ => newest = Some((modified, jsonl.clone())),
        }
    }

    let latest_session_at = newest.as_ref().map(|(t, _)| DateTime::<Utc>::from(*t));

    let (project_cwd, cwd_source) = match newest
        .as_ref()
        .and_then(|(_, path)| read_first_cwd_in_session(path))
    {
        Some(cwd) => (Some(PathBuf::from(cwd)), CwdSource::FirstRecord),
        None => (
            Some(PathBuf::from(reconstruct_cwd_from_encoded(encoded_cwd))),
            CwdSource::ReconstructedFromEncodedCwd,
        ),
    };

    ProjectRow {
        encoded_cwd: encoded_cwd.to_owned(),
        project_cwd,
        cwd_source,
        session_count,
        latest_session_at,
    }
}

fn read_project_dirs(log_root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(log_root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let ft = entry.file_type().ok()?;
            if !ft.is_dir() {
                return None;
            }
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            Some((name, path))
        })
        .collect()
}

fn read_jsonl_children(project_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(project_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let ft = entry.file_type().ok()?;
            if !ft.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            Some(path)
        })
        .collect()
}
