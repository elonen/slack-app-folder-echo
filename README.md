# Scan local folder(s) and post new file(s) to Slack

This CLI tool / daemon is intended to process files from a local or LAN shared folder
that is dedicated to this purpose -- you don't want to run it just anywhere, as it
moves files to `posted/` or `rejected/` after they have been processed,
to avoid accidental re-posting.

Config is .ini format:

```ini
[Funny cat pics]
folder = /path/to/my_folder
slack_channel = #daily-cat-pictures
limit_uploads_per_minute = 10
bot_name = Cat Pictures!
bot_icon = :cat:
slack_token = xoxb-1234567890-1234567890-1234567890-1234567890

[My private folder]
folder = /home/user/private_folder
slack_channel = @user_name
limit_uploads_per_minute = 5
bot_name = Your private files
bot_icon = :robot_face:
slack_token = xoxb-1234567890-1234567890-1234567890-1234567890
```

## `--once` mode for cron jobs

If you want to run the bot in a cron job or similar, you can use the `--once` option
to process all files in the folder and exit. Exit code is 0
if all files were posted successfully, 1 if there were errors.

## CLI options

```
Usage:
  slack-app-folder-echo [options] <config_file>
  slack-app-folder-echo (-h | --help)

Required:
    <config_file>       INI file with configuration

Options:
 -1 --once              Post all files in folder and exit
                        (with status 0 for success, 1 for failure)
 -d --debug             Enable debug logging
 -h --help              Show this screen
 -v --version           Show version
```

## Deployment

For Linux, there's a systemd service file in the debian/ directory, and an
example configuration file that goes to /etc/slack-app-folder-echo.conf
when .deb package is installed. You can build the .deb package
with `./build-deb-in-docker.sh`.

Windows binary should also be usable as no unix-specific
features are required (it uses inotify for file monitoring
on Linux, but will fall back to polling if it's not available).
