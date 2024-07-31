cargo build --release
# TODO: figure out how to create md and then exit if --markdown-help is given
cargo run -- --p / --b foo --markdown-help > help.md