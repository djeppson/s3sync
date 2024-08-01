#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use clap::Parser;
use notify_debouncer_mini::DebouncedEventKind;
use std::time::Duration;
use ux::Cli;
mod ux {
    use std::path::PathBuf;

    use clap::Parser;
    use clap_num::number_range;
    use notify::RecursiveMode;
    use notify_debouncer_mini::notify;
    use regex::Regex;
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct SyncConfigs {
        pub configs: Vec<SyncConfig>,
    }

    #[derive(Deserialize, Debug)]
    pub struct SyncConfig {
        path: PathBuf,
        bucket: String,
        #[serde(default, with = "serde_regex")]
        pattern: Option<regex::Regex>,
        profile: Option<String>,
        region: Option<String>,
        delete: Option<bool>,
        recursive: Option<bool>,
    }

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
    }

    impl Cli {
        pub const fn recursive(&self) -> RecursiveMode {
            if self.recursive {
                notify::RecursiveMode::Recursive
            } else {
                notify::RecursiveMode::NonRecursive
            }
        }
        pub fn configs(&self) -> SyncConfigs {
            let filename = "sync.toml";
            let contents = std::fs::read_to_string(filename).unwrap();
            let mut configs: SyncConfigs = toml::from_str(&contents).unwrap();
            println!("File Configs: {configs:?}");
            let cmdline = SyncConfig {
                path: self.path.clone(),
                bucket: self.bucket.clone(),
                pattern: Some(self.pattern.clone()),
                profile: Some(self.profile_name.clone()),
                region: self.region_name.clone(),
                delete: Some(self.delete),
                recursive: Some(self.recursive),
            };
            configs.configs.push(cmdline);
            configs
        }
    }

    fn default_local_path() -> PathBuf {
        std::env::current_dir().unwrap()
    }

    fn window_seconds_range(s: &str) -> Result<u64, String> {
        number_range(s, 1, 3600)
    }
}

mod client {
    use anyhow::anyhow;
    use aws_config::{default_provider::region::DefaultRegionChain, Region};
    use aws_sdk_s3 as s3;
    use derive_builder::Builder;
    use s3::primitives::ByteStream;
    use std::path::{Path, PathBuf};

    #[derive(Builder)]
    #[builder(build_fn(error = "anyhow::Error"))]
    pub struct S3Sync {
        #[builder(setter(custom))]
        client: s3::Client,
        bucket_name: String,
        local_path: PathBuf,
        pattern: regex::Regex,
        pub delete: bool,
    }

    impl S3SyncBuilder {
        pub async fn client(
            &mut self,
            profile_name: String,
            region_name: Option<String>,
        ) -> &mut Self {
            let region = region_name
                .map(Region::new)
                .or(DefaultRegionChain::builder()
                    .profile_name(&profile_name)
                    .build()
                    .region()
                    .await);
            let sdk_config = aws_config::from_env()
                .region(region)
                .profile_name(profile_name)
                .load()
                .await;
            let client = s3::Client::new(&sdk_config);
            self.client = Some(client);
            self
        }
    }

    impl S3Sync {
        pub const fn local_path(&self) -> &PathBuf {
            &self.local_path
        }
        pub async fn process_file(&self, file: &Path) -> Result<(), anyhow::Error> {
            if let Ok(key) = self.object_key(file) {
                println!("Uploading: {key}");
                self.upload_file(file, &key).await?;
                println!("Upload successful: {file:?}");
                if self.delete {
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
            self.client
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

    let cli = Cli::parse();
    for config in cli.configs().configs {
        println!("{config:?}");
    }

    let window = Duration::from_secs(cli.window);
    let _watchers = [
        &cli.path,
        std::path::Path::new("/Users/darrenjeppson/Downloads/frob"),
        std::path::Path::new("/Users/darrenjeppson/Downloads/freeb"),
    ]
    .map(|path| {
        let mut debouncer = notify_debouncer_mini::new_debouncer(window, tx.clone()).unwrap();
        println!("Watch {path:?}");
        debouncer.watcher().watch(path, cli.recursive()).unwrap();
        debouncer
    });

    let s3sync = client::S3SyncBuilder::default()
        .local_path(cli.path)
        .pattern(cli.pattern)
        .delete(cli.delete)
        .bucket_name(cli.bucket)
        .client(cli.profile_name, cli.region_name)
        .await
        .build()?;
    // for events in rx.into_iter().flatten() {
    //     for event in events {
    //         if event.kind == DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
    //         && event.path.exists()
    //         && event.path.is_file()
    //         {
    //             println!("Process: {event:?}");
    //             s3sync.process_file(&event.path).await?;
    //         }
    //     }
    // }
    Ok(())
}
