//! Phase 24: the faces install as packages. These are smoke checks over what the
//! packages claim to ship, cheap enough for the normal suite. The heavier
//! `npm pack` / wheel build is opt-in through AIDB_PACKAGING_TESTS=1, because it
//! forces a release build.

mod common;

use std::path::Path;

use common::*;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_cli_installs_and_runs_as_aidb() {
    let manifest = read(&repo_root().join("crates/aidb-cli/Cargo.toml"));
    assert!(
        manifest.contains("name = \"aidb\""),
        "the CLI binary has to be called aidb"
    );
    // And the built binary answers, so `cargo install` gives a working command.
    let usage = cli(&[]);
    assert!(
        !usage.status.success(),
        "a bare invocation is a usage error, not a silent success"
    );
    let text = format!("{}{}", stdout_of(&usage), stderr_of(&usage));
    assert!(
        text.contains("sql"),
        "usage must name the sql command: {text}"
    );
    assert!(text.contains("serve"), "usage must name the serve command");
}

#[test]
fn the_npm_package_ships_the_native_addon_and_nothing_hand_copied() {
    let dir = repo_root().join("bindings/typescript");
    let package: serde_json::Value =
        serde_json::from_str(&read(&dir.join("package.json"))).expect("package.json");
    assert_eq!(package["name"], "aidb");
    let files: Vec<String> = package["files"]
        .as_array()
        .expect("files list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        files.iter().any(|f| f.contains(".node")),
        "the tarball has to carry the addon: {files:?}"
    );
    assert!(files.iter().any(|f| f == "src"));
    // A staged addon exists for this platform, which is what `npm i` would install.
    let staged: Vec<_> = std::fs::read_dir(&dir)
        .expect("bindings/typescript")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".node"))
        .collect();
    assert!(
        !staged.is_empty(),
        "run `npm run build` in bindings/typescript first"
    );
    // The install path must not ask anyone to move a dylib around.
    let readme = read(&dir.join("README.md")).to_lowercase();
    assert!(readme.contains("npm i") || readme.contains("npm install"));
    assert!(
        !readme.contains("copy the dylib") && !readme.contains("cp target/"),
        "the README must not ask for a hand-copied dylib"
    );
}

#[test]
fn the_python_wheel_ships_the_native_module_and_nothing_hand_copied() {
    let dir = repo_root().join("bindings/python");
    let pyproject = read(&dir.join("pyproject.toml"));
    assert!(pyproject.contains("name = \"aidb\""));
    assert!(
        pyproject.contains("aidb_native"),
        "package data has to include the native module"
    );
    let manifest = read(&dir.join("MANIFEST.in"));
    assert!(manifest.contains(".so") && manifest.contains(".dylib"));
    let staged: Vec<_> = std::fs::read_dir(dir.join("aidb"))
        .expect("bindings/python/aidb")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("aidb_native"))
        .collect();
    assert!(
        !staged.is_empty(),
        "run scripts/stage_native.py in bindings/python first"
    );
    let readme = read(&dir.join("README.md")).to_lowercase();
    assert!(readme.contains("pip install"));
    assert!(!readme.contains("copy the dylib") && !readme.contains("cp target/"));
}

#[test]
fn a_packaged_face_opens_the_same_file_the_cli_wrote() {
    // The point of packaging is that the installed artifact is the same engine.
    // The staged addon is what the package would install, so use it directly.
    let tmp = TempDb::new("packaging");
    let path = tmp.path();
    let db = path.to_string_lossy().into_owned();
    let inserted = cli(&[
        "sql",
        &db,
        "SELECT aidb_insert_document('Packaged', 'Installed as a package.', '{}');",
    ]);
    assert!(inserted.status.success(), "{}", stderr_of(&inserted));

    let Some(node) = which("node") else {
        eprintln!("skipping: node is not installed");
        return;
    };
    let index = repo_root().join("bindings/typescript/src/index.mjs");
    let script = format!(
        "import {{ AI }} from {index:?};\n\
         const db = await AI.open({db:?});\n\
         const result = await db.query(\"SELECT title FROM documents\");\n\
         if (result.rows[0]?.[0] !== 'Packaged') throw new Error('wrong file: ' + JSON.stringify(result));\n\
         await db.close();\n\
         console.log('ok');\n"
    );
    let out = std::process::Command::new(node)
        .args(["--input-type=module", "-e", &script])
        .output()
        .expect("run node");
    assert!(
        out.status.success(),
        "node face failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_release_artifacts_build_when_asked() {
    if std::env::var("AIDB_PACKAGING_TESTS").is_err() {
        eprintln!("skipping: set AIDB_PACKAGING_TESTS=1 to build release artifacts");
        return;
    }
    let dir = repo_root().join("bindings/typescript");
    let packed = std::process::Command::new("npm")
        .args(["pack", "--dry-run"])
        .current_dir(&dir)
        .output()
        .expect("npm pack");
    assert!(
        packed.status.success(),
        "npm pack failed: {}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let listing = format!(
        "{}{}",
        String::from_utf8_lossy(&packed.stdout),
        String::from_utf8_lossy(&packed.stderr)
    );
    assert!(
        listing.contains(".node"),
        "the tarball has no addon: {listing}"
    );
}

fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}
