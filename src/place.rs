//! Where a conversation happened: the project it started in and the git repository holding it.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

/// A starting folder's project and repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The repository's name plus the path inside it, else the folder's name or a bracketed kind.
    pub project: String,
    /// The repository's name; empty outside one, and `(unknown)` when that cannot be told.
    pub repo: String,
}

/// The project and repository of a session whose folder is not recorded, or no longer exists
/// outside a repository.
pub const UNKNOWN: &str = "(unknown)";

const TEMPORARY: [&str; 4] = [
    "/tmp/",
    "/private/tmp/",
    "/var/folders/",
    "/private/var/folders/",
];

/// The repository's name, from a `.git` folder or a worktree's `.git` file, when there is one.
fn repository(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let main = if git.is_dir() {
        root.to_path_buf()
    } else {
        // A worktree's file reads `gitdir: <main>/.git/worktrees/<name>`, absolute or relative to
        // the worktree; a submodule's points elsewhere and is its own repository.
        let pointer = fs::read(&git).ok()?;
        let target = pointer.trim_ascii().strip_prefix(b"gitdir:")?.trim_ascii();
        let target = root.join(OsStr::from_bytes(target));
        let target = fs::canonicalize(&target).unwrap_or(target);
        let worktrees = target.parent()?;
        let dot_git = worktrees.parent()?;
        if worktrees.file_name() == Some(OsStr::new("worktrees"))
            && dot_git.file_name() == Some(OsStr::new(".git"))
        {
            dot_git.parent()?.to_path_buf()
        } else {
            root.to_path_buf()
        }
    };
    Some(main.file_name()?.to_string_lossy().into_owned())
}

/// The folder with symbolic links resolved, through its nearest existing parent once it is removed.
fn resolved(folder: &Path) -> PathBuf {
    for ancestor in folder.ancestors() {
        if let Ok(real) = fs::canonicalize(ancestor) {
            let rest = folder.strip_prefix(ancestor).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            };
        }
    }
    folder.to_path_buf()
}

/// The folder a session started in, with worktree folders replaced by the repository they copy.
///
/// A Claude Code worktree sits in its repository at `.claude/worktrees/<name>`, so dropping that
/// part gives the repository's own folder. A Codex worktree at `.codex/worktrees/<id>/<repo>` names
/// no repository folder and is kept, to be read from disk while it exists.
fn unworktreed(cwd: &str) -> String {
    match cwd.split_once("/.claude/worktrees/") {
        Some((repository, rest)) => match rest.split_once('/') {
            Some((_, inside)) => format!("{repository}/{inside}"),
            None => repository.to_string(),
        },
        None => cwd.to_string(),
    }
}

/// Place a session that started in `cwd`, reading `.git` entries on disk; `home` is never a
/// repository, so a dotfiles repository does not swallow every project.
pub fn place(cwd: &str, home: &Path) -> Place {
    let cwd = unworktreed(cwd.trim_end_matches('/'));
    let named = |project: &str| Place {
        project: project.to_string(),
        repo: String::new(),
    };
    let unknown = Place {
        project: UNKNOWN.to_string(),
        repo: UNKNOWN.to_string(),
    };
    if cwd.is_empty() {
        return unknown;
    }
    let home = resolved(home);
    let home = home.as_path();
    let folder = resolved(Path::new(&cwd));
    let folder = folder.as_path();
    let cwd = folder.to_string_lossy().into_owned();
    for root in folder.ancestors() {
        if root == home || root.parent().is_none() {
            break;
        }
        if let Some(repo) = repository(root) {
            let inside = folder.strip_prefix(root).unwrap_or(Path::new(""));
            let project = if inside.as_os_str().is_empty() {
                repo.clone()
            } else {
                Path::new(&repo).join(inside).to_string_lossy().into_owned()
            };
            return Place { project, repo };
        }
    }
    // A removed folder was in some repository or none; which, nothing on disk says any more.
    let repo = if folder.exists() { "" } else { UNKNOWN };
    let temporary = TEMPORARY
        .iter()
        .any(|prefix| format!("{cwd}/").starts_with(prefix));
    if temporary && !folder.starts_with(home) {
        return Place {
            project: "(temporary)".to_string(),
            repo: repo.to_string(),
        };
    }
    if folder == home || folder.starts_with(home.join("Documents/Codex")) {
        return named("(no project)");
    }
    if repo == UNKNOWN {
        return unknown;
    }
    named(
        &folder
            .file_name()
            .map_or(cwd.clone(), |name| name.to_string_lossy().into_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(root: &Path, path: &str) -> PathBuf {
        let folder = root.join(path);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn names_a_project_by_repository_and_path_inside_it() {
        let home = std::env::temp_dir().join(format!("atlas-nano-place-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let at = |path: &str| place(&home.join(path).to_string_lossy(), &home);
        let place_of = |project: &str, repo: &str| Place {
            project: project.to_string(),
            repo: repo.to_string(),
        };
        let repo = folder(&home, "Code/app");
        folder(&repo, ".git");
        folder(&repo, "api/src");
        assert_eq!(at("Code/app"), place_of("app", "app"));
        assert_eq!(at("Code/app/api"), place_of("app/api", "app"));
        assert_eq!(at("Code/app/api/"), place_of("app/api", "app"));

        // A Claude Code worktree counts as its repository, even once it is removed.
        let worktree = folder(&repo, ".claude/worktrees/fix/api");
        fs::write(
            worktree.parent().unwrap().join(".git"),
            format!("gitdir: {}/.git/worktrees/fix\n", repo.display()),
        )
        .unwrap();
        assert_eq!(
            at("Code/app/.claude/worktrees/fix/api"),
            place_of("app/api", "app")
        );
        assert_eq!(
            at("Code/app/.claude/worktrees/gone"),
            place_of("app", "app")
        );

        // A Codex worktree is read from its `.git` file; once removed, its repository is unknown.
        let codex = folder(&home, ".codex/worktrees/a1/app");
        fs::write(
            codex.join(".git"),
            format!("gitdir: {}/.git/worktrees/a1\n", repo.display()),
        )
        .unwrap();
        folder(&codex, "api");
        assert_eq!(
            at(".codex/worktrees/a1/app/api"),
            place_of("app/api", "app")
        );
        assert_eq!(
            at(".codex/worktrees/b2/other/web"),
            place_of(UNKNOWN, UNKNOWN)
        );

        // Outside a repository: a folder's name, or a bracketed kind.
        folder(&home, ".git");
        folder(&home, "Code/notes");
        assert_eq!(at("Code/notes"), place_of("notes", ""));
        assert_eq!(at(""), place_of("(no project)", ""));
        assert_eq!(place("", &home), place_of(UNKNOWN, UNKNOWN));
        // A removed folder outside a repository may have been in one.
        assert_eq!(at("Code/gone"), place_of(UNKNOWN, UNKNOWN));
        assert_eq!(
            at("Documents/Codex/2026-01-01/new-chat"),
            place_of("(no project)", "")
        );
        assert_eq!(
            place("/private/tmp/scratch/review", &home),
            place_of("(temporary)", UNKNOWN)
        );

        // A symbolic link is followed to the folder it names.
        std::os::unix::fs::symlink(repo.join("api"), home.join("alias")).unwrap();
        assert_eq!(at("alias"), place_of("app/api", "app"));

        // A worktree pointer relative to the worktree, as `git worktree add --relative-paths` writes.
        let relative = folder(&repo, "worktrees/rel");
        folder(&repo, ".git/worktrees/rel");
        fs::write(relative.join(".git"), "gitdir: ../../.git/worktrees/rel\n").unwrap();
        assert_eq!(at("Code/app/worktrees/rel"), place_of("app", "app"));
        fs::remove_dir_all(&home).unwrap();
    }
}
