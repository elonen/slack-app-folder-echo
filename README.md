# Daemon to monitor local folders and post new files to Slack

This bot is intended to be used on a local or LAN shared folder that is
dedicated to this purpose -- it moves files to `posted/` or `rejected/`
after they have been processed, to avoid accidental re-posting.

For Linux, there's a systemd service file in the debian/ directory, and an
example configuration file that goes to /etc/slack-app-folder-echo.conf
when .deb package is installed. You can build the .deb package
with `./build-deb-in-docker.sh`.

Windows binary should also be usable as no unix-specific
features are required (it uses inotify for file monitoring
on Linux, but will fall back to polling if it's not available).

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
