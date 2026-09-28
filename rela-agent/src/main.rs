//! 服务工程入口。目前只提供离线状态检查，不安装服务、不启动网络引擎。
use rela_protocol::{AppError, ConnectionStatus};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [command] if command == "--version" => {
            println!("rela-agent {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [command] if command == "status" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ConnectionStatus::default()).unwrap()
            );
            ExitCode::SUCCESS
        }
        [] => {
            println!("rela-agent 初始化骨架\n用法：rela-agent status | --version\nWindows Service 与受限 IPC 尚待实现。");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "{}",
                serde_json::to_string(&AppError::new(
                    "not_implemented",
                    "服务安装和连接控制尚未实现。可使用 status 查看离线状态。"
                ))
                .unwrap()
            );
            ExitCode::FAILURE
        }
    }
}
