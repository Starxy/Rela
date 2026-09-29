use super::{storage_error, ServiceState, SERVICE_NAME};
use rela_protocol::AppError;
use std::{
    ffi::{c_void, OsStr},
    fs,
    mem::{size_of, zeroed},
    net::Ipv4Addr,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, ERROR_BUFFER_OVERFLOW,
        ERROR_CANCELLED, ERROR_SERVICE_DOES_NOT_EXIST, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    },
    NetworkManagement::IpHelper::{
        GetAdaptersAddresses, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, GAA_FLAG_SKIP_ANYCAST,
        GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, ICMP_ECHO_REPLY,
        IP_ADAPTER_ADDRESSES_LH,
    },
    Networking::WinSock::{AF_INET, SOCKADDR_IN},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE,
            CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
        SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateDirectoryW, MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH,
    },
    System::{
        Services::*,
        Threading::{GetExitCodeProcess, WaitForSingleObject},
    },
    UI::{
        Shell::{
            FOLDERID_ProgramData, IsUserAnAdmin, SHGetKnownFolderPath, ShellExecuteExW,
            SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        },
        WindowsAndMessaging::SW_HIDE,
    },
};

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub fn protect(plain: &[u8]) -> Result<Vec<u8>, AppError> {
    crypt(plain, true, false)
}
pub fn protect_request(plain: &[u8]) -> Result<Vec<u8>, AppError> {
    crypt(plain, true, true)
}
pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, AppError> {
    crypt(cipher, false, false)
}
fn crypt(bytes: &[u8], encrypt: bool, machine: bool) -> Result<Vec<u8>, AppError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| storage_error())?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut result: CRYPT_INTEGER_BLOB = unsafe { zeroed() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN
                    | if machine {
                        CRYPTPROTECT_LOCAL_MACHINE
                    } else {
                        0
                    },
                &mut result,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut result,
            )
        }
    };
    if ok == 0 {
        return Err(AppError::new(
            "credential_storage_failed",
            "无法读取或保存此 Windows 用户的网络配置，请在设置中恢复默认。",
        ));
    }
    let _allocation = LocalAllocation(result.pbData.cast());
    Ok(unsafe { std::slice::from_raw_parts(result.pbData, result.cbData as usize) }.to_vec())
}

pub fn replace_file(from: &Path, to: &Path) -> Result<(), AppError> {
    if unsafe {
        MoveFileExW(
            wide(from).as_ptr(),
            wide(to).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(storage_error())
    } else {
        Ok(())
    }
}

fn descriptor(sddl: &str) -> Result<LocalAllocation, AppError> {
    let mut raw = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(sddl).as_ptr(),
            1,
            &mut raw,
            null_mut(),
        )
    } == 0
    {
        return Err(storage_error());
    }
    Ok(LocalAllocation(raw))
}

pub fn private_file(path: &Path) -> Result<(), AppError> {
    let desc = descriptor("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)")?;
    if unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            desc.0,
        )
    } == 0
    {
        Err(storage_error())
    } else {
        Ok(())
    }
}

pub fn service_directory() -> Result<PathBuf, AppError> {
    let mut raw = null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, null_mut(), &mut raw) } < 0 {
        return Err(storage_error());
    }
    let path = unsafe {
        let mut len = 0;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let value = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
            raw, len,
        )));
        windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());
        value.join("Rela")
    };
    Ok(path)
}

pub fn secure_service_directory(path: &Path) -> Result<(), AppError> {
    // OWNER RIGHTS prevents a pre-existing unprivileged owner from restoring a writable DACL.
    let desc = descriptor("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;RC;;;OW)")?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: desc.0,
        bInheritHandle: 0,
    };
    if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0
        && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
    {
        return Err(storage_error());
    }
    let info = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    if !info.is_dir() || info.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(storage_error());
    }
    if unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            desc.0,
        )
    } == 0
    {
        return Err(storage_error());
    }
    Ok(())
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn lock_service_control(directory: &Path) -> Result<fs::File, AppError> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(directory.join("control.lock"))
        .map_err(|_| AppError::new("service_busy", "另一项网络操作正在进行，请稍后重试。"))
}

fn service_handle(access: u32) -> Result<Option<ServiceHandle>, AppError> {
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(service_error());
    }
    let manager = ServiceHandle(manager);
    let service = unsafe { OpenServiceW(manager.0, wide(SERVICE_NAME).as_ptr(), access) };
    if service.is_null() {
        if unsafe { GetLastError() } == ERROR_SERVICE_DOES_NOT_EXIST {
            return Ok(None);
        }
        return Err(service_error());
    }
    Ok(Some(ServiceHandle(service)))
}

fn service_error() -> AppError {
    AppError::new(
        "service_unavailable",
        "无法访问 Rela 的网络引擎服务，请检查系统权限。",
    )
}

pub fn service_state() -> Result<ServiceState, AppError> {
    let Some(service) = service_handle(SERVICE_QUERY_STATUS)? else {
        return Ok(ServiceState::NotInstalled);
    };
    let mut status: SERVICE_STATUS_PROCESS = unsafe { zeroed() };
    let mut needed = 0;
    if unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(service_error());
    }
    Ok(match status.dwCurrentState {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_START_PENDING => ServiceState::Starting,
        SERVICE_STOP_PENDING => ServiceState::Stopping,
        _ => ServiceState::Stopped,
    })
}

pub fn stop_service() -> Result<(), AppError> {
    let Some(service) = service_handle(SERVICE_STOP | SERVICE_QUERY_STATUS)? else {
        return Ok(());
    };
    let mut status: SERVICE_STATUS = unsafe { zeroed() };
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0
        && unsafe { GetLastError() } != windows_sys::Win32::Foundation::ERROR_SERVICE_NOT_ACTIVE
    {
        return Err(service_error());
    }
    Ok(())
}

pub fn start_service() -> Result<(), AppError> {
    let service =
        service_handle(SERVICE_START | SERVICE_QUERY_STATUS)?.ok_or_else(service_error)?;
    if unsafe { StartServiceW(service.0, 0, null()) } == 0
        && unsafe { GetLastError() }
            != windows_sys::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING
    {
        return Err(service_error());
    }
    Ok(())
}

pub fn verify_service_binary(expected: &Path) -> Result<(), AppError> {
    let Some(service) = service_handle(SERVICE_QUERY_CONFIG)? else {
        return Ok(());
    };
    let mut needed = 0;
    unsafe {
        QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err(service_error());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe { QueryServiceConfigW(service.0, config, needed, &mut needed) } == 0 {
        return Err(service_error());
    }
    let binary = unsafe { string_from_wide((*config).lpBinaryPathName) };
    let expected = expected.to_string_lossy().to_lowercase();
    let actual = binary.to_lowercase();
    if !actual.starts_with(&format!("\"{expected}\" "))
        && !actual.starts_with(&format!("{expected} "))
    {
        return Err(AppError::new(
            "service_conflict",
            "同名网络服务不属于当前 Rela，无法操作。",
        ));
    }
    Ok(())
}

/// Quote one argument according to CommandLineToArgvW rules, without a shell.
pub fn quote_argument(value: &OsStr) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in value.to_string_lossy().chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        out.push(ch);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

pub fn elevate_helper(request: &Path) -> Result<(), AppError> {
    let executable = std::env::current_exe().map_err(|_| service_error())?;
    let file = wide(&executable);
    let parameters = wide(format!(
        "--rela-core-helper {}",
        quote_argument(request.as_os_str())
    ));
    let verb = wide("runas");
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            AppError::new("permission_cancelled", "已取消系统授权，网络连接未更改。")
        } else {
            AppError::new("permission_required", "需要系统授权才能管理网络连接。")
        });
    }
    if info.hProcess.is_null() {
        return Err(service_error());
    }
    let process = Handle(info.hProcess);
    if unsafe { WaitForSingleObject(process.0, 120_000) } != WAIT_OBJECT_0 {
        return Err(AppError::new(
            "service_timeout",
            "系统操作尚未完成，请稍后查看连接状态。",
        ));
    }
    let mut code = 1;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err(service_error());
    }
    match code {
        0 => Ok(()),
        2 => Err(AppError::new(
            "core_integrity_failed",
            "网络引擎文件校验失败，请重新安装 Rela。",
        )),
        3 => Err(AppError::new(
            "service_conflict",
            "网络服务或管理端口被占用，请先关闭冲突的客户端。",
        )),
        _ => Err(AppError::new(
            "service_action_failed",
            "网络引擎服务操作失败，请检查系统权限并重试。",
        )),
    }
}

unsafe fn string_from_wide(raw: *const u16) -> String {
    if raw.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *raw.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(raw, len))
}

pub fn tun_has_ip(name: &str, ip: Ipv4Addr) -> bool {
    let mut size = 16 * 1024;
    for _ in 0..3 {
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                null(),
                first,
                &mut size,
            )
        };
        if result == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if result != 0 {
            return false;
        }
        let mut adapter = first;
        while !adapter.is_null() {
            unsafe {
                if string_from_wide((*adapter).FriendlyName) == name {
                    let mut unicast = (*adapter).FirstUnicastAddress;
                    while !unicast.is_null() {
                        let socket = (*unicast).Address.lpSockaddr;
                        if !socket.is_null() && (*socket).sa_family == AF_INET {
                            let value = (*(socket.cast::<SOCKADDR_IN>())).sin_addr.S_un.S_addr;
                            if Ipv4Addr::from(value.to_ne_bytes()) == ip {
                                return true;
                            }
                        }
                        unicast = (*unicast).Next;
                    }
                }
                adapter = (*adapter).Next;
            }
        }
        return false;
    }
    false
}

pub fn ping(ip: Ipv4Addr) -> Option<u32> {
    let handle = unsafe { IcmpCreateFile() };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let data = b"Rela";
    let mut buffer = vec![0usize; (size_of::<ICMP_ECHO_REPLY>() + 16).div_ceil(size_of::<usize>())];
    let count = unsafe {
        IcmpSendEcho(
            handle,
            u32::from_ne_bytes(ip.octets()),
            data.as_ptr().cast(),
            data.len() as u16,
            null(),
            buffer.as_mut_ptr().cast(),
            (buffer.len() * size_of::<usize>()) as u32,
            1000,
        )
    };
    unsafe {
        IcmpCloseHandle(handle);
    }
    if count == 0 {
        return None;
    }
    let reply = unsafe { &*buffer.as_ptr().cast::<ICMP_ECHO_REPLY>() };
    (reply.Status == 0).then_some(reply.RoundTripTime)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dpapi_roundtrip_hides_plaintext_and_detects_corruption() {
        let plain = b"rela-test-secret";
        let mut bytes = protect(plain).unwrap();
        assert!(!bytes.windows(plain.len()).any(|part| part == plain));
        assert_eq!(unprotect(&bytes).unwrap(), plain);
        bytes[0] ^= 0xff;
        assert!(unprotect(&bytes).is_err());
    }

    #[test]
    fn windows_arguments_roundtrip_with_spaces_quotes_and_backslashes() {
        for text in [
            "",
            "C:\\My App\\network.json",
            "value\"with quote",
            "C:\\trailing\\",
            "实验室配置",
        ] {
            let command = wide(format!("rela {}", quote_argument(OsStr::new(text))));
            let mut count = 0;
            let argv = unsafe {
                windows_sys::Win32::UI::Shell::CommandLineToArgvW(command.as_ptr(), &mut count)
            };
            assert!(!argv.is_null());
            let _allocation = LocalAllocation(argv.cast());
            assert_eq!(count, 2);
            assert_eq!(unsafe { string_from_wide(*argv.add(1)) }, text);
        }
    }

    #[test]
    fn service_control_lock_blocks_other_handles_and_releases_on_drop() {
        let directory = std::env::temp_dir().join(format!("rela-lock-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let first = lock_service_control(&directory).unwrap();
        assert!(lock_service_control(&directory).is_err());
        drop(first);
        let next = lock_service_control(&directory).unwrap();
        drop(next);
        fs::remove_file(directory.join("control.lock")).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
