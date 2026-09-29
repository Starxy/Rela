use rela_protocol::AppError;
use std::{
    ffi::OsString,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT: u64 = 1024 * 1024;

/// Core/CLI 输出可能含密钥，只在内存中解析；错误只返回受控消息。
pub fn output(
    executable: &Path,
    args: &[OsString],
    timeout: Duration,
) -> Result<Vec<u8>, AppError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(executable.parent().ok_or_else(unavailable)?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|_| unavailable())?;
    let stdout = child.stdout.take().ok_or_else(unavailable)?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(AppError::new(
                    "core_timeout",
                    "网络引擎未及时响应，请稍后重试。",
                ));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    if !status?.success() {
        return Err(AppError::new(
            "core_command_failed",
            "网络引擎操作未完成，请检查引擎状态或重新连接。",
        ));
    }
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(unavailable());
    }
    Ok(bytes)
}

fn unavailable() -> AppError {
    AppError::new("core_unavailable", "无法运行网络引擎，请重新安装 Rela。")
}
