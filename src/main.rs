use std::time::Duration;

use clap::Parser;
use ux::Cli;

mod ux {
    use std::path::PathBuf;

    use clap::Parser;
    use clap_num::number_range;
    use notify::RecursiveMode;
    use regex::Regex;

    #[derive(Parser, Debug)]
    #[command(author, version, about, long_about = None)]
    pub struct Cli {
        // Local file path to sync
        #[arg(long)]
        pub path: PathBuf,
        // Regex pattern to apply to filenames
        #[arg(long)]
        pub pattern: Regex,
        // S3 bucket to sync with
        #[arg(long)]
        pub bucket: String,
        // Named AWS profile
        #[arg(long)]
        pub profile: String,
        // AWS region
        #[arg(long, default_value = "us-east-1")]
        pub region: String,
        // Recursively sync the path
        #[arg(short, long, default_value_t = true)]
        pub recursive: bool,
        // Aggregation window for events (in seconds)
        #[arg(short, long, value_parser=window_seconds_range, default_value_t = 1)]
        pub window: u64,
    }

    impl Cli {
        pub fn recursive(&self) -> RecursiveMode {
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
    use std::path::Path;

    use aws_config::{default_provider::credentials::DefaultCredentialsChain, Region};
    use aws_sdk_s3 as s3;
    use s3::primitives::ByteStream;
    pub struct Bucket {
        client: s3::Client,
        bucket_name: String,
    }

    impl Bucket {
        pub async fn new(profile: String, region_name: String, bucket_name: String) -> Self {
            let aws_creds = DefaultCredentialsChain::builder()
                .profile_name(&profile)
                .region(Region::new(region_name))
                .build()
                .await;
            let sdk_config = aws_config::from_env()
                .credentials_provider(aws_creds)
                .load()
                .await;
            let client = s3::Client::new(&sdk_config);
            Self {
                client,
                bucket_name,
            }
        }
        pub async fn upload_body(&self, body: ByteStream, key: &str) -> Result<(), anyhow::Error> {
            let response = self.client
                .put_object()
                .bucket(self.bucket_name.clone())
                .key(key)
                .body(body)
                .send()
                .await;
                // .map_or(Err(anyhow::Error::msg("Upload request failed")), |_| Ok(()))
            match response {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow::Error::msg(e.to_string()))
            }
        }
        pub async fn upload_file(&self, path: &Path, key: &str) -> Result<(), anyhow::Error> {
            let body = ByteStream::from_path(path).await;
            match body {
                Ok(body) => self.upload_body(body, key).await,
                Err(_) => {
                    // possibly triggered by file deletion event
                    Err(anyhow::Error::msg("File no longer present"))
                }
            }
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
    debouncer
        .watcher()
        .watch(&cli.path, cli.recursive())
        .unwrap();

    // Handle incoming events
    let bucket = client::Bucket::new(cli.profile, cli.region, cli.bucket).await;
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
                    .map(|key| bucket.upload_file(&event.path, key))
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
