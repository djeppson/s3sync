# Controlling log output - esp. from AWS crates (https://docs.aws.amazon.com/sdk-for-rust/latest/dg/logging.html):
#   RUST_LOG=aws_smithy_runtime=warn,aws_config=warn,info cargo run ... avoid informational AWS messages
#   RUST_LOG=debug ... will output AWS and local code debug (very verbose)

git pull
cargo build --release
codesign -s $(cat .private/codesign-id.txt) -v target/release/s3sync