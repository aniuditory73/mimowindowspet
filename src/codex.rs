use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::ptr::null;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::files::Scope;

const PROBE: Duration = Duration::from_secs(10);
const RUN_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub fn check(cancelled: &impl Fn() -> bool) -> io::Result<String> {
    let (exe, version) = installed(cancelled)?;
    let logged_in = invoke(
        &exe,
        exe.parent().unwrap(),
        &["login", "status"],
        None,
        PROBE,
        cancelled,
    )?
    .status
    .success();
    let auth = if logged_in {
        "Авторизация найдена"
    } else {
        "Требуется вход через Codex CLI"
    };
    Ok(format!("{version}\n{auth}\n{}", exe.display()))
}

fn installed(cancelled: &impl Fn() -> bool) -> io::Result<(PathBuf, String)> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let app = std::env::current_exe()?;
    let exe = std::env::split_paths(&paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("codex.exe"))
        .filter_map(|path| path.canonicalize().ok())
        .find(|path| path.is_file() && path != &app)
        .or_else(desktop_cli)
        .ok_or(io::ErrorKind::NotFound)?;
    let version = invoke(
        &exe,
        exe.parent().unwrap(),
        &["--version"],
        None,
        PROBE,
        cancelled,
    )?;
    let version_text = String::from_utf8_lossy(&version.stdout);
    let version_text = version_text.trim();
    if !version.status.success()
        || !version_text.starts_with("codex-cli ")
        || version_text.len() > 80
        || !version_text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || " .-+".contains(character))
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "invalid Codex version response",
        ));
    }
    let help = invoke(
        &exe,
        exe.parent().unwrap(),
        &["exec", "--help"],
        None,
        PROBE,
        cancelled,
    )?;
    let help_text = String::from_utf8_lossy(&help.stdout);
    if !help.status.success()
        || !["--json", "--ephemeral", "--ignore-user-config", "--disable"]
            .iter()
            .all(|flag| help_text.contains(flag))
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported Codex CLI",
        ));
    }
    Ok((exe, version_text.to_owned()))
}

fn desktop_cli() -> Option<PathBuf> {
    let directory = crate::platform::windows::local_app_data()
        .ok()?
        .join("OpenAI/Codex/bin");
    let _guard = crate::storage::guard_directories(&directory).ok()?;
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let _guard = crate::storage::open_directory(&entry.path()).ok()?;
            let exe = entry.path().join("codex.exe");
            let metadata = std::fs::symlink_metadata(&exe).ok()?;
            use std::os::windows::fs::MetadataExt;
            let is_reparse = metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0;
            if !metadata.is_file() || is_reparse {
                return None;
            }
            Some((metadata.modified().ok()?, exe))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, exe)| exe)
}

pub fn run(scope: Scope, prompt: &str, cancelled: &impl Fn() -> bool) -> io::Result<String> {
    let _workspace = scope.guard()?;
    let (exe, _) = installed(cancelled)?;
    let directory = scope.path();
    let project = serde_json::to_string(directory.to_str().ok_or(io::ErrorKind::InvalidInput)?)?;
    let trust = format!("projects={{ {project} = {{ trust_level = \"untrusted\" }} }}");
    let profile = format!(
        "permissions.mimo-read.filesystem={{ \":minimal\" = \"read\", {project} = \"read\" }}"
    );
    let system_root = std::env::var_os("SystemRoot").ok_or(io::ErrorKind::NotFound)?;
    let probe_executable = PathBuf::from(system_root).join("System32/cmd.exe");
    let probe_path = probe_executable
        .to_str()
        .ok_or(io::ErrorKind::InvalidInput)?;
    let sandbox_probe = [
        "-c",
        "project_root_markers=[]",
        "-c",
        &trust,
        "-c",
        "windows.sandbox=\"mxc\"",
        "-c",
        &profile,
        "-c",
        "permissions.mimo-read.network.enabled=false",
        "sandbox",
        "--include-managed-config",
        "--permission-profile",
        "mimo-read",
        "--",
        probe_path,
        "/d",
        "/c",
        "echo",
        "MIMO_SANDBOX_OK",
    ];
    let probe = invoke(&exe, directory, &sandbox_probe, None, PROBE, cancelled)?;
    if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim() != "MIMO_SANDBOX_OK"
    {
        return Err(io::ErrorKind::Unsupported.into());
    }
    let args = [
        "-a",
        "never",
        "exec",
        "--json",
        "--ephemeral",
        "--ignore-user-config",
        "--skip-git-repo-check",
        "--disable",
        "hooks",
        "--disable",
        "apps",
        "--disable",
        "plugins",
        "-c",
        "mcp_servers={}",
        "-c",
        "project_root_markers=[]",
        "-c",
        "allow_login_shell=false",
        "-c",
        "shell_environment_policy.inherit=\"none\"",
        "-c",
        "analytics.enabled=false",
        "-c",
        "windows.sandbox=\"mxc\"",
        "-c",
        "default_permissions=\"mimo-read\"",
        "-c",
        &profile,
        "-c",
        "permissions.mimo-read.network.enabled=false",
        "-c",
        &trust,
        "--",
        "-",
    ];
    let output = invoke(&exe, directory, &args, Some(prompt), RUN_TIMEOUT, cancelled)?;
    let parsed = parse_output(&output.stdout);
    if output.status.success() {
        return parsed;
    }
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
    .to_ascii_lowercase();
    let kind = if diagnostic.contains("unauthorized")
        || diagnostic.contains("not logged in")
        || diagnostic.contains("401")
    {
        io::ErrorKind::PermissionDenied
    } else if diagnostic.contains("quota")
        || diagnostic.contains("rate limit")
        || diagnostic.contains("429")
    {
        io::ErrorKind::WouldBlock
    } else if diagnostic.contains("network")
        || diagnostic.contains("connect")
        || diagnostic.contains("offline")
    {
        io::ErrorKind::NotConnected
    } else if diagnostic.contains("sandbox") || diagnostic.contains("permission profile") {
        io::ErrorKind::Unsupported
    } else {
        io::ErrorKind::Other
    };
    Err(kind.into())
}

fn parse_output(stdout: &[u8]) -> io::Result<String> {
    let mut answer = None;
    let mut complete = false;
    for line in stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: Value = serde_json::from_slice(line).map_err(|_| io::ErrorKind::InvalidData)?;
        match value["type"].as_str() {
            Some("item.completed") if value["item"]["type"] == "agent_message" => {
                let text = value["item"]["text"]
                    .as_str()
                    .ok_or(io::ErrorKind::InvalidData)?;
                answer = Some(text.chars().take(12000).collect());
            }
            Some("turn.completed") => complete = true,
            Some("turn.failed" | "error") => return Err(io::ErrorKind::Other.into()),
            Some(_) => (),
            None => return Err(io::ErrorKind::InvalidData.into()),
        }
    }
    answer
        .filter(|_| complete)
        .ok_or(io::ErrorKind::InvalidData.into())
}

fn invoke(
    exe: &Path,
    cwd: &Path,
    args: &[&str],
    input: Option<&str>,
    timeout: Duration,
    cancelled: &impl Fn() -> bool,
) -> io::Result<Captured> {
    if cancelled() {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let job = kill_on_close_job()?;
    let mut command = Command::new(exe);
    command
        .args(args)
        .current_dir(cwd)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
    ] {
        command.env_remove(OsStr::new(variable));
    }
    let mut child = command.spawn()?;
    // Both handles are live. Refuse to proceed if Windows cannot bind the child to this job.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
        let error = io::Error::last_os_error();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().unwrap();
        let input = input.as_bytes().to_owned();
        thread::spawn(move || stdin.write_all(&input))
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if timeout.is_zero() {
            break Err(io::ErrorKind::TimedOut.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(error) => break Err(error),
            Ok(None) => (),
        }
        if cancelled() {
            break Err(io::ErrorKind::Interrupted.into());
        }
        if Instant::now() >= deadline {
            break Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    drop(job);
    let _ = child.wait();
    let status = status?;
    if let Some(writer) = writer
        && status.success()
    {
        join(writer)?;
    }
    Ok(Captured {
        status,
        stdout: join(stdout)?,
        stderr: join(stderr)?,
    })
}

fn reader(mut source: impl Read + Send + 'static) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0; 8192];
        let mut exceeded = false;
        loop {
            let count = source.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            if output.len() + count > MAX_OUTPUT {
                exceeded = true;
            }
            if !exceeded {
                output.extend_from_slice(&chunk[..count]);
            }
        }
        if exceeded {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(output)
    })
}

fn join<T>(thread: thread::JoinHandle<io::Result<T>>) -> io::Result<T> {
    thread
        .join()
        .map_err(|_| io::Error::other("Codex I/O worker stopped"))?
}

fn kill_on_close_job() -> io::Result<OwnedHandle> {
    // The unnamed job is private to this process; the RAII handle closes it on all exits.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_a_completed_turn_with_an_answer() {
        let answer = "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Готово\"}}\n";
        assert!(parse_output(answer.as_bytes()).is_err());
        assert_eq!(
            parse_output(format!("{answer}{{\"type\":\"turn.completed\"}}\n").as_bytes()).unwrap(),
            "Готово"
        );
        assert!(parse_output(b"invalid\n").is_err());
        assert!(parse_output(b"{\"type\":\"turn.failed\"}\n").is_err());
    }

    #[test]
    fn reads_piped_output_and_handles_timeout_and_nonzero_exit() {
        let executable = std::env::current_exe().unwrap();
        let cwd = executable.parent().unwrap();
        let captured = invoke(&executable, cwd, &["--list"], None, PROBE, &|| false).unwrap();
        assert!(captured.status.success());
        assert!(String::from_utf8_lossy(&captured.stdout).contains("requires_a_completed_turn"));
        let captured = invoke(
            &executable,
            cwd,
            &["--invalid-test-option"],
            None,
            PROBE,
            &|| false,
        )
        .unwrap();
        assert!(!captured.status.success());
        let result = invoke(&executable, cwd, &["--list"], None, Duration::ZERO, &|| {
            false
        });
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::TimedOut));
        let result = invoke(&executable, cwd, &["--list"], None, PROBE, &|| true);
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::Interrupted));
    }
}
