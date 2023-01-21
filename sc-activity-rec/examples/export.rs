use chrono::{TimeZone, Utc};
use clap::Parser;
use git2::{Oid, Repository};
use sc_activity_rec::{get_activities, Activities};
use std::collections::VecDeque;
use std::fs;
use std::fs::create_dir_all;
use std::path::PathBuf;

/// Exports changes in the SpriteCollab repo as JSON.
///
/// The currently checked out commit in the repo at `path` is used for credit
/// lookups for all commits after May 7th 2022.
/// Before that the individual commits are inspected.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Path to the repo
    #[arg(short, long)]
    repo: PathBuf,

    /// Start after commit (default: start with root)
    #[arg(short, long)]
    after: Option<String>,

    /// Up including to commit (default: HEAD)
    #[arg(short, long)]
    to: Option<String>,

    /// Output JSON path.
    #[arg(short, long)]
    out: PathBuf,

    /// If specified, write all asset files out to this path.
    /// File names are Git object ID + file extension.
    #[arg(long)]
    asset_out: Option<PathBuf>,
}

fn main() -> Result<(), anyhow::Error> {
    pretty_env_logger::init();
    let args = Args::parse();

    if !args.repo.join(".git").exists() {
        panic!("--path does not point to a Git repository.")
    }
    let repo = Repository::open(args.repo)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["master"], None, None)?;
    let reference = repo.find_reference("FETCH_HEAD")?;

    let mut walk = repo.revwalk()?;
    walk.push(reference.peel_to_commit()?.id())?;
    if let Some(after) = &args.after {
        walk.hide(Oid::from_str(after)?)?;
    }

    let head_commit = repo.head()?.peel_to_commit()?.id();

    println!("Collecting assets.");
    let walk_vec = walk.collect::<Result<VecDeque<_>, _>>()?;
    let mut activities = Vec::with_capacity(walk_vec.len());
    for commit in walk_vec.into_iter().rev() {
        println!(
            "[{}] Processing commit {}...",
            Utc.timestamp(repo.find_commit(commit).unwrap().time().seconds(), 0),
            commit
        );
        activities.push(get_activities(&repo, commit, head_commit)?)
    }

    if let Some(asset_out) = &args.asset_out {
        println!("Writing out assets");
        create_dir_all(asset_out).expect("Expected to create/have asset output directory");
        for acts in &activities {
            for act in acts.acts() {
                if act.action().has_content() {
                    for file in act.asset().files() {
                        let contents = file
                            .contents(&repo)
                            .expect("Expected to read file contents.")
                            .expect("Expected to have file contents.");
                        let Some((_, file_ext)) = file.file_name.split_once('.') else {
                                panic!("Invalid asset file extension.");
                        };
                        let out_name = asset_out.join(format!(
                            "{}.{}",
                            file.oid.as_ref().expect("Expected to have file oid."),
                            file_ext
                        ));
                        if !out_name.exists() {
                            fs::write(out_name, contents).expect("Unable to write file");
                        }
                    }
                }
            }
        }
    }

    println!(
        "Writing out JSON to {}.",
        args.out.as_os_str().to_string_lossy()
    );
    fs::write(
        args.out,
        serde_json::to_string(
            &activities
                .iter()
                .flat_map(Activities::export)
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
    .expect("Unable to write file");

    Ok(())
}
