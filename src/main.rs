#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use clap::Parser;

const DEFAULT_EVENT_WINDOW_SECONDS: u64 = 5;

#[::tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        .add_directive("aws_config=warn".parse()?)
        .add_directive("aws_smithy_runtime=warn".parse()?);
    let subscriber = tracing_subscriber::fmt()
        .pretty()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_env_filter(filter)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    tracing::debug!("Setting up channel");
    let (tx, rx) = std::sync::mpsc::channel();

    let manager = s3sync::Manager::try_from(ux::Cli::parse())?;
    // Need a variable name to get the watchers to run
    let _watchers = manager
        .watchers()
        .iter()
        .map(|watcher| watcher.watch(tx.clone()))
        .collect::<Vec<_>>();

    for events in rx.into_iter().flatten() {
        for event in events {
            manager.process_event(&event).await?;
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
        pub bucket: Option<String>,
        /// Prefix to prepend to the key
        #[arg(long)]
        pub prefix: Option<String>,
        /// Regex filter to match events
        #[arg(long)]
        pub pattern: Option<Regex>,
        /// AWS credential profile to use
        #[arg(long)]
        pub profile: Option<String>,
        /// AWS region override
        #[arg(long)]
        pub region: Option<String>,
        /// Delete source file after successful upload
        #[arg(long, short)]
        pub delete: Option<bool>,
        /// Recursively sync the provided path
        #[arg(short, long)]
        pub recursive: Option<bool>,
        /// Number of seconds to aggregate events
        #[arg(short, long, value_parser=window_seconds_range, default_value_t = DEFAULT_EVENT_WINDOW_SECONDS)]
        pub window: u64,
        #[arg(long)]
        pub config: Option<PathBuf>,
    }
}

mod s3sync {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

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
        pub fn watchers(&self) -> Vec<AgentWatcher> {
            let mut path_settings: HashMap<&PathBuf, PathSettings> = HashMap::new();
            for agent in &self.agents {
                if let Some(settings) = path_settings.get(agent.watcher.local_path()) {
                    let settings = agent.watcher.settings.clone() + settings.clone();
                    path_settings.insert(&agent.watcher.local_path, settings);
                } else {
                    path_settings.insert(&agent.watcher.local_path, agent.watcher.settings.clone());
                }
            }
            path_settings
                .into_iter()
                .map(|(local_path, settings)| AgentWatcher {
                    local_path: local_path.clone(),
                    settings,
                })
                .collect()
        }
        pub async fn process_event(&self, event: &DebouncedEvent) -> Result<(), anyhow::Error> {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                tracing::debug!("Process: {event:?}");
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
                Ok(serde_yaml::from_str(&contents)?)
            } else {
                let path_settings = PathSettings {
                    recursive: value.recursive,
                    window: Some(value.window),
                };
                let watcher = AgentWatcher {
                    local_path: value.path,
                    settings: path_settings,
                };
                let agent = Agent {
                    watcher,
                    pattern: value.pattern,
                    bucket_name: value.bucket,
                    profile_name: value.profile,
                    region_name: value.region,
                    delete: value.delete,
                    key_prefix: value.prefix,
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
        settings: PathSettings,
    }

    impl AgentWatcher {
        pub const fn local_path(&self) -> &PathBuf {
            &self.local_path
        }
        pub fn watch<F: DebounceEventHandler>(&self, tx: F) -> Debouncer<FsEventWatcher> {
            let mut watcher =
                new_debouncer(std::time::Duration::from_secs(self.settings.window()), tx).unwrap();
            watcher
                .watcher()
                .watch(self.local_path(), self.settings.recursive_mode())
                .unwrap();
            tracing::info!("Watching: {self:?}");
            watcher
        }
    }

    #[derive(Deserialize, Debug, Clone)]
    pub struct PathSettings {
        recursive: Option<bool>,
        window: Option<u64>,
    }

    impl PathSettings {
        pub fn recursive(&self) -> bool {
            self.recursive.unwrap_or(false)
        }
        pub fn recursive_mode(&self) -> RecursiveMode {
            if self.recursive() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            }
        }
        pub fn window(&self) -> u64 {
            self.window.unwrap_or(DEFAULT_EVENT_WINDOW_SECONDS)
        }
    }

    impl std::ops::Add for PathSettings {
        type Output = Self;

        fn add(self, rhs: Self) -> Self::Output {
            let window = std::cmp::min(self.window(), rhs.window());
            let recursive = !matches!(
                (self.recursive_mode(), rhs.recursive_mode()),
                (RecursiveMode::NonRecursive, RecursiveMode::NonRecursive)
            );
            Self {
                window: Some(window),
                recursive: Some(recursive),
            }
        }
    }

    impl Default for PathSettings {
        fn default() -> Self {
            Self {
                recursive: Some(false),
                window: Some(DEFAULT_EVENT_WINDOW_SECONDS),
            }
        }
    }

    #[derive(Builder, Deserialize, Debug, Clone)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct Agent {
        watcher: AgentWatcher,
        #[serde(with = "serde_regex", default)]
        pattern: Option<Regex>,
        bucket_name: Option<String>,
        key_prefix: Option<String>,
        profile_name: Option<String>,
        region_name: Option<String>,
        delete: Option<bool>,
    }

    impl Agent {
        #[tracing::instrument]
        fn object_key(&self, path: &Path) -> Result<String, anyhow::Error> {
            let key = path
                .strip_prefix(self.watcher.local_path.clone())?
                .to_str()
                .ok_or_else(|| anyhow!("Non-unicode path"))?;
            tracing::debug!("Proposed object key: '{key}'");
            let applied_pattern = self
                .pattern
                .clone()
                .unwrap_or_else(|| Regex::new(r".*").unwrap());
            tracing::debug!("Pattern to match: '{applied_pattern}'");
            if applied_pattern.is_match(key) {
                let key = self
                    .key_prefix
                    .clone()
                    .map_or(key.to_string(), |prefix| format!("{prefix}{key}"));
                tracing::debug!("Final object key '{key}'");
                Ok(key)
            } else {
                tracing::debug!("Path does not match pattern");
                Err(anyhow::Error::msg("Does not match pattern"))
            }
        }

        #[tracing::instrument]
        async fn process_file(&self, file: &Path) -> Result<(), anyhow::Error> {
            if let Ok(key) = self.object_key(file) {
                tracing::debug!("Processing");
                self.upload_file(file, &key).await?;
                if self.delete.unwrap_or(false) {
                    Self::delete_source(file)?;
                } else {
                    tracing::debug!("Skip removal");
                }
            } else {
                tracing::debug!("Skip processing");
            }
            Ok(())
        }

        #[tracing::instrument]
        async fn upload_file(&self, path: &Path, key: &str) -> Result<(), anyhow::Error> {
            let bucket_name = self
                .bucket_name
                .clone()
                .ok_or_else(|| anyhow::Error::msg("Bucket name is required"))?;
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
                .bucket(bucket_name)
                .key(key)
                .body(body)
                .send()
                .await?;
            tracing::info!("File uploaded");
            Ok(())
        }

        #[tracing::instrument]
        fn delete_source(path: &Path) -> Result<(), anyhow::Error> {
            std::fs::remove_file(path)?;
            tracing::info!("Source file removed");
            Ok(())
        }
    }
}
