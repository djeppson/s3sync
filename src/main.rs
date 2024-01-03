use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use aws_config::{default_provider::credentials::DefaultCredentialsChain, Region};
use aws_sdk_s3 as s3;
use clap::Parser;
use clap_num::number_range;
use regex::Regex;
use s3::primitives::ByteStream;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    // Local file path to sync
    #[arg(long)]
    path: PathBuf,
    // Regex pattern to apply to filenames
    #[arg(long)]
    pattern: Regex,
    // S3 bucket to sync with
    #[arg(long)]
    bucket: String,
    // Named AWS profile
    #[arg(long)]
    profile: String,
    // AWS region
    #[arg(long, default_value = "us-east-1")]
    region: String,
    // Recursively sync the path
    #[arg(short, long, default_value_t = true)]
    recursive: bool,
    // Aggregation window for events (in seconds)
    #[arg(short, long, value_parser=window_seconds_range, default_value_t = 3)]
    window: u64,
}

fn window_seconds_range(s: &str) -> Result<u64, String> {
    number_range(s, 1, 3600)
}

impl Cli {
    pub fn region(&self) -> Region {
        Region::new(self.region.clone())
    }
}

pub async fn upload_file(
    path: &Path,
    key: &str,
    bucket_name: &str,
    client: &s3::Client,
) -> Result<(), anyhow::Error> {
    let body = ByteStream::from_path(path).await;
    match body {
        Ok(body) => client
            .put_object()
            .bucket(bucket_name)
            .key(key)
            .body(body)
            .send()
            .await
            .map_or(Err(anyhow::Error::msg("Upload request failed")), |_| Ok(())),
        Err(_) => {
            // possibly triggered by file deletion event
            Err(anyhow::Error::msg("File no longer present"))
        }
    }
}

#[::tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // Setup the channel and simple debouncer
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer =
        notify_debouncer_mini::new_debouncer(Duration::from_secs(cli.window), tx).unwrap();
    let recursive_mode = if cli.recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    debouncer
        .watcher()
        .watch(&cli.path, recursive_mode)
        .unwrap();

    // Setup S3
    let aws_creds = DefaultCredentialsChain::builder()
        .profile_name(&cli.profile)
        .region(cli.region())
        .build()
        .await;
    let sdk_config = aws_config::from_env()
        .credentials_provider(aws_creds)
        .load()
        .await;
    let client = s3::Client::new(&sdk_config);

    // Handle incoming events
    for res in rx.into_iter().flatten() {
        for event in res {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            {
                if let Some(result) = event
                    .path
                    .file_name()
                    .and_then(|filename| filename.to_str())
                    .filter(|name| cli.pattern.is_match(name))
                    .map(|key| upload_file(&event.path, key, &cli.bucket, &client))
                {
                    result.await.map_or_else(
                        |e| println!("Error uploading file: {e:?}"),
                        |()| {
                            println!("Upload successful: {:?}", &event.path);
                            std::fs::remove_file(&event.path).map_or_else(
                                |e| println!("Delete failed {e:?}"),
                                |()| println!("Cleaned-up file {:?}", &event.path),
                            );
                        },
                    );
                }
            }
        }
    }
    Ok(())
}
