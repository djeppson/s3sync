use std::time::Duration;

use clap::Parser;
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
    use std::path::Path;

    use aws_config::{default_provider::region::DefaultRegionChain, Region};
    use aws_sdk_s3 as s3;
    use s3::primitives::ByteStream;
    pub struct Bucket {
        client: s3::Client,
        bucket_name: String,
    }

    impl Bucket {
        pub async fn new(
            profile_name: String,
            bucket_name: String,
            region_name: Option<String>,
        ) -> Self {
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
            Self {
                client,
                bucket_name,
            }
        }
        pub async fn upload_body(&self, body: ByteStream, key: &str) -> Result<(), anyhow::Error> {
            let response = self
                .client
                .put_object()
                .bucket(self.bucket_name.clone())
                .key(key)
                .body(body)
                .send()
                .await;
            match response {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow::Error::msg(e.to_string())),
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
    let bucket = client::Bucket::new(cli.profile_name, cli.bucket, cli.region_name).await;
    for res in rx.into_iter().flatten() {
        for event in res {
            if event.kind == notify_debouncer_mini::DebouncedEventKind::Any  // ignore AnyContinuous (i.e., still in progress)
            && event.path.exists()
            && event.path.is_file()
            {
                if let Some(result) = event
                    .path
                    .strip_prefix(&cli.path)
                    .unwrap()
                    .to_str()
                    .filter(|name| cli.pattern.is_match(name))
                    .map(|key| {
                        println!("Uploading: {key}");
                        bucket.upload_file(&event.path, key)
                    })
                {
                    result.await.map_or_else(
                        |e| println!("Error uploading file: {e:?}"),
                        |()| {
                            println!("Upload successful: {:?}", &event.path);
                            if cli.delete {
                                std::fs::remove_file(&event.path).map_or_else(
                                    |e| println!("Delete failed {e:?}"),
                                    |()| println!("Cleaned-up file {:?}", &event.path),
                                );
                            }
                        },
                    );
                }
            }
        }
    }
    Ok(())
}
