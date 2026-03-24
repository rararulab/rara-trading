//! CLI executor for running prompts through backends.
//!
//! Executes prompts via CLI tools with real-time streaming output.
//! Supports optional execution timeout with graceful SIGTERM termination.
//! Ported from ralph-orchestrator.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use super::backend::{CliBackend, CommandSpec};

/// Result of a CLI execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// The full stdout output from the CLI.
    pub output: String,
    /// Captured stderr output (separate from stdout).
    pub stderr: String,
    /// Whether the execution succeeded (exit code 0).
    pub success: bool,
    /// The exit code.
    pub exit_code: Option<i32>,
    /// Whether the execution was terminated due to timeout.
    pub timed_out: bool,
}

/// Executor for running prompts through CLI backends.
#[derive(Debug)]
pub struct CliExecutor {
    backend: CliBackend,
}

/// Internal event type for multiplexing stdout and stderr streams.
enum StreamEvent {
    /// A line was read from stdout.
    StdoutLine(String),
    /// A line was read from stderr.
    StderrLine(String),
    /// stdout reached EOF.
    StdoutEof,
    /// stderr reached EOF.
    StderrEof,
}

/// Identifies which stream an event originated from.
enum StreamKind {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl CliExecutor {
    /// Creates a new executor with the given backend.
    pub const fn new(backend: CliBackend) -> Self {
        Self { backend }
    }

    /// Executes a prompt and streams output to the provided writer.
    ///
    /// Output is streamed line-by-line to the writer while being accumulated
    /// for the return value. If `timeout` is provided and the execution produces
    /// no stdout/stderr activity for longer than that duration, the process
    /// is terminated and the result indicates timeout.
    ///
    /// When `verbose` is true, stderr output is also written to the output writer
    /// with a `[stderr]` prefix. When false, stderr is captured but not displayed.
    pub async fn execute<W: Write + Send>(
        &self,
        prompt: &str,
        mut output_writer: W,
        timeout: Option<Duration>,
        verbose: bool,
    ) -> std::io::Result<ExecutionResult> {
        let spec = self.backend.build_command(prompt, false);
        let mut child = self.spawn_child(&spec)?;

        // Write to stdin if needed, then close to signal EOF
        if let Some(input) = spec.stdin_input.as_deref()
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin.write_all(input.as_bytes()).await?;
            drop(stdin);
        }

        let (accumulated_output, accumulated_stderr, timed_out) =
            self.read_output(&mut child, &mut output_writer, timeout, verbose).await?;

        let status = child.wait().await?;

        Ok(ExecutionResult {
            output: accumulated_output,
            stderr: accumulated_stderr,
            success: status.success() && !timed_out,
            exit_code: status.code(),
            timed_out,
        })
    }

    /// Spawns the child process from a [`CommandSpec`].
    fn spawn_child(&self, spec: &CommandSpec) -> std::io::Result<tokio::process::Child> {
        let mut command = Command::new(&spec.command);
        command.args(&spec.args);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        // Set working directory to current directory
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        command.current_dir(&cwd);

        // Apply backend-specific environment variables (e.g., Agent Teams env var)
        command.envs(self.backend.env_vars.iter().map(|(k, v)| (k, v)));

        debug!(
            command = %spec.command,
            args = ?spec.args,
            cwd = ?cwd,
            "Spawning CLI command"
        );

        if spec.stdin_input.is_some() {
            command.stdin(Stdio::piped());
        }

        command.spawn()
    }

    /// Reads stdout and stderr concurrently, applying inactivity timeout.
    ///
    /// Returns `(stdout_output, stderr_output, timed_out)`.
    async fn read_output<W: Write + Send>(
        &self,
        child: &mut tokio::process::Child,
        output_writer: &mut W,
        timeout: Option<Duration>,
        verbose: bool,
    ) -> std::io::Result<(String, String, bool)> {
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);

        let stdout_task = stdout_handle.map(|stdout| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stdout, tx, StreamKind::Stdout).await })
        });
        let stderr_task = stderr_handle.map(|stderr| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stderr, tx, StreamKind::Stderr).await })
        });
        drop(event_tx);

        let mut stdout_done = stdout_task.is_none();
        let mut stderr_done = stderr_task.is_none();
        let mut accumulated_output = String::new();
        let mut accumulated_stderr = String::new();
        let mut timed_out = false;

        if let Some(duration) = timeout {
            debug!(
                timeout_secs = duration.as_secs(),
                "Executing with inactivity timeout"
            );
        }

        while !stdout_done || !stderr_done {
            let next_event = if let Some(duration) = timeout {
                if let Ok(event) = tokio::time::timeout(duration, event_rx.recv()).await {
                    event
                } else {
                    warn!(
                        timeout_secs = duration.as_secs(),
                        "Execution inactivity timeout reached, terminating process"
                    );
                    timed_out = true;
                    Self::terminate_child(child);
                    break;
                }
            } else {
                event_rx.recv().await
            };

            match next_event {
                Some(StreamEvent::StdoutLine(line)) => {
                    writeln!(output_writer, "{line}")?;
                    output_writer.flush()?;
                    accumulated_output.push_str(&line);
                    accumulated_output.push('\n');
                }
                Some(StreamEvent::StderrLine(line)) => {
                    if verbose {
                        writeln!(output_writer, "[stderr] {line}")?;
                        output_writer.flush()?;
                    }
                    accumulated_stderr.push_str(&line);
                    accumulated_stderr.push('\n');
                }
                Some(StreamEvent::StdoutEof) => stdout_done = true,
                Some(StreamEvent::StderrEof) => stderr_done = true,
                None => {
                    stdout_done = true;
                    stderr_done = true;
                }
            }
        }

        if let Some(handle) = stdout_task {
            handle.await.map_err(join_error_to_io)??;
        }
        if let Some(handle) = stderr_task {
            handle.await.map_err(join_error_to_io)??;
        }

        Ok((accumulated_output, accumulated_stderr, timed_out))
    }

    /// Terminates the child process gracefully via SIGTERM (Unix).
    #[cfg(unix)]
    fn terminate_child(child: &tokio::process::Child) {
        if let Some(pid) = child.id() {
            #[allow(clippy::cast_possible_wrap)]
            let pid = Pid::from_raw(pid as i32);
            debug!(%pid, "Sending SIGTERM to child process");
            let _ = kill(pid, Signal::SIGTERM);
        }
    }

    /// Terminates the child process via `start_kill()` (non-Unix).
    #[cfg(not(unix))]
    fn terminate_child(child: &mut tokio::process::Child) {
        let _ = child.start_kill();
    }

    /// Executes a prompt without streaming (captures all output).
    ///
    /// Uses no timeout by default. For timed execution, use
    /// `execute_capture_with_timeout`.
    pub async fn execute_capture(&self, prompt: &str) -> std::io::Result<ExecutionResult> {
        self.execute_capture_with_timeout(prompt, None).await
    }

    /// Executes a prompt without streaming, with optional timeout.
    pub async fn execute_capture_with_timeout(
        &self,
        prompt: &str,
        timeout: Option<Duration>,
    ) -> std::io::Result<ExecutionResult> {
        let sink = std::io::sink();
        self.execute(prompt, sink, timeout, false).await
    }
}

async fn read_stream<R>(
    stream: R,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
    stream_kind: StreamKind,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let event = match stream_kind {
            StreamKind::Stdout => StreamEvent::StdoutLine(line),
            StreamKind::Stderr => StreamEvent::StderrLine(line),
        };
        if tx.send(event).await.is_err() {
            return Ok(());
        }
    }

    let eof_event = match stream_kind {
        StreamKind::Stdout => StreamEvent::StdoutEof,
        StreamKind::Stderr => StreamEvent::StderrEof,
    };
    let _ = tx.send(eof_event).await;
    Ok(())
}

#[allow(clippy::needless_pass_by_value)] // Used with map_err which passes by value
fn join_error_to_io(error: tokio::task::JoinError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{CliExecutor, Duration};
    use crate::agent::backend::{CliBackend, OutputFormat, PromptMode};

    #[tokio::test]
    async fn test_execute_echo() {
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("hello world", &mut output, None, true)
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.timed_out);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_execute_stdin() {
        let backend = CliBackend {
            command: "cat".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("stdin test").await.unwrap();

        assert!(result.success);
        assert!(result.output.contains("stdin test"));
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let backend = CliBackend {
            command: "false".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("").await.unwrap();

        assert!(!result.success);
        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let backend = CliBackend {
            command: "sleep".to_string(),
            args: vec!["10".to_string()],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let timeout = Some(Duration::from_millis(100));
        let result = executor
            .execute_capture_with_timeout("", timeout)
            .await
            .unwrap();

        assert!(result.timed_out, "Expected execution to time out");
        assert!(
            !result.success,
            "Timed out execution should not be successful"
        );
    }

    #[tokio::test]
    async fn test_execute_timeout_resets_on_output_activity() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let timeout = Some(Duration::from_millis(300));
        let result = executor
            .execute_capture_with_timeout(
                "printf 'start\\n'; sleep 0.2; printf 'middle\\n'; sleep 0.2; printf 'done\\n'",
                timeout,
            )
            .await
            .unwrap();

        assert!(
            !result.timed_out,
            "Periodic output should reset the inactivity timeout"
        );
        assert!(result.success, "Periodic-output command should succeed");
        assert!(result.output.contains("start"));
        assert!(result.output.contains("middle"));
        assert!(result.output.contains("done"));
    }

    #[tokio::test]
    async fn test_execute_streams_output_before_inactivity_timeout() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "printf 'hello\\n'; sleep 10".to_string()],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, Some(Duration::from_millis(200)), false)
            .await
            .unwrap();

        assert!(
            result.timed_out,
            "Expected inactivity timeout after output stops"
        );
        assert_eq!(String::from_utf8(output).unwrap(), "hello\n");
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_no_timeout_when_fast() {
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let timeout = Some(Duration::from_secs(10));
        let result = executor
            .execute_capture_with_timeout("fast", timeout)
            .await
            .unwrap();

        assert!(!result.timed_out, "Fast command should not time out");
        assert!(result.success);
        assert!(result.output.contains("fast"));
    }

    #[tokio::test]
    async fn test_stderr_not_mixed_into_output() {
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo stdout_line; echo stderr_line >&2".to_string(),
            ],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let result = executor.execute_capture("").await.unwrap();

        assert!(result.output.contains("stdout_line"));
        assert!(!result.output.contains("stderr_line"));
        assert!(result.stderr.contains("stderr_line"));
    }
}
