# Command-Line Help for `s3sync`

This document contains the help content for the `s3sync` command-line program.

**Command Overview:**

* [`s3sync`↴](#s3sync)

## `s3sync`

**Usage:** `s3sync [OPTIONS] --path <PATH> --bucket <BUCKET>`

###### **Options:**

* `--path <PATH>` — Local file path to sync
* `--pattern <PATTERN>` — Regex pattern to apply to filenames

  Default value: `.*`
* `--bucket <BUCKET>` — S3 bucket to sync with
* `--profile <PROFILE_NAME>` — AWS credential profile to use

  Default value: `default`
* `--region <REGION_NAME>` — AWS region override
* `--delete` — Delete source file after successful upload

  Default value: `false`
* `-r`, `--recursive` — Recursively sync the provided path

  Default value: `true`
* `-w`, `--window <WINDOW>` — Number of seconds to aggregate events

  Default value: `10`



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

