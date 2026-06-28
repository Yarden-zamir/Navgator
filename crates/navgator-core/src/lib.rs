use std::{
    env,
    error::Error,
    fs, io,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub type AppResult<T> = Result<T, Box<dyn Error>>;

pub fn ensure_tty_stdin() -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::unix::io::AsRawFd;

        if io::stdin().is_terminal() {
            return Ok(());
        }

        let tty = fs::File::open("/dev/tty")?;
        let result = unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) };
        if result == -1 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}

pub fn write_selection(value: &str) -> AppResult<()> {
    if let Ok(output_path) = env::var("NAVGATOR_OUTPUT") {
        if !output_path.is_empty() {
            fs::write(output_path, value)?;
            return Ok(());
        }
    }
    println!("{value}");
    Ok(())
}

pub fn run_command_output(
    program: &str,
    args: &[String],
    current_dir: Option<&Path>,
) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

pub fn copy_to_clipboard(value: &str) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
        let Some(stdin) = child.stdin.as_mut() else {
            return Err("failed to open pbcopy stdin".into());
        };
        stdin.write_all(value.as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            return Err("pbcopy failed".into());
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = value;
        Err("clipboard copy is only implemented for macOS".into())
    }
}

pub fn fuzzy_match(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query_chars = query.chars().filter(|c| !c.is_whitespace());
    let mut current = query_chars.next();
    if current.is_none() {
        return true;
    }
    for ch in text.chars() {
        if let Some(expected) = current {
            if expected.eq_ignore_ascii_case(&ch) {
                current = query_chars.next();
                if current.is_none() {
                    return true;
                }
            }
        }
    }
    false
}
