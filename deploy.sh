git pull
cargo build --release
codesign -s $(cat .private/codesign-id.txt) -v target/release/s3sync