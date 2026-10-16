//! Scaffold a new person's instance folder from the commented templates in `templates/`.
//! Ported from the Node prototype (tag v0.2.0).

use crate::util::{expand_home, slug};
use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

pub struct InitResult {
    pub dir: PathBuf,
    pub instance: String,
    pub files: Vec<String>,
}

/// Live configuration directories an instance may never be scaffolded into.
const PROTECTED: [&str; 4] = [
    "~/.claude",
    "~/Library/LaunchAgents",
    "~/Library/LaunchDaemons",
    "~/.ssh",
];

const SETUP_TEMPLATE: &str = include_str!("../templates/setup.template.yaml");
const POLICIES_TEMPLATE: &str = include_str!("../templates/policies.template.yaml");
const OVERRIDES_TEMPLATE: &str = include_str!("../templates/overrides.template.yaml");
const DENYLIST_TEMPLATE: &str = include_str!("../templates/lint-denylist.template.txt");

/// `path.resolve`: absolute against the working directory, `.` and `..` removed lexically.
pub(crate) fn resolve(p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The path with symlinks resolved as far as it exists (the rest appended as written).
fn real_path(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            return rest.iter().rev().fold(c, |acc, r| acc.join(r));
        }
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Write setup.yaml, policies.yaml, overrides.yaml and lint-denylist.txt into `dir`.
/// Refuses protected directories under `home` and existing files unless `force`.
pub fn init(dir: &Path, name: Option<&str>, force: bool, home: &Path) -> Result<InitResult> {
    let abs = resolve(dir);
    let abs_real = real_path(&abs);
    for p in PROTECTED {
        let prot = resolve(Path::new(&expand_home(p, home)));
        let prot_real = real_path(&prot);
        let inside = [&abs, &abs_real]
            .iter()
            .any(|a| a.starts_with(&prot) || a.starts_with(&prot_real));
        if inside {
            bail!("refusing to scaffold inside {p}");
        }
    }
    let base = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let instance = slug(name.filter(|n| !n.is_empty()).unwrap_or(&base), 40);
    let files: [(&str, String); 4] = [
        (
            "setup.yaml",
            SETUP_TEMPLATE.replacen("{{instance}}", &instance, 1),
        ),
        ("policies.yaml", POLICIES_TEMPLATE.to_string()),
        ("overrides.yaml", OVERRIDES_TEMPLATE.to_string()),
        ("lint-denylist.txt", DENYLIST_TEMPLATE.to_string()),
    ];
    std::fs::create_dir_all(&abs).with_context(|| format!("creating {}", abs.display()))?;
    // Anything at the name clashes, a dangling symlink included (`exists()` would miss it).
    let clash: Vec<&str> = files
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| abs.join(f).symlink_metadata().is_ok())
        .collect();
    if !clash.is_empty() && !force {
        bail!(
            "{} already has {} (use --force to overwrite)",
            abs.display(),
            clash.join(", ")
        );
    }
    // Even with --force, never write through a symlink: its target can be anywhere, such as
    // ~/.claude. Checked for all four before the first write, so a refusal writes nothing.
    for (f, _) in &files {
        let p = abs.join(f);
        if p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            bail!("refusing to write through symlink {}", p.display());
        }
    }
    for (f, text) in &files {
        let p = abs.join(f);
        std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))?;
    }
    for d in ["importers", "checks"] {
        let p = abs.join(d);
        std::fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
    }
    Ok(InitResult {
        dir: abs,
        instance,
        files: files.iter().map(|(f, _)| f.to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::normalize;
    use crate::schema::validate;
    use crate::yaml::read_yaml;
    use std::fs;

    const FILES: [&str; 4] = [
        "setup.yaml",
        "policies.yaml",
        "overrides.yaml",
        "lint-denylist.txt",
    ];

    #[test]
    fn init_writes_four_files_and_plugin_dirs() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let dir = t.path().join("work").join("Alex Setup");
        let r = init(&dir, None, false, &home).unwrap();
        assert_eq!(r.instance, "alex-setup");
        assert_eq!(r.files, FILES.to_vec());
        assert_eq!(r.dir, dir);
        for f in FILES {
            assert!(dir.join(f).is_file(), "{f}");
        }
        assert!(dir.join("importers").is_dir());
        assert!(dir.join("checks").is_dir());
        let setup = fs::read_to_string(dir.join("setup.yaml")).unwrap();
        assert!(setup.contains("instance: alex-setup"));
        assert!(!setup.contains("{{instance}}"));
        assert_eq!(
            fs::read_to_string(dir.join("lint-denylist.txt")).unwrap(),
            include_str!("../templates/lint-denylist.template.txt")
        );
        let v = validate(&normalize(read_yaml(&dir.join("setup.yaml")).unwrap()));
        assert!(v.ok, "{:?}", v.errors);
        assert!(!home.exists(), "init must not touch the home");
    }

    #[test]
    fn init_slugs_the_name_and_resolves_dots() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let dir = t
            .path()
            .join("a")
            .join("..")
            .join("b")
            .join(".")
            .join("inst");
        let r = init(&dir, Some("Alex's Lab!"), false, &home).unwrap();
        assert_eq!(r.instance, "alex-s-lab");
        assert_eq!(r.dir, t.path().join("b").join("inst"));
        assert!(!t.path().join("a").exists());
        let setup = fs::read_to_string(r.dir.join("setup.yaml")).unwrap();
        assert!(setup.contains("instance: alex-s-lab"));
    }

    #[test]
    fn init_refuses_protected_dirs_under_home() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        for (sub, shown) in [
            (".claude", "~/.claude"),
            (".claude/instance", "~/.claude"),
            ("Library/LaunchAgents", "~/Library/LaunchAgents"),
            ("Library/LaunchDaemons/x", "~/Library/LaunchDaemons"),
            (".ssh/keys", "~/.ssh"),
        ] {
            let e = init(&home.join(sub), None, false, &home)
                .err()
                .expect("refused");
            assert_eq!(
                e.to_string(),
                format!("refusing to scaffold inside {shown}")
            );
        }
        assert!(!home.exists(), "nothing may be created under the home");
        // A sibling whose name merely starts with a protected name is fine.
        assert!(init(&home.join(".claude-notes"), None, false, &home).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn init_refuses_a_symlink_into_a_protected_dir() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(home.join(".claude"), &link).unwrap();
        let e = init(&link.join("inst"), None, false, &home)
            .err()
            .expect("refused");
        assert_eq!(e.to_string(), "refusing to scaffold inside ~/.claude");
        assert!(!home.join(".claude").join("inst").exists());
    }

    #[test]
    fn init_refuses_a_clash_without_force() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let dir = t.path().join("inst");
        init(&dir, None, false, &home).unwrap();
        fs::write(dir.join("setup.yaml"), "edited by hand").unwrap();
        let e = init(&dir, None, false, &home).err().expect("clash");
        assert_eq!(
            e.to_string(),
            format!(
                "{} already has setup.yaml, policies.yaml, overrides.yaml, lint-denylist.txt (use --force to overwrite)",
                dir.display()
            )
        );
        assert_eq!(
            fs::read_to_string(dir.join("setup.yaml")).unwrap(),
            "edited by hand"
        );
        let r = init(&dir, None, true, &home).unwrap();
        assert_eq!(r.files.len(), 4);
        assert!(fs::read_to_string(dir.join("setup.yaml"))
            .unwrap()
            .contains("instance: inst"));

        let other = t.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("lint-denylist.txt"), "x").unwrap();
        let e = init(&other, None, false, &home).err().expect("clash");
        assert!(
            e.to_string()
                .ends_with(" already has lint-denylist.txt (use --force to overwrite)"),
            "{e}"
        );
        assert!(!other.join("setup.yaml").exists());
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[cfg(unix)]
    #[test]
    fn init_treats_a_dangling_symlink_as_a_clash() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let outside = t.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let dir = t.path().join("inst");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(outside.join("planted.yaml"), dir.join("setup.yaml")).unwrap();
        let e = init(&dir, None, false, &home).err().expect("clash");
        assert_eq!(
            e.to_string(),
            format!(
                "{} already has setup.yaml (use --force to overwrite)",
                dir.display()
            )
        );
        let e = init(&dir, None, true, &home).err().expect("refused");
        assert_eq!(
            e.to_string(),
            format!(
                "refusing to write through symlink {}",
                dir.join("setup.yaml").display()
            )
        );
        assert!(names_in(&outside).is_empty(), "nothing written outside");
        assert_eq!(names_in(&dir), vec!["setup.yaml"], "nothing written inside");
    }

    #[cfg(unix)]
    #[test]
    fn init_refuses_to_write_through_a_symlink_even_with_force() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let outside = t.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("live.yaml"), "keep me").unwrap();
        let dir = t.path().join("inst");
        init(&dir, None, false, &home).unwrap();
        fs::remove_file(dir.join("policies.yaml")).unwrap();
        std::os::unix::fs::symlink(outside.join("live.yaml"), dir.join("policies.yaml")).unwrap();
        fs::write(dir.join("setup.yaml"), "edited by hand").unwrap();
        let e = init(&dir, None, true, &home).err().expect("refused");
        assert_eq!(
            e.to_string(),
            format!(
                "refusing to write through symlink {}",
                dir.join("policies.yaml").display()
            )
        );
        assert_eq!(
            fs::read_to_string(outside.join("live.yaml")).unwrap(),
            "keep me"
        );
        assert_eq!(names_in(&outside), vec!["live.yaml"]);
        assert_eq!(
            fs::read_to_string(dir.join("setup.yaml")).unwrap(),
            "edited by hand",
            "no file is written when one is refused"
        );
    }
}
