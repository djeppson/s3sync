#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use std::time::Duration;

use clap::Parser;
use notify_debouncer_mini::{notify::FsEventWatcher, DebouncedEventKind, Debouncer};
use ux::Cli;
mod ux {
    use std::path::PathBuf;

    use clap::Parser;
    use clap_num::number_range;
    use regex::Regex;

    #[derive(Parser, Debug)]
    #[command(about, long_about = None)]
    pub struct Cli {
        /// Local file path to sync
        #[arg(long, short, default_value = default_local_path().into_os_string())]
        pub path: PathBuf,
        /// Regex pattern to apply to filenames
        #[arg(long, default_value_t = Regex::new(".*").unwrap())]
        pub pattern: Regex,
        /// S3 bucket to sync with
        #[arg(long, short)]
        pub bucket: String,
        /// AWS credential profile to use
        #[arg(long = "profile", default_value_t = String::from("default"))]
        pub profile_name: String,
        /// AWS region override
        #[arg(long = "region")]
        pub region_name: Option<String>,
        /// Delete source file after successful upload
        #[arg(long, short, default_value_t = false)]
        pub delete: bool,
        /// Recursively sync the provided path
        #[arg(short, long, default_value_t = false)]
        pub recursive: bool,
        /// Number of seconds to aggregate events
        #[arg(short, long, value_parser=window_seconds_range, default_value_t = 10)]
        pub window: u64,
        #[arg(long)]
        pub config: Option<PathBuf>,
    }

    fn default_local_path() -> PathBuf {
        std::env::current_dir().unwrap()
    }

    fn window_seconds_range(s: &str) -> Result<u64, String> {
        number_range(s, 1, 3600)
    }
}

mod s3sync {
    use std::path::{Path, PathBuf};

    use anyhow::anyhow;
    use aws_config::{default_provider::region::DefaultRegionChain, Region};
    use aws_sdk_s3 as s3;
    use derive_builder::Builder;
    use notify_debouncer_mini::notify::RecursiveMode;
    use s3::primitives::ByteStream;
    use serde::Deserialize;

    use crate::ux::Cli;

    #[derive(Deserialize, Debug)]
    pub struct AgentConfigs {
        pub configs: Vec<AgentConfig>,
    }

    impl AgentConfigs {
        pub fn from_config(filename: PathBuf) -> Self {
            let contents = std::fs::read_to_string(filename).unwrap();
            toml::from_str(&contents).unwrap()
        }
    }

    #[derive(Deserialize, Debug)]
    pub struct AgentConfig {
        pub path: PathBuf,
        pub bucket: String,
        #[serde(default, with = "serde_regex")]
        pub pattern: Option<regex::Regex>,
        pub profile: Option<String>,
        pub region: Option<String>,
        pub delete: Option<bool>,
        pub recursive: Option<bool>,
    }

    #[derive(Builder)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct Agent {
        profile_name: String,
        region_name: Option<String>,
        bucket_name: String,
        local_path: PathBuf,
        pattern: regex::Regex,
        delete: Option<bool>,
        recursive: Option<bool>,
    }

    impl TryFrom<AgentConfig> for Agent {
        type Error = anyhow::Error;

        fn try_from(value: AgentConfig) -> Result<Self, Self::Error> {
            let agent = AgentBuilder::default()
                .local_path(value.path)
                .pattern(
                    value
                        .pattern
                        .unwrap_or_else(|| regex::Regex::new(".*").unwrap()),
                )
                .bucket_name(value.bucket)
                .profile_name(value.profile.unwrap_or_else(|| String::from("default")))
                .region_name(value.region)
                .delete(Some(value.delete.unwrap_or(false)))
                .recursive(Some(value.recursive.unwrap_or(false)))
                .build()?;
            Ok(agent)
        }
    }

    impl TryFrom<Cli> for Agent {
        type Error = anyhow::Error;

        fn try_from(value: Cli) -> Result<Self, Self::Error> {
            let agent = AgentBuilder::default()
                .local_path(value.path)
                .pattern(value.pattern)
                .bucket_name(value.bucket)
                .profile_name(value.profile_name)
                .region_name(value.region_name)
                .delete(Some(value.delete))
                .recursive(Some(value.recursive))
                .build()?;
            Ok(agent)
        }
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
        pub async fn process_file(&self, file: &Path) -> Result<(), anyhow::Error> {
            if let Ok(key) = self.object_key(file) {
                println!("Uploading: {key}");
                self.upload_file(file, &key).await?;
                println!("Upload successful: {file:?}");
                if self.delete.unwrap_or(false) {
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
            if self.pattern.is_match(key) {
                Ok(String::from(key))
            } else {
                Err(anyhow::Error::msg("Does not match pattern"))
            }
        }
        async fn upload_file(&self, path: &Path, key: &str) -> Result<(), anyhow::Error> {
            let body = ByteStream::from_path(path).await?;
            let region = self
                .region_name
                .clone()
                .map(Region::new)
                .or(DefaultRegionChain::builder()
                    .profile_name(&self.profile_name)
                    .build()
                    .region()
                    .await);
            let sdk_config = aws_config::from_env()
                .region(region)
                .profile_name(self.profile_name.clone())
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

#[::tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();

    let (tx, rx) = std::sync::mpsc::channel();

    let agents = Cli::parse().config.map_or_else(
        || vec![s3sync::Agent::try_from(Cli::parse()).unwrap()],
        |filename| {
            s3sync::AgentConfigs::from_config(filename)
                .configs
                .into_iter()
                .map(|config| s3sync::Agent::try_from(config).unwrap())
                .collect()
        },
    );

    let s3sync = &agents[0];
    let window = Duration::from_secs(1);
    let _watchers: Vec<Debouncer<FsEventWatcher>> = agents
        .iter()
        .map(|agent| {
            let mut debouncer = notify_debouncer_mini::new_debouncer(window, tx.clone()).unwrap();
            println!("Watch {:?}", agent.local_path());
            debouncer
                .watcher()
                .watch(agent.local_path(), s3sync.recursive_mode())
                .unwrap();
            debouncer
        })
        .collect::<Vec<_>>();

    for events in rx.into_iter().flatten() {
        for event in events {
            if event.kind == DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
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
