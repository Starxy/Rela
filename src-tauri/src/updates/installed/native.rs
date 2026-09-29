//! Restricted Windows surface for an elevated installed-update participant.
//! No system paths, registry key names or shortcut destinations are supplied by
//! a serialized journal. Constructing this surface performs no system writes.
use super::{
    metadata::{Field, Shortcut, Snapshot, Value},
    pending, SystemState, DIRECTORY,
};
use crate::{distribution::package, platform};
use base64::{engine::general_purpose::STANDARD, Engine};
use rela_protocol::AppError;
use std::{
    collections::BTreeMap,
    ffi::{c_void, OsStr},
    fs::{File, OpenOptions},
    io::Read,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{LocalFree, ERROR_FILE_NOT_FOUND},
    Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
            GetSecurityInfo, SE_FILE_OBJECT, SE_REGISTRY_KEY,
        },
        EqualSid, GetAce, IsWellKnownSid, LookupAccountNameW, SetFileSecurityW,
        WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
    },
    System::{Com::CoTaskMemFree, Registry::*},
    UI::Shell::{FOLDERID_CommonPrograms, FOLDERID_PublicDesktop, SHGetKnownFolderPath},
};

const PRODUCT_KEY: &str = r"SOFTWARE\teamsillybees\Rela";
const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Rela";

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
struct SecurityDescriptor(*mut c_void);
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub struct Native {
    root: PathBuf,
    product: Key,
    uninstall: Key,
    shortcuts: BTreeMap<Shortcut, PathBuf>,
    // Deny directory renames while using privileged paths, including custom
    // installation locations under a user-owned ancestor.
    _anchors: Vec<File>,
}

impl Native {
    pub fn open(root: &Path) -> Result<Self, AppError> {
        if !platform::is_elevated() {
            return Err(pending());
        }
        package::plain_directory(root)?;
        let root = root.canonicalize().map_err(|_| pending())?;
        let mut anchors = anchor_directories(&root)?;
        trust_file_object(&root)?;
        // /UPDATE only replaces files in these two existing payload folders.
        // Anchor and check them as well as the root, so a writable/renamable
        // child directory cannot redirect a later privileged file operation.
        for name in ["easytier", "third-party-licenses"] {
            let directory = root.join(name);
            anchors.extend(anchor_directories(&directory)?);
            trust_file_object(&directory)?;
        }
        let product = open_key(PRODUCT_KEY)?;
        let uninstall = open_key(UNINSTALL_KEY)?;
        let mut shortcuts = BTreeMap::new();
        for (id, folder) in [
            (Shortcut::StartMenu, FOLDERID_CommonPrograms),
            (Shortcut::Desktop, FOLDERID_PublicDesktop),
        ] {
            let directory = known_folder(&folder)?;
            anchors.extend(anchor_directories(&directory)?);
            trust_file_object(&directory)?;
            shortcuts.insert(id, directory.join("Rela.lnk"));
        }
        let result = Self {
            root,
            product,
            uninstall,
            shortcuts,
            _anchors: anchors,
        };
        result.validate_root(&result.root)?;
        Ok(result)
    }

    pub(in crate::updates) fn ensure_running_lock(&self) -> Result<(), AppError> {
        let path = self.root.join(super::guard::RUNNING_LOCK);
        if !super::exists(&path)? {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| pending())?;
            file.sync_all().map_err(|_| pending())?;
        }
        self.publish_restored_file(&path)
    }

    pub(in crate::updates) fn create_control(
        &self,
        directory: &Path,
        id: &str,
    ) -> Result<(), AppError> {
        crate::updates::ipc::validate_id(id)?;
        if directory != self.root.join(format!(".rela-installed-staging-{id}"))
            || super::exists(directory)?
        {
            return Err(pending());
        }
        platform::secure_service_directory(directory)
    }
    pub(in crate::updates) fn validate_control(
        &self,
        directory: &Path,
        id: &str,
    ) -> Result<(), AppError> {
        crate::updates::ipc::validate_id(id)?;
        if directory != self.root.join(super::guard::CONTROL)
            && directory != self.root.join(format!(".rela-installed-finished-{id}"))
        {
            return Err(pending());
        }
        package::plain_directory(directory)?;
        trust_file_object(directory)
    }

    fn key(&self, field: Field) -> HKEY {
        if field == Field::InstallRoot {
            self.product.0
        } else {
            self.uninstall.0
        }
    }

    fn shortcut(&self, shortcut: Shortcut) -> Result<Option<Vec<u8>>, AppError> {
        let path = &self.shortcuts[&shortcut];
        if !super::exists(path)? {
            return Ok(None);
        }
        trust_regular_file(path)?;
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .map_err(|_| pending())?
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| pending())?;
        if bytes.len() > 256 * 1024 {
            return Err(pending());
        }
        Ok(Some(bytes))
    }
}

impl SystemState for Native {
    fn validate_root(&self, root: &Path) -> Result<(), AppError> {
        package::plain_directory(root)?;
        if root.canonicalize().map_err(|_| pending())? != self.root {
            return Err(pending());
        }
        trust_file_object(root)?;
        let Some(Value::String(registered)) = read_value(self.product.0, "")? else {
            return Err(pending());
        };
        package::plain_directory(Path::new(&registered))?;
        if Path::new(&registered)
            .canonicalize()
            .map_err(|_| pending())?
            != self.root
        {
            return Err(pending());
        }
        Ok(())
    }

    fn validate_live_file(&self, path: &Path) -> Result<(), AppError> {
        if !path.starts_with(&self.root) {
            return Err(pending());
        }
        trust_regular_file(path)
    }

    fn create_session(&self, path: &Path) -> Result<(), AppError> {
        if path != self.root.join(DIRECTORY) || super::exists(path)? {
            return Err(pending());
        }
        platform::secure_service_directory(path)
    }

    fn validate_session(&self, path: &Path) -> Result<(), AppError> {
        if path != self.root.join(DIRECTORY) {
            return Err(pending());
        }
        package::plain_directory(path)?;
        trust_file_object(path)
    }

    fn snapshot(&self) -> Result<Snapshot, AppError> {
        let mut registry = BTreeMap::new();
        for field in Field::ALL {
            registry.insert(field, read_value(self.key(field), field.name())?);
        }
        let mut shortcuts = BTreeMap::new();
        for shortcut in Shortcut::ALL {
            shortcuts.insert(
                shortcut,
                self.shortcut(shortcut)?.map(|bytes| STANDARD.encode(bytes)),
            );
        }
        let result = Snapshot {
            registry,
            shortcuts,
        };
        result.validate()?;
        Ok(result)
    }

    fn restore_registry(&mut self, field: Field, value: Option<&Value>) -> Result<(), AppError> {
        let key = self.key(field);
        let name = wide(field.name());
        let status = if let Some(value) = value {
            let (kind, bytes) = match value {
                Value::String(value) => {
                    if value.encode_utf16().count() > 2047 || value.contains('\0') {
                        return Err(pending());
                    }
                    (
                        REG_SZ,
                        wide(value)
                            .iter()
                            .flat_map(|value| value.to_le_bytes())
                            .collect::<Vec<_>>(),
                    )
                }
                Value::Dword(value) => (REG_DWORD, value.to_le_bytes().to_vec()),
            };
            unsafe {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    kind,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(key, name.as_ptr()) }
        };
        if status != 0 && !(value.is_none() && status == ERROR_FILE_NOT_FOUND) {
            return Err(pending());
        }
        if unsafe { RegFlushKey(key) } != 0 {
            return Err(pending());
        }
        if read_value(key, field.name())?.as_ref() != value {
            return Err(pending());
        }
        Ok(())
    }

    fn restore_shortcut(
        &mut self,
        shortcut: Shortcut,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        let previous = value
            .map(|value| STANDARD.decode(value).map_err(|_| pending()))
            .transpose()?;
        if previous
            .as_ref()
            .is_some_and(|value| value.len() > 256 * 1024)
        {
            return Err(pending());
        }
        let current = self.shortcut(shortcut)?;
        if current == previous {
            return Ok(());
        }
        let path = &self.shortcuts[&shortcut];
        match previous {
            Some(bytes) => {
                platform::atomic_write(path, &bytes)?;
                publish_file(path)?;
            }
            None => {
                // /UPDATE does not create shortcuts for this fixed NSIS layout.
                // A newly appearing shortcut is unrecognized and is preserved.
                return Err(pending());
            }
        }
        if self.shortcut(shortcut)?
            != Some(
                STANDARD
                    .decode(value.ok_or_else(pending)?)
                    .map_err(|_| pending())?,
            )
        {
            return Err(pending());
        }
        Ok(())
    }

    fn publish_restored_file(&self, path: &Path) -> Result<(), AppError> {
        self.validate_live_file(path)?;
        publish_file(path)
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn anchor_directories(path: &Path) -> Result<Vec<File>, AppError> {
    let mut anchors = Vec::new();
    for directory in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        package::plain_directory(directory)?;
        anchors.push(
            OpenOptions::new()
                .access_mode(0x0002_0080)
                .share_mode(3)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(directory)
                .map_err(|_| pending())?,
        );
    }
    Ok(anchors)
}

fn known_folder(id: &windows_sys::core::GUID) -> Result<PathBuf, AppError> {
    let mut raw = null_mut();
    if unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut raw) } < 0 {
        return Err(pending());
    }
    let result = unsafe {
        let mut size = 0;
        while *raw.add(size) != 0 {
            size += 1;
        }
        let value = String::from_utf16(std::slice::from_raw_parts(raw, size));
        CoTaskMemFree(raw.cast());
        value
    }
    .map_err(|_| pending())?;
    Ok(result.into())
}

fn open_key(name: &str) -> Result<Key, AppError> {
    let mut key = null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(name).as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY | 0x0002_0000,
            &mut key,
        )
    } != 0
    {
        return Err(pending());
    }
    let key = Key(key);
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    if unsafe {
        GetSecurityInfo(
            key.0,
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    } != 0
    {
        return Err(pending());
    }
    let _descriptor = SecurityDescriptor(descriptor);
    check_security(owner, dacl, 0x500D_0026)?;
    Ok(key)
}

fn read_value(key: HKEY, name: &str) -> Result<Option<Value>, AppError> {
    let mut kind = 0;
    let mut size = 0;
    let name = wide(name);
    let status =
        unsafe { RegQueryValueExW(key, name.as_ptr(), null(), &mut kind, null_mut(), &mut size) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != 0 || size > 4096 {
        return Err(pending());
    }
    let mut bytes = vec![0u8; size as usize];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    } != 0
        || size as usize > bytes.len()
    {
        return Err(pending());
    }
    bytes.truncate(size as usize);
    let value = match kind {
        REG_DWORD if bytes.len() == 4 => {
            Value::Dword(u32::from_le_bytes(bytes.try_into().map_err(|_| pending())?))
        }
        REG_SZ if bytes.len() >= 2 && bytes.len() % 2 == 0 => {
            let mut chars: Vec<_> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            if chars.pop() != Some(0) || chars.contains(&0) {
                return Err(pending());
            }
            Value::String(String::from_utf16(&chars).map_err(|_| pending())?)
        }
        _ => return Err(pending()),
    };
    Ok(Some(value))
}

pub(super) fn trust_regular_file(path: &Path) -> Result<(), AppError> {
    package::plain_file(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| pending())?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0
        || info.nNumberOfLinks != 1
    {
        return Err(pending());
    }
    trust_file_object(path)
}

fn trust_file_object(path: &Path) -> Result<(), AppError> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    } != 0
    {
        return Err(pending());
    }
    let _descriptor = SecurityDescriptor(descriptor);
    check_security(owner, dacl, 0x500D_0156)
}

fn check_security(owner: *mut c_void, dacl: *mut ACL, mutations: u32) -> Result<(), AppError> {
    // Program Files may legitimately be owned/writable by TrustedInstaller.
    // Resolve the system service SID, never a user-supplied account name.
    let mut service_sid = vec![0u8; 256];
    let mut sid_size = service_sid.len() as u32;
    let mut domain = vec![0u16; 256];
    let mut domain_size = domain.len() as u32;
    let mut usage = 0;
    let found = unsafe {
        LookupAccountNameW(
            null(),
            wide(r"NT SERVICE\TrustedInstaller").as_ptr(),
            service_sid.as_mut_ptr().cast(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut usage,
        )
    } != 0;
    let trusted = |sid: *mut c_void| {
        !sid.is_null()
            && unsafe {
                IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                    || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || (found && EqualSid(sid, service_sid.as_ptr().cast_mut().cast()) != 0)
            }
    };
    if !trusted(owner) || dacl.is_null() {
        return Err(pending());
    }
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut raw = null_mut();
        if unsafe { GetAce(dacl, u32::from(index), &mut raw) } == 0 {
            return Err(pending());
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if header.AceFlags & 8 != 0 || header.AceType == 1 {
            continue;
        } // inherit-only / deny
        if header.AceType != 0 || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(pending());
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Mask & mutations != 0 && !trusted((&ace.SidStart as *const u32).cast_mut().cast()) {
            return Err(pending());
        }
    }
    Ok(())
}

fn publish_file(path: &Path) -> Result<(), AppError> {
    trust_regular_file(path)?;
    let mut raw = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;BU)(A;;RC;;;OW)").as_ptr(),
            1,
            &mut raw,
            null_mut(),
        )
    } == 0
    {
        return Err(pending());
    }
    let _descriptor = SecurityDescriptor(raw);
    if unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            raw,
        )
    } == 0
    {
        return Err(pending());
    }
    trust_regular_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, GetSecurityDescriptorOwner};

    // Inspect only in-memory descriptors. No live file/registry ACL is changed.
    fn accepts(sddl: &str, mutations: u32) -> bool {
        let mut raw = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide(sddl).as_ptr(),
                    1,
                    &mut raw,
                    null_mut(),
                )
            },
            0
        );
        let _descriptor = SecurityDescriptor(raw);
        let mut owner = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut dacl = null_mut();
        assert_ne!(
            unsafe { GetSecurityDescriptorOwner(raw, &mut owner, &mut defaulted) },
            0
        );
        assert_ne!(
            unsafe { GetSecurityDescriptorDacl(raw, &mut present, &mut dacl, &mut defaulted) },
            0
        );
        check_security(owner, dacl, mutations).is_ok()
    }

    #[test]
    fn file_permissions_allow_system_maintenance_and_user_read_only() {
        const MASK: u32 = 0x500D_0156;
        assert!(accepts(
            "O:BAG:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;FRFX;;;BU)(A;;RC;;;OW)",
            MASK
        ));
        assert!(accepts(
            "O:SYG:SYD:P(A;;FA;;;SY)(A;IO;GA;;;CO)(A;;FRFX;;;BU)",
            MASK
        ));
        for access in [
            "GA", "GW", "WD", "WO", "SD", "0x2", "0x4", "0x10", "0x40", "0x100",
        ] {
            assert!(
                !accepts(&format!("O:BAG:BAD:P(A;;FA;;;BA)(A;;{access};;;BU)"), MASK),
                "accepted {access}"
            );
        }
        assert!(!accepts("O:BUG:BUD:P(A;;FRFX;;;BU)(A;;FA;;;BA)", MASK));
        assert!(!accepts("O:BAG:BAD:NO_ACCESS_CONTROL", MASK));
    }

    #[test]
    fn registry_permissions_reject_untrusted_set_create_link_or_owner_changes() {
        const MASK: u32 = 0x500D_0026;
        assert!(accepts(
            "O:BAG:BAD:P(A;;KA;;;BA)(A;;KA;;;SY)(A;;KR;;;BU)",
            MASK
        ));
        for access in [
            "KA", "KW", "GA", "GW", "WD", "WO", "SD", "0x2", "0x4", "0x20",
        ] {
            assert!(
                !accepts(&format!("O:BAG:BAD:P(A;;KA;;;BA)(A;;{access};;;BU)"), MASK),
                "accepted {access}"
            );
        }
    }
}
