cargo build --release
# TODO: figure out how to create md and then exit if --markdown-help is given
cargo run -- --path / --bucket foo --markdown-help > help.md