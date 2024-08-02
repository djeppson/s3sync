#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use clap::Parser;

const DEFAULT_EVENT_WINDOW_SECONDS: u64 = 5;

#[::tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let (tx, rx) = std::sync::mpsc::channel();

    let agents = s3sync::Agents::try_from(ux::Cli::parse())?.agents;
    let _ = agents
        .iter()
        .map(|agent| agent.watcher(tx.clone()));

    // TODO: send events to each agent
    let s3sync = agents.first().unwrap();
    for events in rx.into_iter().flatten() {
        for event in events {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                println!("Process: {event:?}");
                s3sync.process_file(&event.path).await?;
            }
        }
    }
    Ok(())
}

fn window_seconds_range(s: &str) -> Result<u64, String> {
    clap_num::number_range(s, 1, 3600)
}

mod ux {
    use std::path::PathBuf;

    use clap::Parser;
    use regex::Regex;

    use crate::{window_seconds_range, DEFAULT_EVENT_WINDOW_SECONDS};

    #[derive(Parser, Debug)]
    #[command(about, long_about = None)]
    pub struct Cli {
        /// Local file path to sync
        #[arg(long, short, default_value = std::env::current_dir().unwrap().into_os_string())]
        pub path: PathBuf,
        /// S3 bucket to sync with
        #[arg(long, short)]
        pub bucket: String,
        /// Regex filter to match events
        #[arg(long, default_value_t = Regex::new(".*").unwrap())]
        pub pattern: Regex,
        /// AWS credential profile to use
        #[arg(long)]
        pub profile: Option<String>,
        /// AWS region override
        #[arg(long)]
        pub region: Option<String>,
        /// Delete source file after successful upload
        #[arg(long, short, default_value_t = false)]
        pub delete: bool,
        /// Recursively sync the provided path
        #[arg(short, long, default_value_t = false)]
        pub recursive: bool,
        /// Number of seconds to aggregate events
        #[arg(short, long, value_parser=window_seconds_range, default_value_t = DEFAULT_EVENT_WINDOW_SECONDS)]
        pub window: u64,
        #[arg(long)]
        pub config: Option<PathBuf>,
    }
}

mod s3sync {
    use std::path::{Path, PathBuf};

    use anyhow::anyhow;
    use aws_config::{default_provider::region::DefaultRegionChain, Region};
    use aws_sdk_s3 as s3;
    use derive_builder::Builder;
    use notify_debouncer_mini::{
        new_debouncer,
        notify::{RecursiveMode, Watcher},
        DebounceEventHandler, Debouncer,
    };
    use regex::Regex;
    use s3::primitives::ByteStream;
    use serde::Deserialize;

    use crate::{ux::Cli, DEFAULT_EVENT_WINDOW_SECONDS};

    #[derive(Deserialize, Debug)]
    pub struct Agents {
        pub agents: Vec<Agent>,
    }

    impl TryFrom<Cli> for Agents {
        type Error = anyhow::Error;

        fn try_from(value: Cli) -> Result<Self, Self::Error> {
            if let Some(filename) = value.config {
                let contents = std::fs::read_to_string(filename)?;
                Ok(toml::from_str(&contents)?)
            } else {
                let agent = Agent {
                    local_path: value.path,
                    bucket_name: value.bucket,
                    pattern: Some(value.pattern),
                    profile_name: value.profile,
                    region_name: value.region,
                    delete: Some(value.delete),
                    recursive: Some(value.recursive),
                    window: Some(value.window),
                };                
                Ok(Self { agents: vec![agent] })
            }
        }
    }

    #[derive(Builder, Deserialize, Debug, Clone)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct Agent {
        local_path: PathBuf,
        bucket_name: String,
        #[serde(with = "serde_regex", default)]
        pattern: Option<Regex>,
        profile_name: Option<String>,
        region_name: Option<String>,
        delete: Option<bool>,
        recursive: Option<bool>,
        window: Option<u64>,
    }

    impl Agent {
        pub const fn local_path(&self) -> &PathBuf {
            &self.local_path
        }
        pub fn recursive_mode(&self) -> RecursiveMode {
            if self.recursive.unwrap_or(false) {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            }
        }
        pub fn delete(&self) -> bool {
            self.delete.unwrap_or(false)
        }
        pub fn window(&self) -> u64 {
            self.window.unwrap_or(DEFAULT_EVENT_WINDOW_SECONDS)
        }
        pub fn watcher<F: DebounceEventHandler>(&self, tx: F) -> Debouncer<impl Watcher> {
            let mut watcher =
                new_debouncer(std::time::Duration::from_secs(self.window()), tx).unwrap();
            println!("Watch {:?}", self.local_path());
            watcher
                .watcher()
                .watch(self.local_path(), self.recursive_mode())
                .unwrap();
            watcher
        }
        pub async fn process_file(&self, file: &Path) -> Result<(), anyhow::Error> {
            if let Ok(key) = self.object_key(file) {
                println!("Uploading: {key}");
                self.upload_file(file, &key).await?;
                println!("Upload successful: {file:?}");
                if self.delete() {
                    std::fs::remove_file(file)?;
                    println!("Cleaned-up file {file:?}");
                }
            }
            Ok(())
        }
        fn object_key(&self, path: &Path) -> Result<String, anyhow::Error> {
            let key = path
                .strip_prefix(self.local_path())?
                .to_str()
                .ok_or_else(|| anyhow!("Non-unicode path"))?;
            let applied_pattern = self
                .pattern
                .clone()
                .unwrap_or_else(|| Regex::new(r".*").unwrap());
            if applied_pattern.is_match(key) {
                Ok(String::from(key))
            } else {
                Err(anyhow::Error::msg("Does not match pattern"))
            }
        }
        async fn upload_file(&self, path: &Path, key: &str) -> Result<(), anyhow::Error> {
            let body = ByteStream::from_path(path).await?;
            let profile_name = self
                .profile_name
                .clone()
                .unwrap_or_else(|| String::from("default"));
            let region = self.region_name.clone().map(Region::new).or({
                DefaultRegionChain::builder()
                    .profile_name(&profile_name)
                    .build()
                    .region()
                    .await
            });
            let sdk_config = aws_config::from_env()
                .region(region)
                .profile_name(profile_name)
                .load()
                .await;
            let client = s3::Client::new(&sdk_config);
            client
                .put_object()
                .bucket(self.bucket_name.clone())
                .key(key)
                .body(body)
                .send()
                .await?;
            Ok(())
        }
    }
}
