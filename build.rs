// Copyright (C) 2020-2026 Andy Kurnia.

// compiles macondo.proto, then stamps every build with the commit it was built
// from, the way wolges's own build.rs stamps wolges: <the commit's time in
// UTC>-<its short sha>, with "-dirty" when what builds it has uncommitted
// changes, then the target. Where git cannot say, as in a copy of the source
// without its repository, the commit reads "unknown". The stamp never stops a
// build.

use std::path::Path;
use std::process::Command;

// what a build is made of: an uncommitted change to any of these makes it
// dirty, wolges's too, which git sees as one when wolges is checked out at a
// commit other than the one this commit pins, or a file of it is edited
const BUILT_FROM: &[&str] = &[".cargo", "Cargo.lock", "Cargo.toml", "build.rs", "src"];
const WOLGES: &str = "wolges";

// git's output, trimmed, when it succeeds; git takes no optional lock, so git
// status leaves the index as it was
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .env("TZ", "UTC0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned())
}

// a path to watch, only if it exists: cargo reruns a build script every build
// for one that does not
fn rerun_if_changed(path: &Path) {
    if path.exists() {
        println!("cargo::rerun-if-changed={}", path.display());
    }
}

// watch source files individually so ignored data cannot invalidate a build
fn watch_inputs(repo: &str, own_repo: bool) {
    let root = Path::new(repo);
    if own_repo {
        let mut args = vec![
            "-C",
            repo,
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ];
        args.extend(BUILT_FROM);
        args.push(".gitignore");
        if let Some(files) = git(&args) {
            for file in files.split_terminator('\0') {
                rerun_if_changed(&root.join(file));
            }
            return;
        }
    }
    for file in BUILT_FROM {
        rerun_if_changed(&root.join(file));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&["src/macondo.proto"], &["src/"])?;
    println!("cargo::rerun-if-changed=build.rs");
    rerun_if_changed(Path::new("src/macondo.proto"));
    // only this package's own repository says what it was built from, never one
    // that holds it further up
    let own_repo = std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .and_then(|dir| Path::new(&dir).canonicalize().ok())
        .is_some_and(|dir| {
            git(&["rev-parse", "--show-toplevel"])
                .and_then(|top| Path::new(&top).canonicalize().ok())
                .is_some_and(|top| top == dir)
        });
    let commit = if own_repo {
        // a commit, a checkout, or a reset moves HEAD: git rewrites HEAD or
        // adds to logs/HEAD, or, where refs live in reftable/, rewrites
        // tables.list there. Not the index: git status and staging rewrite it
        // without changing what builds, and each rerun rebuilds both programs
        if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
            for file in ["HEAD", "logs/HEAD", "reftable/tables.list"] {
                rerun_if_changed(&Path::new(&git_dir).join(file));
            }
        }
        // wolges's own HEAD too, in its git directory: wolges checked out at
        // another commit makes this build dirty
        let wolges_repo = git(&["-C", WOLGES, "rev-parse", "--show-prefix"])
            .is_some_and(|prefix| prefix.is_empty());
        if wolges_repo && let Some(git_dir) = git(&["-C", WOLGES, "rev-parse", "--git-dir"]) {
            for file in ["HEAD", "logs/HEAD", "reftable/tables.list"] {
                rerun_if_changed(&Path::new(WOLGES).join(&git_dir).join(file));
            }
        }
        watch_inputs(".", true);
        watch_inputs(WOLGES, wolges_repo);
        (|| {
            let time = git(&[
                "log",
                "-1",
                "--format=%cd",
                "--date=format-local:%Y%m%dT%H%M%SZ",
            ])?;
            let sha = git(&["rev-parse", "--short", "HEAD"])?;
            let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
            status.extend(BUILT_FROM);
            status.push(WOLGES);
            let dirty = if git(&status)?.is_empty() {
                ""
            } else {
                "-dirty"
            };
            Some(format!("{time}-{sha}{dirty}"))
        })()
    } else {
        None
    };
    println!(
        "cargo::rustc-env=OMGBOT_VERSION={} {}",
        commit.as_deref().unwrap_or("unknown"),
        std::env::var("TARGET").unwrap_or_default(),
    );
    Ok(())
}
