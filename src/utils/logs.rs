use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_logs() {
    // Read RUST_LOG (e.g. kovinwm=debug,smithay=info)
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));

    // Stdout (non-blocking)
    let (stdout_nb, _stdout_guard) = tracing_appender::non_blocking(std::io::stdout());

    // File appender (overwrites each run)
    let home_dir = std::env::var("HOME").expect("$HOME is not set");
    let log_path = format!("{}/prefix.log", home_dir);

    // Create a new file, overwriting if it exists
    let file = std::fs::File::create(&log_path).expect("Failed to create log file");
    let (file_nb, _file_guard) = tracing_appender::non_blocking(file);

    // Prevent guards from being dropped
    Box::leak(Box::new((_stdout_guard, _file_guard)));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(stdout_nb).with_target(false))
        .with(
            fmt::layer()
                .with_writer(file_nb)
                .with_ansi(false)
                .with_target(true),
        )
        .init();

    tracing::info!("Logging initialized. Log file: {}", log_path);
}
