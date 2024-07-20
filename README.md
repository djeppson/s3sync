## s3sync - a super basic S3 sync utility
Hobby project for playing with Rust.  You should probably use AWS's CLI `aws s3 sync` instead.

## Why
Prior solution was running this little gem: 
```
aws s3 mv /example/source/folder/ s3://my-great-bucket/ --profile foo --recursive --exclude "*" --include "example*.csv"
```

Issues:
* Had to wait for up to a minute for the cron schedule to kick-off, why wait?!
* I didn't build it and I wanted to build a thing


### Examples: 

This will upload all files (recursively) to the desired bucket
```
s3sync \
    --path ~/Downloads \
    --bucket cloud-backup-bucket
```

This will only upload files meeting a specific pattern to a bucket using a named profile
```
s3sync \
    --path ~/Downloads \
    --pattern "Pictures[0-9]{8}.jpg" \
    --bucket my-awesome-pics \
    --profile picapp 
```

Someday features:

- [X] Handy-dandy way to run as a background service on start-up
- [x] Arg for recursion
- [ ] Option to remove files from bucket upon local deletion
- [ ] Validate bucket access on start-up
- [ ] Bi-directional sync (e.g. `S3 --> SQS <-- client --> local`)
- [ ] Works on Mac/Linux and PC
- [ ] Delete object on local file removal

Non-features:
- Multi-bucket destinations (use S3 replication instead)


Thanks to:
* plist shenannigans:
  * https://www.karltarvas.com/macos-run-script-on-startup.html
  * https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html
* PC compilation:
  * https://gist.github.com/Mefistophell/9787e1b6d2d9441c16d2ac79d6a505e6
