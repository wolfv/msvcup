#![allow(dead_code)]

mod arch;
mod autoenv_cmd;
mod channel_kind;
mod extra;
mod fetch_cmd;
mod install;
mod lock_file;
mod lockfile_parse;
mod manifest;
mod packages;
mod sha;
mod util;
mod zip_extract;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use packages::{
    ManifestUpdate, MsvcupPackage, MsvcupPackageKind, PackageId, PayloadId, get_packages,
    identify_package, identify_payload,
};

#[derive(Parser)]
#[command(name = "msvcup", version, about = "MSVC package installer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all available packages
    List,
    /// List all payloads
    ListPayloads,
    /// Install packages
    Install {
        /// Packages to install (e.g. msvc-14.30.17.6)
        packages: Vec<String>,
        /// Path to lock file
        #[arg(long)]
        lock_file: String,
        /// Manifest update policy
        #[arg(long, value_parser = parse_manifest_update)]
        manifest_update: ManifestUpdate,
        /// Cache directory
        #[arg(long)]
        cache_dir: Option<String>,
    },
    /// Create autoenv directory with executable wrappers
    Autoenv {
        /// Target CPU architecture
        #[arg(long)]
        target_cpu: String,
        /// Output directory
        #[arg(long)]
        out_dir: String,
        /// Packages
        packages: Vec<String>,
    },
    /// Fetch a package URL
    Fetch {
        /// URL to fetch
        url: String,
        /// Cache directory
        #[arg(long)]
        cache_dir: Option<String>,
    },
}

fn parse_manifest_update(s: &str) -> Result<ManifestUpdate, String> {
    match s {
        "off" => Ok(ManifestUpdate::Off),
        "daily" => Ok(ManifestUpdate::Daily),
        "always" => Ok(ManifestUpdate::Always),
        _ => Err(format!(
            "invalid manifest update value '{}', expected 'off', 'daily', or 'always'",
            s
        )),
    }
}

fn parse_msvcup_packages(pkg_strings: &[String]) -> Result<Vec<MsvcupPackage>> {
    let mut pkgs = Vec::new();
    for s in pkg_strings {
        match MsvcupPackage::from_string(s) {
            Ok(pkg) => {
                util::insert_sorted(&mut pkgs, pkg, MsvcupPackage::order);
            }
            Err(e) => bail!("invalid package '{}': {}", s, e),
        }
    }
    Ok(pkgs)
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let client = reqwest::Client::builder().build()?;
    let msvcup_dir = manifest::MsvcupDir::new()?;

    match cli.command {
        Commands::List => list_command(&client, &msvcup_dir).await,
        Commands::ListPayloads => list_payloads_command(&client, &msvcup_dir).await,
        Commands::Install {
            packages: pkg_strings,
            lock_file,
            manifest_update,
            cache_dir,
        } => {
            let pkgs = parse_msvcup_packages(&pkg_strings)?;
            install::install_command(
                &client,
                &msvcup_dir,
                &pkgs,
                &lock_file,
                manifest_update,
                cache_dir.as_deref(),
            )
            .await
        }
        Commands::Autoenv {
            target_cpu,
            out_dir,
            packages: pkg_strings,
        } => {
            let target_cpu = arch::Arch::from_str_exact(&target_cpu)
                .ok_or_else(|| anyhow::anyhow!("invalid --target-cpu '{}'", target_cpu))?;
            let pkgs = parse_msvcup_packages(&pkg_strings)?;
            autoenv_cmd::autoenv_command(&pkgs, target_cpu, &out_dir)
        }
        Commands::Fetch { url, cache_dir } => {
            fetch_cmd::fetch_command(&client, &url, cache_dir.as_deref()).await
        }
    }
}

async fn list_command(client: &reqwest::Client, msvcup_dir: &manifest::MsvcupDir) -> Result<()> {
    let (vsman_path, vsman_content) = manifest::read_vs_manifest(
        client,
        msvcup_dir,
        channel_kind::ChannelKind::Release,
        ManifestUpdate::Off,
    )
    .await?;

    let pkgs = get_packages(vsman_path.to_str().unwrap(), &vsman_content)?;

    let mut msvcup_pkgs: Vec<MsvcupPackage> = Vec::new();
    for (pkg_index, pkg) in pkgs.packages.iter().enumerate() {
        match identify_package(&pkg.id) {
            PackageId::Unknown => {}
            PackageId::Unexpected { .. } => {}
            PackageId::MsvcVersionSomething { .. } => {}
            PackageId::MsvcVersionToolsSomething { .. } => {}
            PackageId::MsvcVersionHostTarget { build_version, .. } => {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Msvc, build_version);
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
            PackageId::Msbuild(version) => {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Msbuild, version);
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
            PackageId::Diasdk => {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Diasdk, pkg.version.clone());
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
            PackageId::Ninja(version) => {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Ninja, version);
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
            PackageId::Cmake(version) => {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Cmake, version);
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
        }

        for payload in pkgs.payloads_from_pkg_index(pkg_index) {
            if identify_payload(&payload.file_name) == PayloadId::Sdk {
                let msvcup_pkg = MsvcupPackage::new(MsvcupPackageKind::Sdk, pkg.version.clone());
                util::insert_sorted(&mut msvcup_pkgs, msvcup_pkg, |a, b| {
                    MsvcupPackage::order(a, b)
                });
            }
        }
    }

    for pkg in &msvcup_pkgs {
        println!("{}", pkg);
    }
    Ok(())
}

async fn list_payloads_command(
    client: &reqwest::Client,
    msvcup_dir: &manifest::MsvcupDir,
) -> Result<()> {
    let (vsman_path, vsman_content) = manifest::read_vs_manifest(
        client,
        msvcup_dir,
        channel_kind::ChannelKind::Release,
        ManifestUpdate::Off,
    )
    .await?;

    let pkgs = get_packages(vsman_path.to_str().unwrap(), &vsman_content)?;

    let mut payload_indices: Vec<usize> = Vec::new();
    for (pkg_index, pkg) in pkgs.packages.iter().enumerate() {
        match pkg.language {
            packages::Language::Neutral | packages::Language::EnUs => {}
            packages::Language::Other => continue,
        }
        let range = pkgs.payload_range_from_pkg_index(pkg_index);
        for pi in range {
            util::insert_sorted(&mut payload_indices, pi, |a, b| {
                let pa = &pkgs.payloads[*a];
                let pb = &pkgs.payloads[*b];
                pa.name_decoded()
                    .cmp(pb.name_decoded())
                    .then_with(|| a.cmp(b))
            });
        }
    }

    for &pi in &payload_indices {
        let pkg_index = pkgs.pkg_index_from_payload_index(pi);
        let payload = &pkgs.payloads[pi];
        let pkg = &pkgs.packages[pkg_index];
        println!("{} ({})", payload.file_name, pkg.id);
    }
    Ok(())
}
