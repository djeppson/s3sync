#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

use clap::Parser;
use std::time::Duration;
use ux::Cli;
mod ux {
    use std::path::PathBuf;

    use clap::Parser;
    use clap_num::number_range;
    use notify::RecursiveMode;
    use notify_debouncer_mini::notify;
    use regex::Regex;

    #[derive(Parser, Debug)]
    #[command(about, long_about = None)]
    pub struct Cli {
        /// Local file path to sync
        #[arg(long, short)]
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
        #[arg(short, long, default_value_t = true)]
        pub recursive: bool,
        /// Number of seconds to aggregate events
        #[arg(short, long, value_parser=window_seconds_range, default_value_t = 10)]
        pub window: u64,
        #[arg(long, hide = true)]
        pub markdown_help: bool,
    }

    impl Cli {
        pub const fn recursive(&self) -> RecursiveMode {
            if self.recursive {
                notify::RecursiveMode::Recursive
            } else {
                notify::RecursiveMode::NonRecursive
            }
        }
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

    let cli = Cli::parse();

    // Invoked as: `$ my-app --markdown-help`
    if cli.markdown_help {
        clap_markdown::print_help_markdown::<Cli>();
    }

    // Setup the channel and simple debouncer
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer =
        notify_debouncer_mini::new_debouncer(Duration::from_secs(cli.window), tx).unwrap();
    debouncer
        .watcher()
        .watch(&cli.path, cli.recursive())
        .unwrap();

    // Handle incoming events
    let s3sync = client::S3SyncBuilder::default()
        .local_path(cli.path)
        .pattern(cli.pattern)
        .delete(cli.delete)
        .bucket_name(cli.bucket)
        .client(cli.profile_name, cli.region_name)
        .await
        .build()?;
    for events in rx.into_iter().flatten() {
        for event in events {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                s3sync.process_file(&event.path).await?;
            }
        }
    }
    Ok(())
}
