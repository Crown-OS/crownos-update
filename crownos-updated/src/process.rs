use std::process::{ExitStatus, Stdio};

use anyhow::{Context, bail};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader, Lines},
    process::Command,
};

/// Runs `command`, handing each line of stdout and stderr to `report` as it
/// arrives. A failure names the last line the program wrote to stderr.
pub async fn stream(command: &mut Command, mut report: impl FnMut(&str)) -> anyhow::Result<()> {
    let program = program_name(command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot start {program}"))?;
    let mut stdout = child.stdout.take().map(|pipe| BufReader::new(pipe).lines());
    let mut stderr = child.stderr.take().map(|pipe| BufReader::new(pipe).lines());
    let mut last_error_line = String::new();
    while stdout.is_some() || stderr.is_some() {
        tokio::select! {
            line = next_line(&mut stdout), if stdout.is_some() => match line {
                Some(line) => {
                    emit(&line, &mut report);
                }
                None => stdout = None,
            },
            line = next_line(&mut stderr), if stderr.is_some() => match line {
                Some(line) => {
                    if let Some(latest) = emit(&line, &mut report) {
                        last_error_line = latest.to_owned();
                    }
                }
                None => stderr = None,
            },
        }
    }
    let status = child
        .wait()
        .await
        .with_context(|| format!("cannot wait for {program}"))?;
    if !status.success() {
        bail!(failure(&program, status, &last_error_line));
    }
    Ok(())
}

/// Runs `command` to completion and returns its stdout.
pub async fn capture(command: &mut Command) -> anyhow::Result<String> {
    let program = program_name(command);
    let output = command
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("cannot start {program}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(failure(&program, output.status, &stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn failure(program: &str, status: ExitStatus, error_output: &str) -> String {
    match error_output.trim() {
        "" => format!("{program} failed with {status}"),
        last_words => format!("{program} failed with {status}: {last_words}"),
    }
}

fn program_name(command: &Command) -> String {
    let program = std::path::Path::new(command.as_std().get_program());
    program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

async fn next_line<R: AsyncBufRead + Unpin>(lines: &mut Option<Lines<R>>) -> Option<String> {
    lines.as_mut()?.next_line().await.ok().flatten()
}

/// Progress bars redraw with `\r`; only the latest redraw matters.
fn emit<'a>(line: &'a str, report: &mut impl FnMut(&str)) -> Option<&'a str> {
    let latest = line
        .rsplit('\r')
        .map(str::trim)
        .find(|segment| !segment.is_empty())?;
    report(latest);
    Some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streams_both_pipes_and_reports_the_last_error_line() {
        let script = "echo '(1/2) installing a'; echo 'warning: x' >&2; printf 'dl 10%%\\rdl 90%%\\n'; echo 'error: broke' >&2; exit 3";
        let mut lines = Vec::new();
        let result = stream(Command::new("sh").args(["-c", script]), |line| {
            lines.push(line.to_owned())
        })
        .await;
        assert!(lines.contains(&"dl 90%".to_owned()));
        assert!(!lines.contains(&"dl 10%".to_owned()));
        assert!(lines.contains(&"(1/2) installing a".to_owned()));
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            message.starts_with("sh failed with exit status: 3"),
            "{message}"
        );
        assert!(message.ends_with("error: broke"), "{message}");
    }

    #[tokio::test]
    async fn capture_returns_stdout_and_names_failures() -> anyhow::Result<()> {
        assert_eq!(
            capture(Command::new("sh").args(["-c", "echo 7"])).await?,
            "7\n"
        );
        let failure = capture(Command::new("sh").args(["-c", "echo no >&2; exit 2"])).await;
        let message = failure
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(message.ends_with(": no"), "{message}");
        let silent = capture(Command::new("sh").args(["-c", "exit 4"])).await;
        let message = silent
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert_eq!(message, "sh failed with exit status: 4");
        Ok(())
    }
}
