use anyhow::{bail, Context};
use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand, Node, Package, PackageId};
use semver::Version;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::fs::File;
use std::io::{BufWriter, ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let manifest_dir = getenv("CARGO_MANIFEST_DIR")?;
    let out_dir = getenv("OUT_DIR")?;
    let mut fp = BufWriter::new(File::create(Path::new(&out_dir).join("build_info.rs"))?);

    writeln!(
        &mut fp,
        "pub(crate) const BUILD_TIMESTAMP: &str = {:?};",
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("formatting a datetime as RFC3339 should not fail"),
    )?;

    let target = getenv("TARGET")?;
    writeln!(
        &mut fp,
        "pub(crate) const TARGET_TRIPLE: &str = {target:?};"
    )?;
    writeln!(
        &mut fp,
        "pub(crate) const HOST_TRIPLE: &str = {:?};",
        getenv("HOST")?
    )?;

    let mut features = Vec::new();
    for (key, _) in env::vars_os() {
        if let Some(feat) = key.to_str().and_then(|s| s.strip_prefix("CARGO_FEATURE_")) {
            features.push(feat.to_ascii_lowercase().replace('_', "-"));
        }
    }
    features.sort();
    writeln!(
        &mut fp,
        "pub(crate) const FEATURES: &str = {:?};",
        features.join(", ")
    )?;

    let rv = Command::new(getenv("RUSTC")?)
        .arg("-V")
        .output()
        .context("failed to get compiler version")?;
    if !rv.status.success() {
        bail!("compiler version command was not successful: {}", rv.status);
    }
    let mut rv = String::from_utf8(rv.stdout).context("compiler version output was not UTF-8")?;
    chomp(&mut rv);
    writeln!(&mut fp, "pub(crate) const RUSTC_VERSION: &str = {rv:?};")?;

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
                .output()
                .context("failed to run `git rev-parse HEAD`")?;
            if !output.status.success() {
                bail!(
                    "`git rev-parse HEAD` command was not successful: {}",
                    output.status
                );
            }
            let mut revision = String::from_utf8(output.stdout)
                .context("`git rev-parse HEAD` output was not UTF-8")?;
            chomp(&mut revision);
            writeln!(
                &mut fp,
                "pub(crate) const GIT_COMMIT_HASH: Option<&str> = Some({revision:?});"
            )?;
        }
        Ok(_) => {
            // We are not in a Git repository
            writeln!(
                &mut fp,
                "pub(crate) const GIT_COMMIT_HASH: Option<&str> = None;"
            )?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Git doesn't seem to be installed, so assume we're not in a Git
            // repository
            writeln!(
                &mut fp,
                "pub(crate) const GIT_COMMIT_HASH: Option<&str> = None;"
            )?;
        }
        Err(e) => return Err(e).context("failed to run `git rev-parse --git-dir`"),
    }

    let package = getenv("CARGO_PKG_NAME")?;
    let deps = normal_dependencies(manifest_dir, &package, &target, features)?;
    writeln!(
        &mut fp,
        "pub(crate) const DEPENDENCIES: [(&str, &str); {}] = [",
        deps.len()
    )?;
    for (name, version) in deps {
        writeln!(&mut fp, "    ({name:?}, {:?}),", version.to_string())?;
    }
    writeln!(&mut fp, "];")?;

    fp.flush()?;
    Ok(())
}

fn getenv(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("{name} envvar not set"))
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
) -> anyhow::Result<Vec<(String, Version)>> {
    let metadata = MetadataCommand::new()
        .manifest_path(manifest_dir.as_ref().join("Cargo.toml"))
        .features(CargoOpt::NoDefaultFeatures)
        .features(CargoOpt::SomeFeatures(features))
        .other_options(vec![format!("--filter-platform={target}")])
        .exec()
        .context("failed to get Cargo metadata")?;
    let Some(root_id) = metadata.workspace_members.iter().find(|pkgid| {
        package_by_id(&metadata.packages, pkgid).is_ok_and(|pkg| pkg.name == package)
    }) else {
        bail!("Package {package} not found in metadata");
    };
    let mut dependencies = Vec::new();
    let mut queue = VecDeque::<&PackageId>::from([root_id]);
    let mut seen = HashSet::<&PackageId>::new();
    let nodes = metadata
        .resolve
        .expect("dependencies should be included in metadata")
        .nodes;
    while let Some(pkgid) = queue.pop_front() {
        let n = node_by_id(&nodes, pkgid)?;
        for dep in &n.deps {
            if dep
                .dep_kinds
                .iter()
                .any(|dk| dk.kind == DependencyKind::Normal)
                && seen.insert(&dep.pkg)
            {
                queue.push_back(&dep.pkg);
                let pkg = package_by_id(&metadata.packages, &dep.pkg)?;
                dependencies.push((pkg.name.clone(), pkg.version.clone()));
            }
        }
    }
    dependencies.sort_unstable();
    Ok(dependencies)
}

fn package_by_id<'a>(packages: &'a [Package], pkgid: &'a PackageId) -> anyhow::Result<&'a Package> {
    let Some(pkg) = packages.iter().find(|p| &p.id == pkgid) else {
        bail!("Package ID {pkgid} not found in metadata");
    };
    Ok(pkg)
}

fn node_by_id<'a>(nodes: &'a [Node], pkgid: &'a PackageId) -> anyhow::Result<&'a Node> {
    let Some(n) = nodes.iter().find(|n| &n.id == pkgid) else {
        bail!("Node with ID {pkgid} not found in metadata");
    };
    Ok(n)
}
