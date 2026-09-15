pub async fn wait(signal_file: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let file = wait_file(signal_file.clone());
    tokio::pin!(file);
    tokio::select! {
        signal = wait_console() => match signal {
            Err(_) if signal_file.is_some() => file.await,
            other => other,
        },
        signal = &mut file => signal,
    }
}

async fn wait_file(path: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let Some(path) = path else {
        return std::future::pending().await;
    };
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    loop {
        tick.tick().await;
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
}

async fn wait_console() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut ctrl_break = tokio::signal::windows::ctrl_break()?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = ctrl_break.recv() => Ok(()),
        }
    }
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = term.recv() => Ok(()),
        }
    }
}
