//! A program's files, read from its directory.

use std::path::{Path, PathBuf};

use crate::Report;

/// One file of a program.
#[derive(Clone, Debug)]
pub struct SourceFile {
    /// The path from the program's directory, with `/` between parts, such
    /// as `src/main.inset`. Diagnostics name the file by it.
    pub path: String,
    pub text: String,
}

/// A file of the front end, under `web/`.
#[derive(Clone, Debug)]
pub struct Asset {
    /// The path from `web/`, with `/` between parts, such as `index.html`.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// The `.inset` files of one program: its code under `src/` and its
/// requirements under `requirements/`, and its front end's files in `web/`.
#[derive(Clone, Debug)]
pub struct Program {
    /// In order of path, so diagnostics come out in the same order every time.
    pub files: Vec<SourceFile>,
    /// The files under `web/`, in order of path, or `None` when they are
    /// not known, as for files an editor holds.
    pub web: Option<Vec<Asset>>,
}

/// The root module, which holds the app.
pub const MAIN: &str = "src/main.inset";

impl Program {
    /// Reads every `.inset` file under `src/` and `requirements/` in `dir`,
    /// and every file under `web/`.
    pub fn load(dir: &Path) -> Result<Program, String> {
        let mut files = Vec::new();
        for sub in ["src", "requirements"] {
            let path = dir.join(sub);
            if path.is_dir() {
                collect(dir, &path, &mut files)?;
            }
        }
        let mut program = Program::from_files(files);
        let web = dir.join("web");
        let mut assets = Vec::new();
        if web.is_dir() {
            collect_assets(&web, &web, &mut assets)?;
        }
        program.web = Some(assets);
        if program.file(MAIN).is_none() {
            return Err(format!(
                "{} has no {MAIN}, which holds the program's app",
                dir.display()
            ));
        }
        Ok(program)
    }

    /// A program from files already in memory, as an editor holds them.
    pub fn from_files(mut files: Vec<SourceFile>) -> Program {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Program { files, web: None }
    }

    pub fn file(&self, path: &str) -> Option<&SourceFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Renders a report with the source line it points at, as the command
    /// line shows it.
    pub fn render(&self, report: &Report, color: bool) -> String {
        let file = &self.files[report.file];
        report.diagnostic.render(&file.path, &file.text, color)
    }
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<SourceFile>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, files)?;
        } else if path.extension().is_some_and(|e| e == "inset") {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let parts: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect();
            files.push(SourceFile {
                path: parts.join("/"),
                text,
            });
        }
    }
    Ok(())
}

fn collect_assets(root: &Path, dir: &Path, assets: &mut Vec<Asset>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_assets(root, &path, assets)?;
        } else {
            let bytes =
                std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let parts: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect();
            assets.push(Asset {
                path: parts.join("/"),
                bytes,
            });
        }
    }
    Ok(())
}
