use docopt::Docopt;
use std::{path::{PathBuf, Path}, time::Duration, num::NonZeroU32};
use notify::{self, Watcher, RecommendedWatcher};
use log::{info, debug, warn, error};
use thiserror::Error;
use governor::{Quota, RateLimiter};
use anyhow::anyhow;

const FILE_SETTLE_MAX_WAIT: Duration = Duration::from_secs(60);
const FILE_SETTLE_WAIT: Duration = Duration::from_secs(5);

const NAME: &'static str = env!("CARGO_PKG_NAME");
const VERSION: &'static str = env!("CARGO_PKG_VERSION");

const USAGE: &'static str = r#"
Slack folder-echo bot

Monitors given folder for new files and posts them to Slack.
If post fails, the file is moved to a "rejected" folder.
On success, the file is moved to a "posted" folder.

Usage:
  slack-folder-echo [options] <config_file>
  slack-folder-echo (-h | --help)

Required:
    <config_file>       INI file with configuration

Options:
 -j --json              Log in JSON format
 -d --debug             Enable debug logging
 -h --help              Show this screen
 -v --version           Show version


Example configuration file:

    [public folder]
    bot_name = Cat Pictures!
    bot_icon = :robot_face:
    folder = /path/to/my_folder
    limit_uploads_per_minute = 10
    slack_channel = #daily-cat-pictures
    slack_token = xoxb-1234567890-1234567890-1234567890-1234567890

    [my private folder]
    folder = /home/user/private_folder
    slack_channel = @user_name
    ...
"#;


#[derive(Error, Debug)]
enum BotError {
    #[error("Config error: {0}")]
    ConfigError(#[from] ini::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Slack API error: {0}")]
    SlackApiError(String),

    #[error("File error: {0}")]
    FileError(#[from] std::io::Error),

    #[error("Folder watcher error: {0}")]
    WatcherError(#[from] notify::Error),

    #[error("Timeout: file failed to settle after {0:?}")]
    TimeoutError(Duration),

    #[error("Anyhow error: {0}")]
    AnyhowError(#[from] anyhow::Error),
}
type BotResult<T> = Result<T, BotError>;


#[derive(Debug, Clone)]
struct BotConfig {
    bot_name: String,
    folder: PathBuf,
    limit_uploads_per_minute: NonZeroU32,
    slack_channel: String,
    slack_token: String,
}

#[derive(Debug, Clone)]
struct BotSlackMessage {
    title: Option<String>,
    icon_emoji: Option<String>,
    text: Option<String>,
    file: Option<PathBuf>,
}

/**
 * Parse an INI config file.
 */
fn read_config_file(config_file: &Path) -> BotResult<Vec<BotConfig>>
{
    info!("Reading config file: {:?}", config_file);
    let config = ini::Ini::load_from_file(config_file)?;
    let mut bots = Vec::new();
    for (_, section) in config.iter() {
        let bot_name =  section.get("bot_name").ok_or(anyhow!("Missing bot_name"))?.to_string();
        let folder = PathBuf::from(section.get("folder").ok_or(anyhow!("Missing folder"))?);
        let limit_uploads_per_minute = section.get("limit_uploads_per_minute")
            .ok_or(anyhow::anyhow!("Missing limit_uploads_per_minute"))?.parse::<NonZeroU32>()
            .map_err(|_| anyhow::anyhow!("Invalid limit_uploads_per_minute"))?;
        let slack_channel = section.get("slack_channel").ok_or(anyhow!("Missing slack_channel"))?.to_string();
        let slack_token = section.get("slack_token").ok_or(anyhow!("Missing slack_token"))?.to_string();
        info!("Found bot: {:?}, watching folder: {:?}", bot_name, folder);
        bots.push(BotConfig { bot_name, folder, limit_uploads_per_minute, slack_channel, slack_token });
    }
    Ok(bots)
}

/**
 * Watch a folder for new files and send them to the given channel.
 * This function will block until given path is unwatch()ed (i.e. paths_tx closes).
 * 
 * @param config Bot configuration
 * @param paths_rx Channel to receive new file paths
 */
fn file_watcher(folder: PathBuf, paths_tx: std::sync::mpsc::Sender<PathBuf>) -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Use inotify is available, otherwise fall back to polling
    let mut watcher: Box<dyn Watcher> =
        if RecommendedWatcher::kind() == notify::WatcherKind::PollWatcher {
            let config = notify::Config::default().with_poll_interval(Duration::from_secs(2));
            Box::new(notify::PollWatcher::new(tx, config).unwrap())
        } else {
            Box::new(RecommendedWatcher::new(tx, notify::Config::default()).unwrap())
        };

    info!("Watching folder: {:?}", folder);
    watcher.watch(folder.as_path(), notify::RecursiveMode::NonRecursive)?;

    for res in rx {
        match res {
            Ok(event) => {
                if let notify::EventKind::Create(_) = event.kind {
                    for path in event.paths {
                        debug!("Watcher saw new file: {:?}", path);
                        if path.is_file() {
                            paths_tx.send(path.clone()).unwrap();
            }}}},
            Err(e) => return Err(e),
        }
    }
    Ok(())
}


/**
 * Waits until a file settles -- that is, hasn't grown in size for a `settle_wait` time.
 * 
 * @param path Path to file
 * @param settle_wait Time to wait for file to stop growing
 * @param max_wait Maximum time to wait for file to settle before giving up
 * @return Ok(()) if file settles, Err(TimeoutError) if file doesn't settle within `max_wait` time
 */
fn wait_until_file_settles(path: &Path, settle_wait: Duration, max_wait: Duration ) -> BotResult<()> {
    assert!(settle_wait < max_wait);
    let file_basename = path.file_name().ok_or(anyhow!("Invalid file path"))?.to_string_lossy();
    info!("Waiting for file to settle: {:?} (max_wait: {:?}, settle_wait: {:?})", file_basename, max_wait, settle_wait);

    let start_t = std::time::Instant::now();
    let mut last_change_t = start_t.clone();
    let mut size = std::fs::metadata(path)?.len();

    while start_t.elapsed() < max_wait {
        std::thread::sleep(settle_wait/4);
        let new_size = std::fs::metadata(path)?.len();
        if new_size != size {
            last_change_t = std::time::Instant::now();
            size = new_size;
        } else if last_change_t.elapsed() > settle_wait {
            info!("File settled: {:?}", file_basename);
            return Ok(());
        }
    }
    warn!("File failed to settle: {:?}", file_basename);
    Err(BotError::TimeoutError(max_wait))
}

/**
 * Upload file or a message to Slack
 * 
 * @param conf Bot configuration (for a single channel)
 * @param msg Message to post
 */
fn post_message(conf: &BotConfig, msg: &BotSlackMessage) -> BotResult<()> {
    let res = if let Some(file) = &msg.file
    {
        info!("Posting file to Slack: {:?}", &msg);

        let mut form = reqwest::blocking::multipart::Form::new();
        if let Some(text) = &msg.text {
            form = form.text("initial_comment", text.clone());
        }
        if let Some(title) = &msg.title {
            form = form.text("title", title.clone());
        }    
        form = form.text("channels", conf.slack_channel.clone());
        
        if std::fs::metadata(file)?.len() > 1024*1024 {
            return Err(BotError::AnyhowError(anyhow!("File too large for Slack")));
        }
        let part = reqwest::blocking::multipart::Part::file(file)?;
        form = form.part("file", part);

        let client = reqwest::blocking::Client::new();
        client.post("https://slack.com/api/files.upload")
            .multipart(form)
            .bearer_auth(&conf.slack_token)
            .send()
    }
    else
    {
        info!("Posting message to Slack: {:?}", &msg);

        let client = reqwest::blocking::Client::new();
        let mut params = std::collections::HashMap::new();
        params.insert("channel", conf.slack_channel.clone());
        if let Some(text) = &msg.text {
            let mut text = text.clone();
            if let Some(title) = &msg.title {
                text = format!("*{}*\n{}", title, text);
            }
            params.insert("text", text);
        }
        if let Some(emoji) = &msg.icon_emoji {
            params.insert("icon_emoji", emoji.clone());
        }
        client.post("https://slack.com/api/chat.postMessage")
            .form(&params)
            .bearer_auth(&conf.slack_token)
            .send()
    }?;

    debug!("Slack HTTP API response: {:?}", res);

    if !res.status().is_success() {
        return Err(BotError::SlackApiError(res.text()?));
    }
    Ok(())
}


/**
 * Worker thread for a single folder/channel pair.
 */
fn bot_thread(conf: BotConfig) -> BotResult<()>
{
    info!("Starting bot thread: {:?}. Folder {:?}, channel: {:?}",
        conf.bot_name, conf.folder, conf.slack_channel);

    let upload_limiter = RateLimiter::direct(Quota::per_minute(conf.limit_uploads_per_minute));
    let limit_warning_limiter = RateLimiter::direct(Quota::per_minute(NonZeroU32::new(1).unwrap()));

    // Create folders for rejected and posted files
    let rejected_dir = conf.folder.join("rejected");
    let posted_dir = conf.folder.join("posted");
    info!("Creating folders: {:?} {:?}", rejected_dir, posted_dir);
    std::fs::create_dir_all(&rejected_dir)?;
    std::fs::create_dir_all(&posted_dir)?;

    // Start file watcher thread
    let (files_tx, files_rx) = std::sync::mpsc::channel();
    let c = conf.clone();
    let watcher_thread = std::thread::spawn(move || {
        let conf = c;
        file_watcher(conf.folder.clone(), files_tx).unwrap();
    });

    fn handle_file(path: &Path, conf: &BotConfig) -> BotResult<()> 
    {
        let file_basename = path.file_name().ok_or(anyhow!("Invalid file path"))?.to_string_lossy();
        wait_until_file_settles(&path, FILE_SETTLE_WAIT, FILE_SETTLE_MAX_WAIT)?;
        post_message(conf, &BotSlackMessage {
            title: Some(file_basename.to_string()),
            text: None,
            icon_emoji: None,
            file: Some(path.to_path_buf())
        })?;
        Ok(())
    }

    fn post_error(filename: &str, conf: &BotConfig, err: &BotError) -> BotResult<()> 
    {
        post_message(conf, &BotSlackMessage {
            title: Some(format!("Sorry! Error posting file.")),
            text: Some(format!("Failed to process / post incoming file '{}'. Admins, please check logs. Error: {:?}", filename, err)),
            icon_emoji: Some(":scream_cat:".to_string()),
            file: None
        })?;
        Ok(())
    }

    let mut queue = std::collections::VecDeque::new();
    loop {
        // Check for new files, add to queue
        match files_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(path) => { queue.push_back(path); },
            Err(e) => {
                match e {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {},
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        error!("File watcher thread disconnected");
                        break;
                    }}}};

        // Process files form queue if rate limit allows
        if !queue.is_empty()
        {
            if upload_limiter.check().is_err() {
                if limit_warning_limiter.check().is_ok() {
                    warn!("Upload rate limit exceeded");
                    post_message(&conf, &BotSlackMessage {
                        title: Some(format!("(Upload rate limit exceeded.)")),
                        text: Some(format!("Note: There are currently too many (>{}) files to upload per minute. Limiting posting rate for now.", conf.limit_uploads_per_minute)),
                        icon_emoji: Some(":snail:".to_string()),
                        file: None
                    })?;
                }
                continue;
            }

            // Post next file
            if let Some(path) = queue.pop_front() {
                let file_basename = path.file_name().ok_or(anyhow!("Invalid file path"))?;
                match handle_file(&path, &conf) {
                    Ok(_) => {
                        let posted_path = posted_dir.join(file_basename);
                        std::fs::rename(&path, posted_path)?;
                    },
                    Err(e) => {
                        error!("Error handling file: {:?}", e);
                        let rejected_path = rejected_dir.join(file_basename);
                        std::fs::rename(&path, rejected_path)?;
        
                        let lossy = file_basename.to_string_lossy().to_string();
                        if let Err(e2) = post_error(&lossy, &conf, &e) {
                            error!("Error posting error message: {:?}", e2);
                        }
                    }
                }        
            }
        }
    }

    watcher_thread.join().unwrap();
    Ok(())
}


/**
 * Main entry point.
 */
fn main() -> anyhow::Result<()>
{
    let argv = std::env::args;
    let args = Docopt::new(USAGE)
        .and_then(|d| d.argv(argv().into_iter()).parse())
        .unwrap_or_else(|e| e.exit());

    if args.get_bool("--version") {
        println!("{} {}", NAME, VERSION);
        return Ok(());
    }

    if args.get_bool("--debug") {
        env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .init();
    } else {
        env_logger::builder()
            .filter_level(log::LevelFilter::Info)
            .init();
    }

    let config_file = PathBuf::from(args.get_str("<config_file>"));
    let bots = read_config_file(&config_file)?;

    let mut threads = Vec::new();
    for bot in bots {
        let t = std::thread::spawn(move || {
            bot_thread(bot).unwrap();
        });
        threads.push(t);
    }

    for t in threads {
        t.join().unwrap();
    }
    
    Ok(())
}
