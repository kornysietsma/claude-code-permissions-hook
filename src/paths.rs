use std::fs;
use std::path::{Component, Path, PathBuf};

/// Expands a configured `under` directory: `~` is the home directory, `{cwd}` the payload cwd
pub fn expand_dir(dir: &str, home: &Path, cwd: &Path) -> PathBuf {
    let dir = dir.replace("{cwd}", &cwd.to_string_lossy());
    match dir.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(dir),
    }
}

/// The path the OS would actually reach, for paths that may not exist yet.
///
/// The longest existing prefix is canonicalised first, so symlinks are followed before any
/// `..` is applied (as the OS does); only the not-yet-existing remainder is cleaned up
/// textually, and it cannot contain symlinks.
pub fn resolve(path: &Path, cwd: &Path) -> PathBuf {
    let path = cwd.join(path);
    let (base, remainder) = path
        .ancestors()
        .find_map(|ancestor| {
            let base = fs::canonicalize(ancestor).ok()?;
            let remainder = path.strip_prefix(ancestor).ok()?;
            Some((base, remainder.to_path_buf()))
        })
        .unwrap_or_else(|| (PathBuf::from("/"), path.clone()));
    append_lexically(base, &remainder)
}

fn append_lexically(base: PathBuf, remainder: &Path) -> PathBuf {
    remainder.components().fold(base, |mut path, component| {
        match component {
            Component::ParentDir => {
                path.pop();
            }
            Component::Normal(part) => path.push(part),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
        path
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn expand_dir_handles_home_and_cwd() {
        let home = Path::new("/home/me");
        let cwd = Path::new("/work/proj");

        assert_eq!(expand_dir("~", home, cwd), PathBuf::from("/home/me"));
        assert_eq!(
            expand_dir("~/prj", home, cwd),
            PathBuf::from("/home/me/prj")
        );
        assert_eq!(expand_dir("~other/x", home, cwd), PathBuf::from("~other/x"));
        assert_eq!(
            expand_dir("{cwd}/src", home, cwd),
            PathBuf::from("/work/proj/src")
        );
    }

    #[test]
    fn remainder_is_cleaned_up_without_escaping_root() {
        assert_eq!(
            append_lexically(PathBuf::from("/a/b"), Path::new("new/../../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(
            append_lexically(PathBuf::from("/"), Path::new("../../etc")),
            PathBuf::from("/etc")
        );
    }
}
