use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand, Node, Package, PackageId};
use semver::Version;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::env;
use std::fs::File;
use std::io::{BufWriter, ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR envvar not set");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR envvar not set");
    let mut fp = BufWriter::new(File::create(Path::new(&out_dir).join("build_info.rs"))?);

    writeln!(
        &mut fp,
        "pub const BUILD_TIMESTAMP: &str = {:?};",
        OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
    )?;

    let target = env::var("TARGET").expect("TARGET envvar not set");
    writeln!(&mut fp, "pub const TARGET_TRIPLE: &str = {target:?};")?;

    writeln!(
        &mut fp,
        "pub const HOST_TRIPLE: &str = {:?};",
        env::var("HOST").expect("HOST envvar not set"),
    )?;

    let mut features = Vec::new();
    for (key, _) in env::vars_os() {
        if let Some(s) = key.to_str() {
            if let Some(feat) = s.strip_prefix("CARGO_FEATURE_") {
                features.push(feat.to_ascii_lowercase().replace('_', "-"));
            }
        }
    }
    features.sort();
    writeln!(
        &mut fp,
        "pub const FEATURES: &str = {:?};",
        features.join(", ")
    )?;

    let rustc_v = Command::new(env::var("RUSTC").expect("RUSTC envvar not set"))
        .arg("-V")
        .output()?;
    let mut rv = String::from_utf8(rustc_v.stdout)?;
    chomp(&mut rv);

    writeln!(&mut fp, "pub const RUSTC_VERSION: &str = {:?};", rv)?;

    match Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(&manifest_dir)
        .status()
    {
        Ok(rc) if rc.success() => {
            // We are in a Git repository
            let output = Command::new("git")
                .arg("rev-parse")
                .arg("HEAD")
                .current_dir(&manifest_dir)
                .output()?;
            let mut revision = String::from_utf8(output.stdout)?;
            chomp(&mut revision);
            writeln!(
                &mut fp,
                "pub const GIT_COMMIT_HASH: Option<&str> = Some({:?});",
                revision
            )?;
        }
        Ok(_) => {
            // We are not in a Git repository
            writeln!(&mut fp, "pub const GIT_COMMIT_HASH: Option<&str> = None;")?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Git doesn't seem to be installed, so assume we're not in a Git
            // repository
            writeln!(&mut fp, "pub const GIT_COMMIT_HASH: Option<&str> = None;")?;
        }
        Err(e) => return Err(e.into()),
    }

    let package = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME envvar not set");
    let deps = normal_dependencies(manifest_dir, &package, &target, features)?;
    writeln!(
        &mut fp,
        "pub const DEPENDENCIES: [(&str, &str); {}] = [",
        deps.len()
    )?;
    for (name, version) in deps {
        writeln!(&mut fp, "    ({name:?}, {:?}),", version.to_string())?;
    }
    writeln!(&mut fp, "];")?;

    fp.flush()?;
    Ok(())
}

fn chomp(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

fn normal_dependencies<P: AsRef<Path>>(
    manifest_dir: P,
    package: &str,
    target: &str,
    features: Vec<String>,
) -> cargo_metadata::Result<Vec<(String, Version)>> {
    let metadata = MetadataCommand::new()
        .manifest_path(&manifest_dir.as_ref().join("Cargo.toml"))
        .features(CargoOpt::NoDefaultFeatures)
        .features(CargoOpt::SomeFeatures(features))
        .other_options(vec![format!("--filter-platform={target}")])
        .exec()?;
    let Some(root_id) = metadata.workspace_members.iter().find(|pkgid| {
        let pkg = package_by_id(&metadata.packages, pkgid);
        pkg.name == package
    }) else {
        panic!("Package {package} not found in metadata");
    };
    let mut dependencies = BTreeMap::new();
    let mut queue = VecDeque::<&PackageId>::from([root_id]);
    let mut seen = HashSet::<&PackageId>::new();
    let nodes = metadata.resolve.unwrap().nodes;
    while let Some(pkgid) = queue.pop_front() {
        let n = node_by_id(&nodes, pkgid);
        for dep in &n.deps {
            if dep
                .dep_kinds
                .iter()
                .any(|dk| dk.kind == DependencyKind::Normal)
                && seen.insert(&dep.pkg)
            {
                queue.push_back(&dep.pkg);
                let pkg = package_by_id(&metadata.packages, &dep.pkg);
                dependencies.insert(pkg.name.clone(), pkg.version.clone());
            }
        }
    }
    Ok(dependencies.into_iter().collect())
}

fn package_by_id<'a>(packages: &'a [Package], pkgid: &'a PackageId) -> &'a Package {
    let Some(pkg) = packages.iter().find(|p| &p.id == pkgid) else {
        panic!("Package ID {pkgid} not found in metadata");
    };
    pkg
}

fn node_by_id<'a>(nodes: &'a [Node], pkgid: &'a PackageId) -> &'a Node {
    let Some(n) = nodes.iter().find(|n| &n.id == pkgid) else {
        panic!("Node with ID {pkgid} not found in metadata");
    };
    n
}
