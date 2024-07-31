## s3sync - a super basic S3 sync utility
Hobby project for playing with Rust.  You should probably use AWS's CLI `aws s3 sync` instead.

## Why
Prior solution was running this little gem on a every-minute cron job
```
aws s3 mv /example/source/folder/ s3://my-great-bucket/ --profile foo --recursive --exclude "*" --include "example*.csv"
```

Issues:
* Had to wait for up to a minute for the cron schedule to kick-off, why wait?!
* I didn't build it and I wanted to build a thing


### Usage: 

Check here for [cli instructions](help.md)


## Run on Mac at startup:
1. Create the appropriate plist with the arguments you need [example](com.darrenjeppson.s3sync.plist)
2. Save the plist file in `~/Library/LaunchAgents/`
3. Load the file: `launchctl load ~/Library/LaunchAgents/com.darrenjeppson.s3sync.plist`


## Someday features:
- [X] Handy-dandy way to run as a background service on start-up
- [x] Arg for recursion
- [x] Works on Mac/Linux and PC
- [x] Optional deletion upon successful upload / staging files
- [x] Ability to override the AWS profile region
- [x] Automated markdown generation for README.md
- [ ] Multiple source-to-target mappings
- [ ] Automated [plist](com.darrenjeppson.s3sync.plist) generation
- [ ] Automated [plist](com.darrenjeppson.s3sync.plist) deployment and `launchctl unload ... && launchctl load ...`
- [ ] Option to remove files from bucket upon local deletion
- [ ] Try to get the region info from the bucket name if not provided
- [ ] Validate bucket access on start-up
- [ ] Delete S3 object on local file removal
- [ ] Bi-directional sync (e.g. `S3 --> SQS <-- client --> local`)


## Thanks to:
* plist shenannigans:
  * https://www.karltarvas.com/macos-run-script-on-startup.html
  * https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html
* PC compilation:
  * https://gist.github.com/Mefistophell/9787e1b6d2d9441c16d2ac79d6a505e6
