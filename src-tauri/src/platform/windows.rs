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
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, LocalFree, ERROR_ALREADY_EXISTS, ERROR_BUFFER_OVERFLOW, ERROR_CANCELLED,
        ERROR_SERVICE_DOES_NOT_EXIST, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
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
    known_directory(&FOLDERID_ProgramData)
}

fn known_directory(id: &windows_sys::core::GUID) -> Result<PathBuf, AppError> {
    let mut raw = null_mut();
    if unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut raw) } < 0 {
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
    if path.try_exists().map_err(|_| storage_error())? {
        trusted_service_object(path)?;
    }
    let desc = descriptor("O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;RC;;;OW)")?;
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

pub fn secure_service_file(path: &Path) -> Result<(), AppError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    trusted_service_object(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(storage_error());
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| storage_error())?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0
        || info.nNumberOfLinks != 1
    {
        return Err(storage_error());
    }
    let desc = descriptor("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;RC;;;OW)")?;
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

/// Never promote a pre-created user-writable object into trusted recovery state.
fn trusted_service_object(path: &Path) -> Result<(), AppError> {
    use windows_sys::Win32::Security::{
        Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
        GetAce, IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE,
        ACE_HEADER, OWNER_SECURITY_INFORMATION,
    };
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(storage_error());
    }
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut raw = null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut raw,
        )
    } != 0
    {
        return Err(storage_error());
    }
    let _descriptor = LocalAllocation(raw);
    let administrator = |sid: *mut c_void| {
        !sid.is_null()
            && unsafe {
                IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                    || IsWellKnownSid(sid, WinLocalSystemSid) != 0
            }
    };
    if !administrator(owner) || dacl.is_null() {
        return Err(storage_error());
    }
    // Generic write/all, delete, ACL/owner mutation and file/directory write rights.
    const MUTATION: u32 = 0x500D0156;
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut raw_ace = null_mut();
        if unsafe { GetAce(dacl, u32::from(index), &mut raw_ace) } == 0 {
            return Err(storage_error());
        }
        let header = unsafe { &*raw_ace.cast::<ACE_HEADER>() };
        if header.AceType == 1 {
            continue;
        } // Access-denied ACE cannot grant mutation.
        if header.AceType != 0 || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(storage_error());
        }
        let ace = unsafe { &*raw_ace.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = (&ace.SidStart as *const u32).cast_mut().cast();
        if ace.Mask & MUTATION != 0 && !administrator(sid) {
            return Err(storage_error());
        }
    }
    Ok(())
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

fn sid_string(sid: *mut c_void) -> Result<String, AppError> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut value = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(storage_error());
    }
    let _allocation = LocalAllocation(value.cast());
    Ok(unsafe { string_from_wide(value) })
}

pub fn file_owner_sid(path: &Path) -> Result<String, AppError> {
    use windows_sys::Win32::Security::{
        Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
        OWNER_SECURITY_INFORMATION,
    };
    let mut owner = null_mut();
    let mut raw = null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut raw,
        )
    } != 0
    {
        return Err(storage_error());
    }
    let _descriptor = LocalAllocation(raw);
    sid_string(owner)
}

pub fn current_user_sid() -> Result<String, AppError> {
    use windows_sys::Win32::{
        Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(storage_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err(storage_error());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(storage_error());
    }
    sid_string(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn service_control_sddl(controller: Option<&str>) -> Result<String, AppError> {
    // Users can read status/config metadata. Only the approved user can start/stop;
    // changing the executable, service configuration, DACL or owner remains privileged.
    let mut sddl = String::from("D:P(A;;0xF01FF;;;SY)(A;;0xF01FF;;;BA)(A;;0x20005;;;AU)");
    if let Some(sid) = controller {
        if !sid.starts_with("S-1-")
            || sid.len() > 192
            || !sid
                .split('-')
                .skip(1)
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(service_error());
        }
        sddl.push_str(&format!("(A;;0x20035;;;{sid})"));
    }
    Ok(sddl)
}

pub fn authorize_service_controller(controller: Option<&str>) -> Result<(), AppError> {
    let result = service_control_sddl(controller).and_then(|sddl| set_service_security(&sddl));
    result.map_err(|_| {
        AppError::new(
            "service_permissions_failed",
            "网络服务启停权限设置失败，请重新连接以完成系统授权。",
        )
    })
}

pub fn service_control_allowed(start: bool, stop: bool) -> Result<bool, AppError> {
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(service_error());
    }
    let manager = ServiceHandle(manager);
    let access = SERVICE_QUERY_STATUS
        | if start { SERVICE_START } else { 0 }
        | if stop { SERVICE_STOP } else { 0 };
    let service = unsafe { OpenServiceW(manager.0, wide(SERVICE_NAME).as_ptr(), access) };
    if service.is_null() {
        return match unsafe { GetLastError() } {
            windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED | ERROR_SERVICE_DOES_NOT_EXIST => {
                Ok(false)
            }
            _ => Err(service_error()),
        };
    }
    let _service = ServiceHandle(service);
    Ok(true)
}

pub fn service_security() -> Result<Option<String>, AppError> {
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    let Some(service) = service_handle(0x00020000)? else {
        return Ok(None);
    };
    let mut needed = 0;
    unsafe {
        QueryServiceObjectSecurity(
            service.0,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            0,
            &mut needed,
        );
    }
    if needed == 0 || needed > 16 * 1024 {
        return Err(service_error());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        QueryServiceObjectSecurity(
            service.0,
            DACL_SECURITY_INFORMATION,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(service_error());
    }
    let mut raw = null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            buffer.as_mut_ptr().cast(),
            1,
            DACL_SECURITY_INFORMATION,
            &mut raw,
            null_mut(),
        )
    } == 0
    {
        return Err(service_error());
    }
    let _allocation = LocalAllocation(raw.cast());
    let value = unsafe { string_from_wide(raw) };
    if value.len() > 4096 {
        return Err(service_error());
    }
    Ok(Some(value))
}

pub fn set_service_security(sddl: &str) -> Result<(), AppError> {
    if sddl.len() > 4096 || sddl.contains('\0') {
        return Err(service_error());
    }
    let desc = descriptor(sddl)?;
    let Some(service) = service_handle(0x00040000)? else {
        return Ok(());
    };
    if unsafe { SetServiceObjectSecurity(service.0, DACL_SECURITY_INFORMATION, desc.0) } == 0 {
        return Err(service_error());
    }
    Ok(())
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

fn service_binary_command() -> Result<Option<String>, AppError> {
    let Some(service) = service_handle(SERVICE_QUERY_CONFIG)? else {
        return Ok(None);
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
    Ok(Some(unsafe {
        string_from_wide((*config).lpBinaryPathName)
    }))
}

fn service_command_matches(binary: &str, expected: &Path) -> bool {
    let command = wide(binary);
    let mut count = 0;
    let argv =
        unsafe { windows_sys::Win32::UI::Shell::CommandLineToArgvW(command.as_ptr(), &mut count) };
    if argv.is_null() {
        return false;
    }
    let _allocation = LocalAllocation(argv.cast());
    if count != 9 {
        return false;
    }
    let args: Vec<String> = (0..count as usize)
        .map(|index| unsafe { string_from_wide(*argv.add(index)) })
        .collect();
    let Some(root) = expected.parent().and_then(Path::parent) else {
        return false;
    };
    path_argument_matches(&args[0], expected)
        && args[1] == "--disable-env-parsing"
        && args[2] == "--config-file"
        && path_argument_matches(&args[3], &root.join("core.toml"))
        && args[4] == "--rpc-portal"
        && args[5] == crate::network_config::RPC_PORTAL
        && args[6] == "--rpc-portal-whitelist"
        && args[7] == "127.0.0.1/32"
        && args[8] == "--no-listener"
}

fn path_argument_matches(value: &str, expected: &Path) -> bool {
    // std::fs::canonicalize in the official CLI emits extended-length local paths.
    let value = value.strip_prefix(r"\\?\").unwrap_or(value);
    value.eq_ignore_ascii_case(&expected.to_string_lossy())
}

fn service_conflict() -> AppError {
    AppError::new(
        "service_conflict",
        "同名网络服务不属于当前 Rela，无法操作。",
    )
}

pub fn owned_service_binary(allowed: &[PathBuf]) -> Result<Option<PathBuf>, AppError> {
    let Some(binary) = service_binary_command()? else {
        return Ok(None);
    };
    allowed
        .iter()
        .find(|path| service_command_matches(&binary, path))
        .cloned()
        .map(Some)
        .ok_or_else(service_conflict)
}

pub fn delete_service() -> Result<(), AppError> {
    let Some(service) = service_handle(0x00010000)? else {
        return Ok(());
    };
    if unsafe { DeleteService(service.0) } == 0
        && unsafe { GetLastError() }
            != windows_sys::Win32::Foundation::ERROR_SERVICE_MARKED_FOR_DELETE
    {
        return Err(service_error());
    }
    Ok(())
}

pub fn service_description() -> Result<Option<String>, AppError> {
    let Some(service) = service_handle(SERVICE_QUERY_CONFIG)? else {
        return Ok(None);
    };
    let mut needed = 0;
    unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            null_mut(),
            0,
            &mut needed,
        );
    }
    if needed == 0 || needed > 16 * 1024 {
        return Err(service_error());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(service_error());
    }
    let description = unsafe { &*buffer.as_ptr().cast::<SERVICE_DESCRIPTIONW>() };
    if description.lpDescription.is_null() {
        return Ok(None);
    }
    Ok(Some(unsafe { string_from_wide(description.lpDescription) }))
}

pub fn set_service_description(value: &str) -> Result<(), AppError> {
    if value.len() > 4096 || value.contains('\0') {
        return Err(service_error());
    }
    let service = service_handle(SERVICE_CHANGE_CONFIG)?.ok_or_else(service_error)?;
    let mut value = wide(value);
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: value.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            (&description as *const SERVICE_DESCRIPTIONW).cast(),
        )
    } == 0
    {
        return Err(service_error());
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

pub fn elevate_helper(request: &Path) -> Result<bool, AppError> {
    let executable = std::env::current_exe().map_err(|_| service_error())?;
    let process = launch_elevated(&executable, "--rela-core-helper", request)?;
    process.wait(std::time::Duration::from_secs(120))?;
    let mut code = 1;
    if unsafe { GetExitCodeProcess(process.0.as_raw_handle(), &mut code) } == 0 {
        return Err(service_error());
    }
    crate::easytier::helper::decode_exit(code)
}

pub struct ElevatedProcess(OwnedHandle);
impl ElevatedProcess {
    pub fn wait(&self, timeout: std::time::Duration) -> Result<(), AppError> {
        if unsafe {
            WaitForSingleObject(
                self.0.as_raw_handle(),
                timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32,
            )
        } == WAIT_OBJECT_0
        {
            Ok(())
        } else {
            Err(AppError::new(
                "service_timeout",
                "系统操作尚未完成，请稍后查看连接状态。",
            ))
        }
    }
}

fn launch_elevated(
    executable: &Path,
    entry: &'static str,
    request: &Path,
) -> Result<ElevatedProcess, AppError> {
    let file = wide(executable);
    let parameters = wide(format!("{entry} {}", quote_argument(request.as_os_str())));
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
    Ok(ElevatedProcess(unsafe {
        OwnedHandle::from_raw_handle(info.hProcess)
    }))
}

/// Only validated manifest targets reach system applications; never a shell command.
pub fn open_resource(resource: &rela_manifests::Resource) -> Result<(), AppError> {
    use rela_manifests::ResourceKind;
    resource
        .validate()
        .map_err(crate::distribution::manifest_error)?;
    let failed = || {
        AppError::new(
            "resource_open_failed",
            "无法打开资源，请检查系统应用和资源地址。",
        )
    };
    match resource.kind {
        ResourceKind::Ssh => {
            use std::{os::windows::process::CommandExt, process::Command};
            use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
            let mut buffer = [0u16; 512];
            let length =
                unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
            if length == 0 || length >= buffer.len() {
                return Err(failed());
            }
            let executable =
                PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("OpenSSH/ssh.exe");
            if !executable.is_file() {
                return Err(AppError::new(
                    "ssh_unavailable",
                    "请先在 Windows 可选功能中安装 OpenSSH 客户端。",
                ));
            }
            let mut command = Command::new(executable);
            command.args(["-p", &resource.port.expect("validated port").to_string()]);
            if let Some(username) = &resource.username {
                command.args(["-l", username]);
            }
            // The user clicked an interactive SSH entry, so allocate its console.
            command
                .arg("--")
                .arg(&resource.address)
                .creation_flags(0x00000010)
                .spawn()
                .map_err(|_| failed())?;
        }
        ResourceKind::Web => {
            let target = wide(&resource.address);
            let verb = wide("open");
            let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
            info.lpVerb = verb.as_ptr();
            info.lpFile = target.as_ptr();
            info.nShow = 1;
            if unsafe { ShellExecuteExW(&mut info) } == 0 {
                return Err(failed());
            }
        }
        ResourceKind::Nas => {
            use windows_sys::Win32::{
                System::Com::{
                    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
                },
                System::SystemServices::SFGAO_FOLDER,
                UI::Shell::{SHOpenFolderAndSelectItems, SHParseDisplayName},
            };
            let initialized = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
            if initialized < 0 {
                return Err(failed());
            }
            let target = wide(&resource.address);
            let mut item = null_mut();
            let mut attributes = 0;
            let parsed = unsafe {
                SHParseDisplayName(
                    target.as_ptr(),
                    null_mut(),
                    &mut item,
                    SFGAO_FOLDER,
                    &mut attributes,
                )
            };
            let opened = if parsed >= 0 && !item.is_null() && attributes & SFGAO_FOLDER != 0 {
                // Folder-only API: a NAS filename can never become an executable launch.
                unsafe { SHOpenFolderAndSelectItems(item, 0, null(), 0) }
            } else {
                -1
            };
            unsafe {
                if !item.is_null() {
                    CoTaskMemFree(item.cast());
                }
                CoUninitialize();
            }
            if opened < 0 {
                return Err(failed());
            }
        }
    }
    Ok(())
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
    fn service_controller_permissions_grant_only_start_stop_and_queries_to_the_approved_user() {
        use windows_sys::Win32::Security::{GetAce, GetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE};
        let user = "S-1-5-21-1-2-3-1001";
        for controller in [None, Some(user)] {
            let desc = descriptor(&service_control_sddl(controller).unwrap()).unwrap();
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl = null_mut();
            assert_ne!(
                unsafe {
                    GetSecurityDescriptorDacl(desc.0, &mut present, &mut dacl, &mut defaulted)
                },
                0
            );
            assert_ne!(present, 0);
            let mut approved = None;
            for index in 0..unsafe { (*dacl).AceCount } {
                let mut raw = null_mut();
                assert_ne!(unsafe { GetAce(dacl, index.into(), &mut raw) }, 0);
                let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = sid_string((&ace.SidStart as *const u32).cast_mut().cast()).unwrap();
                if sid == user {
                    approved = Some(ace.Mask);
                }
                if sid == "S-1-5-11" {
                    assert_eq!(ace.Mask, 0x20005);
                }
            }
            assert_eq!(approved, controller.map(|_| 0x20035));
            if let Some(mask) = approved {
                assert_eq!(
                    mask & (SERVICE_CHANGE_CONFIG | 0x00010000 | 0x00040000 | 0x00080000),
                    0
                );
            }
        }
        assert!(service_control_sddl(Some("S-1-5-21-1)(A;;GA;;;WD)")).is_err());
    }

    #[test]
    fn helper_authorization_uses_the_request_owner_instead_of_the_elevated_account() {
        let path = std::env::temp_dir().join(format!(
            "rela-request-owner-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"synthetic-encrypted-request").unwrap();
        private_file(&path).unwrap();
        if !is_elevated() {
            assert_eq!(file_owner_sid(&path).unwrap(), current_user_sid().unwrap());
        }
        fs::remove_file(path).unwrap();
    }
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
    fn service_ownership_requires_exact_path_and_restricted_arguments() {
        let path = Path::new(r"C:\Program Data\Rela\engine-v2\easytier-core.exe");
        let command = r#""\\?\C:\Program Data\Rela\engine-v2\easytier-core.exe" --disable-env-parsing --config-file "C:\Program Data\Rela\core.toml" --rpc-portal 127.0.0.1:15888 --rpc-portal-whitelist 127.0.0.1/32 --no-listener"#;
        let command = command.replace("127.0.0.1:15888", crate::network_config::RPC_PORTAL);
        assert!(service_command_matches(&command, path));
        assert!(service_command_matches(&command.replace("C:", "c:"), path));
        for invalid in [
            command.replace("easytier-core.exe", "easytier-core.exe.bad"),
            command.replace("core.toml", "other.toml"),
            command.replace("127.0.0.1/32", "0.0.0.0/0"),
            format!("{command} --no-tun"),
            command.replace("engine-v2", "engine"),
        ] {
            assert!(!service_command_matches(&invalid, path));
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
