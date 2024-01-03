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


Example: 
```
s3sync \
    --path ~/Downloads \
    --pattern "Pictures[0-9]{8}.jpg" \
    --bucket my-awesome-pics \
    --profile picapp \
    --region us-east-1
```

Someday features:

- [X] Handy-dandy way to run as a background service on start-up
- [ ] Validate bucket access on start-up
- [x] Arg for recursion
- [ ] Arg for key prefix (original | fixed)
- [ ] Bi-directional sync (e.g. `S3 --> SQS <-- client --> local`)
- [ ] AWS Cognito and/or AWS IAM Identity Center integration

Non-features:
- Multi-bucket destinations (use S3 replication instead)


Thanks to:
* plist shenannigans:
  * https://www.karltarvas.com/macos-run-script-on-startup.html
  * https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html
  