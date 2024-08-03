#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use clap::Parser;

const DEFAULT_EVENT_WINDOW_SECONDS: u64 = 5;

#[::tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let (tx, rx) = std::sync::mpsc::channel();

    let manager = s3sync::Manager::try_from(ux::Cli::parse())?;
    // Need a variable name to get the watchers to run
    let _watchers = manager
        .agents
        .iter()
        .map(|agent| agent.watcher().watch(tx.clone()))
        .collect::<Vec<_>>();

    for events in rx.into_iter().flatten() {
        for event in events {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                manager.process_event(&event).await?;
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
        notify::{FsEventWatcher, RecursiveMode},
        DebounceEventHandler, DebouncedEvent, Debouncer,
    };
    use regex::Regex;
    use s3::primitives::ByteStream;
    use serde::Deserialize;

    use crate::{ux::Cli, DEFAULT_EVENT_WINDOW_SECONDS};

    #[derive(Deserialize, Debug)]
    pub struct Manager {
        pub agents: Vec<Agent>,
    }

    impl Manager {
        pub async fn process_event(&self, event: &DebouncedEvent) -> Result<(), anyhow::Error> {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                println!("Process: {event:?}");
                for agent in &self.agents {
                    agent.process_file(&event.path).await?;
                }
            }
            Ok(())
        }
    }

    impl TryFrom<Cli> for Manager {
        type Error = anyhow::Error;

        fn try_from(value: Cli) -> Result<Self, Self::Error> {
            if let Some(filename) = value.config {
                let contents = std::fs::read_to_string(filename)?;
                Ok(toml::from_str(&contents)?)
            } else {
                let watcher = AgentWatcher {
                    local_path: value.path,
                    pattern: Some(value.pattern),
                    recursive: Some(value.recursive),
                    window: Some(value.window),
                };
                let agent = Agent {
                    watcher,
                    bucket_name: value.bucket,
                    profile_name: value.profile,
                    region_name: value.region,
                    delete: Some(value.delete),
                };
                Ok(Self {
                    agents: vec![agent],
                })
            }
        }
    }

    #[derive(Builder, Deserialize, Debug, Clone)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct AgentWatcher {
        local_path: PathBuf,
        #[serde(with = "serde_regex", default)]
        pattern: Option<Regex>,
        recursive: Option<bool>,
        window: Option<u64>,
    }

    impl AgentWatcher {
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
        pub fn window(&self) -> u64 {
            self.window.unwrap_or(DEFAULT_EVENT_WINDOW_SECONDS)
        }
        pub fn watch<F: DebounceEventHandler>(&self, tx: F) -> Debouncer<FsEventWatcher> {
            let mut watcher =
                new_debouncer(std::time::Duration::from_secs(self.window()), tx).unwrap();
            watcher
                .watcher()
                .watch(self.local_path(), self.recursive_mode())
                .unwrap();
            println!("Watching: {self:?}");
            watcher
        }
    }

    #[derive(Builder, Deserialize, Debug, Clone)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct Agent {
        watcher: AgentWatcher,
        bucket_name: String,
        profile_name: Option<String>,
        region_name: Option<String>,
        delete: Option<bool>,
    }

    impl Agent {
        pub const fn watcher(&self) -> &AgentWatcher {
            &self.watcher
        }
        pub fn delete(&self) -> bool {
            self.delete.unwrap_or(false)
        }
        pub async fn process_file(&self, file: &Path) -> Result<(), anyhow::Error> {
            if let Ok(key) = self.object_key(file) {
                println!("Uploading: {self:?} - {key}");
                self.upload_file(file, &key).await?;
                println!("Successful: {self:?} - {file:?}");
                if self.delete() {
                    std::fs::remove_file(file)?;
                    println!("Cleaned: {self:?} - {file:?}");
                }
            }
            Ok(())
        }
        fn object_key(&self, path: &Path) -> Result<String, anyhow::Error> {
            let key = path
                .strip_prefix(self.watcher.local_path.clone())?
                .to_str()
                .ok_or_else(|| anyhow!("Non-unicode path"))?;
            let applied_pattern = self
                .watcher
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
