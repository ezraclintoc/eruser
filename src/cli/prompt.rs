//! Small helpers for the interactive commands.

use std::io::Write;

/// Read one trimmed line, printing `message` first.
pub fn line(message: &str) -> Result<String, std::io::Error> {
    print!("{message}");
    std::io::stdout().flush()?;

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input)? == 0 {
        // EOF: the user piped input that ran out, or pressed Ctrl-D.
        return Ok(String::new());
    }
    Ok(input.trim().to_string())
}

/// Read a line, substituting `default` when the answer is empty.
pub fn line_or(message: &str, default: &str) -> Result<String, std::io::Error> {
    let answer = line(message)?;
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer
    })
}

/// Read a secret without echoing it.
///
/// The Go version read the app password with the same line reader as
/// everything else, so it appeared in the terminal and in the scrollback.
pub fn secret(message: &str) -> Result<String, std::io::Error> {
    dialoguer::Password::new()
        .with_prompt(message.trim_end_matches([':', ' ']))
        .allow_empty_password(true)
        .interact()
        .map_err(|e| std::io::Error::other(e.to_string()))
}
