//! 极简命令执行 API。
//!
//! 这个模块用于同步执行系统命令，并默认完整读取 `stdout` 和 `stderr`。命令非零退出不会直接
//! 返回错误，需要显式调用 [`Output::raise_for_status`] 才把退出状态转换成错误。

use std::{
    error::Error as StdError,
    fmt,
    process::{Command, ExitStatus},
};

const PREVIEW_CHARS: usize = 500;

/// cmd 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。启动失败、命令字符串解析失败和显式状态码检查失败
/// 都会通过这个类型返回。
pub type Result<T> = std::result::Result<T, Error>;

/// cmd 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分解析错误、启动错误或退出状态错误时，使用
/// [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn StdError + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + 'static) -> Self {
        Self {
            kind,
            source: Some(Box::new(source)),
        }
    }

    /// 返回具体错误类型。
    ///
    /// 调用方可以通过匹配 [`ErrorKind`] 做精细错误处理。
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref()
    }
}

/// cmd 模块的具体错误原因。
///
/// 错误信息会包含操作名、命令文本、退出码和输出预览，方便定位命令执行失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 简单命令字符串解析失败。
    #[error("cmd run `{command}` failed: {message}")]
    Parse {
        /// 调用方传入的命令字符串。
        command: String,
        /// 面向人的解析失败原因。
        message: String,
    },

    /// 启动命令失败。
    #[error("cmd spawn `{command}` failed")]
    Spawn {
        /// 调用方要执行的命令。
        command: String,
    },

    /// 显式检查退出状态时发现命令失败。
    #[error(
        "cmd status `{command}` failed with status {status:?}: stdout `{stdout}`, stderr `{stderr}`"
    )]
    Status {
        /// 调用方执行的命令。
        command: String,
        /// 进程退出码；进程被信号终止时可能没有普通退出码。
        status: Option<i32>,
        /// `stdout` 的安全长度预览。
        stdout: String,
        /// `stderr` 的安全长度预览。
        stderr: String,
    },
}

/// 命令执行结果。
///
/// 这个类型保存命令退出状态、`stdout` 和 `stderr`。输出已经用 UTF-8 lossy 方式转换成文本，
/// 适合脚本和后端常见命令输出场景。
#[derive(Debug)]
pub struct Output {
    command: String,
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Output {
    /// 返回命令是否以成功状态退出。
    ///
    /// 非零退出会返回 `false`，但不会自动变成错误。需要错误语义时调用
    /// [`raise_for_status`](Self::raise_for_status)。
    #[must_use]
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// 返回进程退出码。
    ///
    /// 如果进程被信号终止而没有普通退出码，会返回 `None`。
    #[must_use]
    pub fn status_code(&self) -> Option<i32> {
        self.status.code()
    }

    /// 返回标准输出文本。
    ///
    /// 输出使用 UTF-8 lossy 解码，无法解码的字节会替换为 Unicode replacement 字符。
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// 返回标准错误文本。
    ///
    /// 输出使用 UTF-8 lossy 解码，适合读取普通命令错误信息。
    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// 返回主要输出文本。
    ///
    /// 第一版中 `text()` 等价于 [`stdout`](Self::stdout)，对齐脚本里“拿命令文本输出”的常见心智。
    #[must_use]
    pub fn text(&self) -> &str {
        self.stdout()
    }

    /// 显式把非零退出状态转换成错误。
    ///
    /// 成功退出时返回自身引用；非零退出时返回 [`ErrorKind::Status`]，错误中包含命令、退出码、
    /// `stdout` 预览和 `stderr` 预览。
    pub fn raise_for_status(&self) -> Result<&Self> {
        if self.success() {
            return Ok(self);
        }

        Err(ErrorKind::Status {
            command: self.command.clone(),
            status: self.status_code(),
            stdout: preview(&self.stdout),
            stderr: preview(&self.stderr),
        }
        .into())
    }
}

/// 执行简单命令字符串。
///
/// `run` 只支持按空白分词，并支持单引号和双引号包裹参数；它不经过 shell，也不支持管道、
/// 重定向、变量展开、`;`、`&&` 或 `||`。需要 shell 语法时请显式使用 [`shell`]。
pub fn run(command: impl AsRef<str>) -> Result<Output> {
    let command = command.as_ref();
    let parts = parse_command(command)?;
    let program = &parts[0];
    execute(command.trim().to_owned(), program, &parts[1..])
}

/// 直接执行程序和参数。
///
/// 这是安全主路径：程序和参数会直接传给系统进程创建接口，不经过 shell，因此不会执行管道、
/// 变量展开或重定向等 shell 语法。
pub fn exec<A>(program: impl AsRef<str>, args: impl IntoIterator<Item = A>) -> Result<Output>
where
    A: AsRef<str>,
{
    let program = program.as_ref().to_owned();
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    let command = format_command(&program, &args);
    execute(command, &program, &args)
}

/// 显式通过系统 shell 执行命令。
///
/// Unix 使用 `sh -c`，Windows 使用 `cmd /C`。这个函数会启用 shell 的管道、重定向、变量展开等
/// 能力；不要把未信任的用户输入拼进命令字符串，否则会有 shell 注入风险。
pub fn shell(command: impl AsRef<str>) -> Result<Output> {
    let command = command.as_ref().to_owned();

    #[cfg(windows)]
    {
        execute(command.clone(), "cmd", &["/C".to_owned(), command])
    }

    #[cfg(not(windows))]
    {
        execute(command.clone(), "sh", &["-c".to_owned(), command])
    }
}

fn execute(command: String, program: &str, args: &[String]) -> Result<Output> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Spawn {
                    command: command.clone(),
                },
                source,
            )
        })?;

    Ok(Output {
        command,
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn parse_command(command: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut in_token = false;
    let chars = command.chars();

    for ch in chars {
        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else if quote_ch == '"' && ch == '$' {
                return parse_error(command, "run 不支持变量展开");
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                in_token = true;
            }
            ch if ch.is_whitespace() => {
                if in_token {
                    parts.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '|' | '<' | '>' | ';' => {
                return parse_error(command, "run 不支持 shell 管道、重定向或命令分隔符");
            }
            '&' => return parse_error(command, "run 不支持 shell 连接符或后台执行"),
            '$' => return parse_error(command, "run 不支持变量展开"),
            _ => {
                current.push(ch);
                in_token = true;
            }
        }
    }

    if let Some(quote_ch) = quote {
        return parse_error(command, format!("引号 `{quote_ch}` 未闭合"));
    }

    if in_token {
        parts.push(current);
    }

    if parts.is_empty() {
        return parse_error(command, "命令不能为空");
    }

    Ok(parts)
}

fn parse_error<T>(command: &str, message: impl Into<String>) -> Result<T> {
    Err(ErrorKind::Parse {
        command: command.to_owned(),
        message: message.into(),
    }
    .into())
}

fn format_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_owned())
        .chain(args.iter().map(|arg| quote_for_display(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_for_display(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_owned()
    }
}

fn preview(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars();

    for _ in 0..PREVIEW_CHARS {
        let Some(ch) = chars.next() else {
            return output;
        };
        output.push(ch);
    }

    if chars.next().is_some() {
        output.push_str("...");
    }

    output
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn exec_reads_stdout() -> std::result::Result<(), Box<dyn StdError>> {
        let output = exec("rustc", ["--version"])?;

        assert!(output.success());
        assert_eq!(output.status_code(), Some(0));
        assert!(output.stdout().contains("rustc"));
        assert_eq!(output.text(), output.stdout());
        Ok(())
    }

    #[test]
    fn run_parser_supports_whitespace_and_quotes() -> std::result::Result<(), Box<dyn StdError>> {
        let parts = parse_command("git commit -m 'hello world' \"file name.txt\" ''")?;

        assert_eq!(
            parts,
            vec![
                "git".to_owned(),
                "commit".to_owned(),
                "-m".to_owned(),
                "hello world".to_owned(),
                "file name.txt".to_owned(),
                String::new()
            ]
        );
        Ok(())
    }

    #[test]
    fn run_parser_rejects_shell_syntax() -> std::result::Result<(), Box<dyn StdError>> {
        for command in [
            "echo a | grep a",
            "echo a > file",
            "echo a && echo b",
            "echo 'unterminated",
        ] {
            let error = match parse_command(command) {
                Ok(parts) => return Err(format!("expected parse error, got {parts:?}").into()),
                Err(error) => error,
            };

            match error.kind() {
                ErrorKind::Parse {
                    command: actual, ..
                } => assert_eq!(actual, command),
                other => return Err(format!("unexpected error: {other}").into()),
            }
        }

        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_executes_unix_shell_syntax() -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("printf hello | tr a-z A-Z")?;

        assert!(output.success());
        assert_eq!(output.stdout(), "HELLO");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn shell_executes_windows_shell_syntax() -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("echo hello")?;

        assert!(output.success());
        assert!(output.stdout().contains("hello"));
        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn nonzero_exit_returns_output_on_unix() -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("exit 7")?;

        assert!(!output.success());
        assert_eq!(output.status_code(), Some(7));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn nonzero_exit_returns_output_on_windows() -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("exit /B 7")?;

        assert!(!output.success());
        assert_eq!(output.status_code(), Some(7));
        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn raise_for_status_returns_status_error_on_unix() -> std::result::Result<(), Box<dyn StdError>>
    {
        let output = shell("printf out; printf err 1>&2; exit 7")?;

        let error = match output.raise_for_status() {
            Ok(_) => return Err("expected status error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Status {
                command,
                status,
                stdout,
                stderr,
            } => {
                assert_eq!(command, "printf out; printf err 1>&2; exit 7");
                assert_eq!(*status, Some(7));
                assert_eq!(stdout, "out");
                assert_eq!(stderr, "err");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn raise_for_status_returns_status_error_on_windows()
    -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("echo out & echo err 1>&2 & exit /B 7")?;

        let error = match output.raise_for_status() {
            Ok(_) => return Err("expected status error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Status {
                command,
                status,
                stdout,
                stderr,
            } => {
                assert_eq!(command, "echo out & echo err 1>&2 & exit /B 7");
                assert_eq!(*status, Some(7));
                assert!(stdout.contains("out"));
                assert!(stderr.contains("err"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn stdout_stderr_text_status_and_success_are_available_on_unix()
    -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("printf out; printf err 1>&2")?;

        assert!(output.success());
        assert_eq!(output.status_code(), Some(0));
        assert_eq!(output.stdout(), "out");
        assert_eq!(output.stderr(), "err");
        assert_eq!(output.text(), "out");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn stdout_stderr_text_status_and_success_are_available_on_windows()
    -> std::result::Result<(), Box<dyn StdError>> {
        let output = shell("echo out & echo err 1>&2")?;

        assert!(output.success());
        assert_eq!(output.status_code(), Some(0));
        assert!(output.stdout().contains("out"));
        assert!(output.stderr().contains("err"));
        assert_eq!(output.text(), output.stdout());
        Ok(())
    }

    #[test]
    fn missing_command_returns_spawn_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match exec("easy-rust-command-that-does-not-exist-xyz", [""; 0]) {
            Ok(output) => return Err(format!("expected spawn error, got {output:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Spawn { command, .. } => {
                assert_eq!(command, "easy-rust-command-that-does-not-exist-xyz");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
