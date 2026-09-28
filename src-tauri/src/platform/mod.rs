//! Windows Service、启动项与系统唤醒通知在这里接入。
//! 当前桌面应用始终按普通用户权限运行。
pub const WINDOWS_AGENT_PIPE: &str = r"\\.\pipe\rela-agent-v1";
