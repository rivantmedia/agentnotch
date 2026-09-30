//! Files the app writes on Windows (DESIGN-WIN §3.2 `SecureFiles`, §4.3; WP2): private folders
//! with a protected DACL (the user and SYSTEM only), staged atomic replaces with one rename
//! (`FileRenameInfoEx`, `MoveFileExW` fallback) that keep the target's security or apply the
//! private one, exclusive create, file identity, reparse-point checks, canonical paths, and the
//! 8.3 / long spellings of a path.
//!
//! `WinFiles` exists only on Windows. What can be decided without the OS (the SDDL text, the
//! retry schedule, which error codes mean what, prefix stripping, FILETIME arithmetic) is plain
//! functions here, built and tested on every system.

use std::time::Duration;

/// SYSTEM's SID, which SDDL abbreviates `SY`.
pub const SYSTEM_SID: &str = "S-1-5-18";

/// How many times a refused rename is tried again.
pub const RENAME_RETRIES: usize = 5;

/// Whether `text` has the shape of a string SID (`S-1-5-21-…`). The SID is written into an SDDL
/// string, so anything else (a parenthesis, a semicolon) must never get there.
pub fn is_sid(text: &str) -> bool {
    let mut parts = text.split('-');
    parts.next() == Some("S")
        && parts.clone().count() >= 2
        && parts.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The protected DACL of a private folder or file: full access for the user and SYSTEM, nothing
/// inherited from above (`P`). A folder's entries are inherited by what is created inside it
/// (`OICI`); a file's carry no inheritance flags, which mean nothing there.
pub fn private_sddl(user_sid: &str, folder: bool) -> String {
    let inherit = if folder { "OICI" } else { "" };
    format!("D:P(A;{inherit};FA;;;{user_sid})(A;{inherit};FA;;;SY)")
}

/// The waits before each retry of a refused rename: 5 retries over 500 ms in all. They grow, so
/// a scanner that lets go quickly costs 20 ms and a slow one still gets its half second.
pub fn retry_delays() -> [Duration; RENAME_RETRIES] {
    [20, 60, 100, 140, 180].map(Duration::from_millis)
}

/// Someone else has the file open right now (ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION,
/// ERROR_LOCK_VIOLATION): worth trying again shortly. Nothing else is retried.
pub fn is_transient(code: u32) -> bool {
    matches!(code, 5 | 32 | 33)
}

/// The file system doesn't know `FileRenameInfoEx` (ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED,
/// ERROR_INVALID_PARAMETER: FAT, some network shares, Windows before 10 1607).
pub fn rename_class_unsupported(code: u32) -> bool {
    matches!(code, 1 | 50 | 87)
}

/// The volume keeps no ACLs (ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED: FAT, exFAT), so there
/// is no security to copy.
pub fn no_acl_support(code: u32) -> bool {
    matches!(code, 1 | 50)
}

/// The name is taken (ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS).
pub fn already_exists(code: u32) -> bool {
    matches!(code, 80 | 183)
}

/// The Win32 error code inside an HRESULT, when it wraps one (`0x8007xxxx`).
pub fn win32_code(hresult: i32) -> Option<u32> {
    let bits = hresult as u32;
    (bits >> 16 == 0x8007).then_some(bits & 0xFFFF)
}

/// `GetFinalPathNameByHandleW`'s answer without its `\\?\` prefix: `\\?\C:\x` is `C:\x` and
/// `\\?\UNC\server\share\x` is `\\server\share\x`. Anything else is returned as it is.
pub fn strip_verbatim(path: &str) -> String {
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return path.to_owned();
    };
    match rest.get(..4) {
        Some(unc) if unc.eq_ignore_ascii_case(r"UNC\") => format!(r"\\{}", &rest[4..]),
        _ => rest.to_owned(),
    }
}

/// A FILETIME (100 ns ticks since 1601-01-01 UTC) as nanoseconds since the Unix epoch.
pub fn filetime_to_unix_ns(ticks: i64) -> i128 {
    /// 1601-01-01 to 1970-01-01 in 100 ns ticks.
    const EPOCH_DIFFERENCE: i128 = 116_444_736_000_000_000;
    (i128::from(ticks) - EPOCH_DIFFERENCE) * 100
}

/// The attributes of a replaced file its replacement keeps: hidden, system, archive, not
/// content indexed. None of them set is FILE_ATTRIBUTE_NORMAL, which must stand alone.
pub fn kept_attributes(attributes: u32) -> u32 {
    const KEPT: u32 = 0x2 | 0x4 | 0x20 | 0x2000;
    const NORMAL: u32 = 0x80;
    match attributes & KEPT {
        0 => NORMAL,
        kept => kept,
    }
}

/// Private = the DACL is protected (nothing inherited later from the folder above) and every
/// entry that allows something names the user or SYSTEM.
pub fn only_user_and_system(protected: bool, allowed: &[String], user_sid: &str) -> bool {
    protected
        && allowed
            .iter()
            .all(|sid| sid.eq_ignore_ascii_case(user_sid) || sid.eq_ignore_ascii_case(SYSTEM_SID))
}

#[cfg(windows)]
pub use imp::WinFiles;

#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsString};
    use std::io;
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::ptr;

    use agentnotch_engine::core::atomic::stage_path;
    use agentnotch_engine::platform::{Expect, FileIdentity, SecureFiles, WriteMode, WriteResult};
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL, WIN32_ERROR,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, DeleteFileW, FileIdInfo, FileRenameInfoEx, FlushFileBuffers,
        GetFileAttributesW, GetFileInformationByHandle, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, GetLongPathNameW, GetShortPathNameW, MoveFileExW,
        SetFileInformationByHandle, WriteFile, BY_HANDLE_FILE_INFORMATION, CREATE_NEW, DELETE,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_NAME_NORMALIZED,
        FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_MODE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH, MOVE_FILE_FLAGS, OPEN_EXISTING, WRITE_DAC,
    };

    use super::{
        already_exists, filetime_to_unix_ns, is_sid, is_transient, kept_attributes, no_acl_support,
        only_user_and_system, private_sddl, rename_class_unsupported, retry_delays, strip_verbatim,
        win32_code,
    };

    // The two flags of FILE_RENAME_INFO.Flags this file needs (winbase.h; the crate keeps them in
    // a feature nothing else here uses).
    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x2;
    // ACE_HEADER.AceType values (winnt.h).
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;
    /// ERROR_GEN_FAILURE, for a failure that carries no Win32 code.
    const ERROR_GEN_FAILURE: u32 = 31;
    /// One `WriteFile` call takes a u32 length.
    const WRITE_CHUNK: usize = 1 << 30;

    #[derive(Debug, Default)]
    pub struct WinFiles;

    impl WinFiles {
        pub fn new() -> Self {
            WinFiles
        }
    }

    // --- errors and strings -------------------------------------------------------------------

    fn code_of(error: &windows::core::Error) -> u32 {
        win32_code(error.code().0).unwrap_or(ERROR_GEN_FAILURE)
    }

    /// An `io::Error` whose `kind()` and `raw_os_error()` are the Win32 ones (the crate's own
    /// conversion keeps the HRESULT, which std can't classify).
    fn os_error(error: windows::core::Error) -> io::Error {
        match win32_code(error.code().0) {
            Some(code) => from_code(code),
            None => io::Error::other(error.to_string()),
        }
    }

    fn from_code(code: u32) -> io::Error {
        io::Error::from_raw_os_error(code as i32)
    }

    fn status(result: WIN32_ERROR) -> io::Result<()> {
        if result == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(from_code(result.0))
        }
    }

    fn code_in(error: &io::Error, test: fn(u32) -> bool) -> bool {
        error.raw_os_error().is_some_and(|code| test(code as u32))
    }

    /// The path as a NUL-terminated UTF-16 string.
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut text: Vec<u16> = path.as_os_str().encode_wide().collect();
        if text.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a path can't hold a NUL",
            ));
        }
        text.push(0);
        Ok(text)
    }

    fn wide_text(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    // --- handles ------------------------------------------------------------------------------

    /// A file handle that is closed when dropped, on every path.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful open, is owned by this value alone and
            // is not used after this.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// The one place a file or folder is opened. `path` is NUL-terminated.
    fn open(
        path: &[u16],
        access: u32,
        share: FILE_SHARE_MODE,
        security: Option<&Descriptor>,
        disposition: FILE_CREATION_DISPOSITION,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> io::Result<Handle> {
        let attributes = security.map(Descriptor::attributes);
        let attributes = attributes.as_ref().map(ptr::from_ref);
        // SAFETY: `path` is NUL-terminated and alive for the call; `attributes` points at a
        // SECURITY_ATTRIBUTES on this stack whose descriptor `security` keeps alive.
        // not a pipe: a plain file or folder on disk
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                access,
                share,
                attributes,
                disposition,
                flags,
                None,
            )
        };
        handle.map(Handle).map_err(os_error)
    }

    /// Opens what is there for its metadata only, following links; such an open never conflicts
    /// with anyone's sharing mode, and the backup flag lets it be a folder.
    fn open_existing(path: &[u16]) -> io::Result<Handle> {
        open(
            path,
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
        )
    }

    /// The attributes of the path itself (a link is not followed); `None` when nothing is there.
    fn attributes_of(path: &[u16]) -> io::Result<Option<u32>> {
        // SAFETY: `path` is NUL-terminated and alive for the call.
        let attributes = unsafe { GetFileAttributesW(PCWSTR(path.as_ptr())) };
        if attributes != INVALID_FILE_ATTRIBUTES {
            return Ok(Some(attributes));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        }
    }

    fn delete_quietly(path: &[u16]) {
        // SAFETY: `path` is NUL-terminated and alive for the call.
        let _ = unsafe { DeleteFileW(PCWSTR(path.as_ptr())) };
    }

    // --- security -----------------------------------------------------------------------------

    fn user_sid() -> io::Result<String> {
        crate::sid::current_user_sid()
            .filter(|sid| is_sid(sid))
            .ok_or_else(|| io::Error::other("the user's SID can't be read"))
    }

    /// A security descriptor the system allocated; freed with `LocalFree` when dropped.
    struct Descriptor(PSECURITY_DESCRIPTOR);

    impl Drop for Descriptor {
        fn drop(&mut self) {
            if !self.0 .0.is_null() {
                // SAFETY: the descriptor was allocated with LocalAlloc by the call that returned
                // it, is owned by this value alone and is not used after this.
                let _ = unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
            }
        }
    }

    impl Descriptor {
        /// The user and SYSTEM only, protected.
        fn private(folder: bool) -> io::Result<Self> {
            let sddl = wide_text(&private_sddl(&user_sid()?, folder));
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `sddl` is NUL-terminated and alive for the call; `descriptor` is a valid
            // out pointer and receives a LocalAlloc'd descriptor, which `Descriptor` frees.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(sddl.as_ptr()),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    None,
                )
            }
            .map_err(os_error)?;
            Ok(Descriptor(descriptor))
        }

        /// The DACL (with its protection flag) of what `path` names.
        fn of_path(path: &[u16]) -> io::Result<Self> {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `path` is NUL-terminated and alive for the call; `descriptor` is a valid
            // out pointer and receives a LocalAlloc'd descriptor. It is wrapped before the
            // result is looked at, so it is freed on failure too.
            let result = unsafe {
                GetNamedSecurityInfoW(
                    PCWSTR(path.as_ptr()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    None,
                    None,
                    &mut descriptor,
                )
            };
            let descriptor = Descriptor(descriptor);
            status(result)?;
            if descriptor.0 .0.is_null() {
                return Err(io::Error::other("no security descriptor was returned"));
            }
            Ok(descriptor)
        }

        /// For a create call: the object is born with this security, never the folder's.
        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0 .0,
                bInheritHandle: BOOL(0),
            }
        }

        /// The DACL inside the descriptor (it lives as long as `self`); null when the
        /// descriptor has none, which grants everyone everything.
        fn dacl(&self) -> io::Result<*mut ACL> {
            let mut present = BOOL(0);
            let mut defaulted = BOOL(0);
            let mut dacl: *mut ACL = ptr::null_mut();
            // SAFETY: the descriptor is valid while `self` lives; the three out pointers are
            // valid locals.
            unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }
                .map_err(os_error)?;
            Ok(if present.as_bool() {
                dacl
            } else {
                ptr::null_mut()
            })
        }

        /// Whether the DACL refuses inheritance from the folder above.
        fn protected(&self) -> io::Result<bool> {
            let mut control = 0u16;
            let mut revision = 0u32;
            // SAFETY: the descriptor is valid while `self` lives; both out pointers are valid.
            unsafe { GetSecurityDescriptorControl(self.0, &mut control, &mut revision) }
                .map_err(os_error)?;
            Ok(control & SE_DACL_PROTECTED.0 != 0)
        }

        /// Puts this DACL and its protection flag on the open file. Entries the DACL inherited
        /// are worked out again from the file's folder, which is the target's own folder.
        fn apply_to(&self, file: &Handle) -> io::Result<()> {
            let protection = if self.protected()? {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
            let dacl = self.dacl()?;
            // SAFETY: the handle is open with WRITE_DAC; `dacl` points into the descriptor,
            // which outlives the call (a null DACL is passed as "none", as it was read).
            let result = unsafe {
                SetSecurityInfo(
                    file.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | protection,
                    None,
                    None,
                    Some(dacl.cast_const()),
                    None,
                )
            };
            status(result)
        }

        /// The SIDs this DACL allows something to; `None` when it can't be judged (no DACL, or
        /// a kind of entry this code doesn't know), which is never private.
        fn allowed_sids(&self) -> io::Result<Option<Vec<String>>> {
            let dacl = self.dacl()?;
            if dacl.is_null() {
                return Ok(None);
            }
            let mut size = ACL_SIZE_INFORMATION::default();
            // SAFETY: `dacl` points into the live descriptor; `size` is a valid out buffer of
            // the length passed.
            unsafe {
                GetAclInformation(
                    dacl,
                    ptr::from_mut(&mut size).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            }
            .map_err(os_error)?;
            let mut allowed = Vec::new();
            for index in 0..size.AceCount {
                let mut entry: *mut c_void = ptr::null_mut();
                // SAFETY: `dacl` is live and `index` is below its entry count; `entry` is a
                // valid out pointer.
                unsafe { GetAce(dacl, index, &mut entry) }.map_err(os_error)?;
                // SAFETY: GetAce succeeded, so `entry` points at an entry inside the DACL, and
                // every entry starts with an ACE_HEADER.
                let kind = unsafe { (*entry.cast::<ACE_HEADER>()).AceType };
                match kind {
                    ACCESS_ALLOWED_ACE_TYPE => {
                        // SAFETY: an entry of this type is an ACCESS_ALLOWED_ACE, whose SID
                        // starts at its SidStart field; only the field's address is taken.
                        let sid = unsafe {
                            PSID((&raw mut (*entry.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast())
                        };
                        allowed.push(sid_text(sid)?);
                    }
                    // A refusal lets no one in.
                    ACCESS_DENIED_ACE_TYPE => {}
                    _ => return Ok(None),
                }
            }
            Ok(Some(allowed))
        }
    }

    fn sid_text(sid: PSID) -> io::Result<String> {
        let mut text = PWSTR::null();
        // SAFETY: `sid` points at a SID inside a live DACL; `text` receives a LocalAlloc'd
        // string.
        unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(os_error)?;
        // SAFETY: `text` is the NUL-terminated string the call just returned.
        let result = unsafe { text.to_string() };
        // SAFETY: the string was allocated with LocalAlloc by the call above and is not used
        // after this.
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        result.map_err(|_| io::Error::other("a SID is not valid UTF-16"))
    }

    // --- identity -----------------------------------------------------------------------------

    fn identity_of(file: &Handle) -> io::Result<FileIdentity> {
        let mut basic = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle is open; `basic` is a valid out structure.
        unsafe { GetFileInformationByHandle(file.0, &mut basic) }.map_err(os_error)?;
        let join = |high: u32, low: u32| (u64::from(high) << 32) | u64::from(low);
        let written = join(
            basic.ftLastWriteTime.dwHighDateTime,
            basic.ftLastWriteTime.dwLowDateTime,
        );
        let mut id = FILE_ID_INFO::default();
        // SAFETY: the handle is open; `id` is a valid out buffer of the length passed.
        let full = unsafe {
            GetFileInformationByHandleEx(
                file.0,
                FileIdInfo,
                ptr::from_mut(&mut id).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        // The 128-bit id needs NTFS or ReFS; elsewhere the 64-bit index is all there is.
        let (volume, index) = match full {
            Ok(()) => (
                id.VolumeSerialNumber,
                u128::from_le_bytes(id.FileId.Identifier),
            ),
            Err(_) => (
                u64::from(basic.dwVolumeSerialNumber),
                u128::from(join(basic.nFileIndexHigh, basic.nFileIndexLow)),
            ),
        };
        Ok(FileIdentity {
            volume,
            index,
            modified_ns: filetime_to_unix_ns(written as i64),
            size: join(basic.nFileSizeHigh, basic.nFileSizeLow),
        })
    }

    /// Whether `path` is still what `expect` says, read from a handle on it. `Some(result)`
    /// when it isn't.
    fn check(path: &[u16], expect: Expect) -> io::Result<Option<WriteResult>> {
        if expect == Expect::Nothing {
            return Ok(None);
        }
        let current = match open_existing(path) {
            Ok(file) => Some(identity_of(&file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(match (expect, current) {
            (Expect::Nothing, _) | (Expect::Absent, None) => None,
            (Expect::Absent, Some(_)) => Some(WriteResult::Changed),
            (Expect::Same(_), None) => Some(WriteResult::Vanished),
            (Expect::Same(expected), Some(found)) if found == expected => None,
            (Expect::Same(_), Some(_)) => Some(WriteResult::Changed),
        })
    }

    // --- staging and the rename ---------------------------------------------------------------

    /// Creates the stage. It must not exist (`CREATE_NEW`), is born with `security` when given
    /// (else it inherits the folder's), and can be renamed and re-secured through its handle.
    fn create_stage(
        stage: &[u16],
        security: Option<&Descriptor>,
        attributes: u32,
    ) -> io::Result<Handle> {
        open(
            stage,
            FILE_GENERIC_WRITE.0 | DELETE.0 | WRITE_DAC.0,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            security,
            CREATE_NEW,
            FILE_FLAGS_AND_ATTRIBUTES(attributes),
        )
    }

    fn write_all(file: &Handle, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let chunk = &bytes[..bytes.len().min(WRITE_CHUNK)];
            let mut written = 0u32;
            // SAFETY: the handle is open for writing; `chunk` and `written` are alive for the
            // call, which is synchronous (no OVERLAPPED).
            unsafe { WriteFile(file.0, Some(chunk), Some(&mut written), None) }
                .map_err(os_error)?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn flush(file: &Handle) -> io::Result<()> {
        // SAFETY: the handle is open for writing.
        unsafe { FlushFileBuffers(file.0) }.map_err(os_error)
    }

    /// A FILE_RENAME_INFO naming `target` (NUL-terminated), in a buffer aligned for it, and its
    /// size in bytes.
    fn rename_info(target: &[u16], replace: bool) -> (Vec<u64>, u32) {
        let name = &target[..target.len() - 1];
        let name_bytes = size_of_val(name);
        // The structure ends in the name; room for the NUL after it too.
        let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
        let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        let flags = if replace {
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS
        } else {
            FILE_RENAME_FLAG_POSIX_SEMANTICS
        };
        // SAFETY: `buffer` is zeroed, aligned for FILE_RENAME_INFO and at least `bytes` long,
        // so every field written and the whole name lie inside it. RootDirectory stays null:
        // the name is a full path.
        unsafe {
            (&raw mut (*info).Anonymous.Flags).write(flags);
            (&raw mut (*info).FileNameLength).write(name_bytes as u32);
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                (&raw mut (*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        (buffer, bytes as u32)
    }

    fn rename_by_handle(file: &Handle, info: &(Vec<u64>, u32)) -> Result<(), u32> {
        // SAFETY: the handle is open with DELETE access; the buffer holds a FILE_RENAME_INFO
        // of the size passed and is alive for the call.
        unsafe {
            SetFileInformationByHandle(file.0, FileRenameInfoEx, info.0.as_ptr().cast(), info.1)
        }
        .map_err(|error| code_of(&error))
    }

    fn move_file(stage: &[u16], target: &[u16], flags: MOVE_FILE_FLAGS) -> Result<(), u32> {
        // SAFETY: both paths are NUL-terminated and alive for the call.
        unsafe { MoveFileExW(PCWSTR(stage.as_ptr()), PCWSTR(target.as_ptr()), flags) }
            .map_err(|error| code_of(&error))
    }

    /// The one rename of the stage onto `target`: all or nothing, so the target holds its old
    /// bytes or the new ones at every instant. With `replace` false an existing target is an
    /// error instead. Someone holding the target open without sharing delete refuses the rename;
    /// that is retried on the schedule and then given up, the target untouched. The stage's
    /// handle is consumed (closed) either way.
    fn rename_onto(file: Handle, stage: &[u16], target: &[u16], replace: bool) -> io::Result<()> {
        let info = rename_info(target, replace);
        let fallback = if replace {
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
        } else {
            MOVEFILE_WRITE_THROUGH
        };
        let mut file = Some(file);
        let mut delays = retry_delays().into_iter();
        loop {
            let by_handle = file.as_ref().map(|open| rename_by_handle(open, &info));
            let outcome = match by_handle {
                Some(Err(code)) if rename_class_unsupported(code) => {
                    // This file system lacks the class. MoveFileExW opens the stage itself,
                    // so our handle goes first; every later try uses it too.
                    file = None;
                    move_file(stage, target, fallback)
                }
                Some(result) => result,
                None => move_file(stage, target, fallback),
            };
            match outcome {
                Ok(()) => return Ok(()),
                Err(code) if is_transient(code) => match delays.next() {
                    Some(delay) => std::thread::sleep(delay),
                    None => return Err(from_code(code)),
                },
                Err(code) => return Err(from_code(code)),
            }
        }
    }

    /// The target's DACL for `KeepTargetSecurity`. `Ok(None)` on a volume without ACLs.
    fn security_to_keep(target: &[u16]) -> io::Result<Option<Descriptor>> {
        match Descriptor::of_path(target) {
            Ok(descriptor) => Ok(Some(descriptor)),
            Err(error) if code_in(&error, no_acl_support) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// What a stage is made of.
    struct Staging<'a> {
        bytes: &'a [u8],
        /// The security the stage is created with; `None` inherits the folder's.
        born_with: Option<&'a Descriptor>,
        /// The replaced file's DACL, settled on the stage before the rename.
        keep: Option<&'a Descriptor>,
        attributes: u32,
    }

    /// Stages the bytes and renames the stage over `target`. `Ok(Some(result))` when the last
    /// check found the target changed or gone; the caller then removes the stage, as it does
    /// on an error.
    fn stage_and_replace(
        stage: &[u16],
        target: &[u16],
        staging: &Staging<'_>,
        expect: Expect,
    ) -> io::Result<Option<WriteResult>> {
        let file = create_stage(stage, staging.born_with, staging.attributes)?;
        write_all(&file, staging.bytes)?;
        if let Some(keep) = staging.keep {
            // Born with the target's DACL already; this also settles the protection flag and
            // the inherited entries exactly as the target had them.
            match keep.apply_to(&file) {
                Err(error) if !code_in(&error, no_acl_support) => return Err(error),
                _ => {}
            }
        }
        flush(&file)?;
        // The last moment: someone may have written or removed it meanwhile.
        if let Some(result) = check(target, expect)? {
            return Ok(Some(result));
        }
        rename_onto(file, stage, target, true)?;
        Ok(None)
    }

    // --- paths --------------------------------------------------------------------------------

    /// Calls a "fill this buffer with a path" function (it returns the length written, or the
    /// size needed when the buffer is too small, or 0 on failure) until the answer fits.
    fn path_from(fill: impl Fn(&mut [u16]) -> u32) -> io::Result<OsString> {
        let mut buffer = vec![0u16; 512];
        loop {
            let length = fill(&mut buffer) as usize;
            if length == 0 {
                return Err(io::Error::last_os_error());
            }
            if length < buffer.len() {
                return Ok(OsString::from_wide(&buffer[..length]));
            }
            buffer.resize(length + 1, 0);
        }
    }

    impl SecureFiles for WinFiles {
        fn ensure_private_dir(&self, dir: &Path) -> io::Result<()> {
            let target = wide(dir)?;
            let mut existing = attributes_of(&target)?;
            if existing.is_none() {
                if let Some(parent) = dir.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let security = Descriptor::private(true)?;
                let attributes = security.attributes();
                // SAFETY: `target` is NUL-terminated; `attributes` and the descriptor it
                // points at are alive for the call.
                let created =
                    unsafe { CreateDirectoryW(PCWSTR(target.as_ptr()), Some(&attributes)) };
                match created {
                    // Born private: nobody else could read it even for a moment.
                    Ok(()) => return Ok(()),
                    // Someone made it meanwhile: secure that one, below.
                    Err(error) if already_exists(code_of(&error)) => {
                        existing = attributes_of(&target)?;
                    }
                    Err(error) => return Err(os_error(error)),
                }
            }
            if existing.is_none_or(|attributes| attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0) {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "a folder is needed here",
                ));
            }
            // Setting a DACL is pushed down to everything inside; skip it when nothing changes.
            if self.is_private(dir)? {
                return Ok(());
            }
            let security = Descriptor::private(true)?;
            let dacl = security.dacl()?;
            // SAFETY: `target` is NUL-terminated; `dacl` points into `security`, which outlives
            // the call.
            let result = unsafe {
                SetNamedSecurityInfoW(
                    PCWSTR(target.as_ptr()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(dacl.cast_const()),
                    None,
                )
            };
            status(result)
        }

        fn write_atomic(
            &self,
            path: &Path,
            bytes: &[u8],
            mode: WriteMode,
            expect: Expect,
        ) -> io::Result<WriteResult> {
            let target = wide(path)?;
            let existing = attributes_of(&target)?;
            if existing.is_some_and(|attributes| attributes & FILE_ATTRIBUTE_READONLY.0 != 0) {
                return Ok(WriteResult::ReadOnly);
            }
            if let Some(result) = check(&target, expect)? {
                return Ok(result);
            }
            let stage = wide(&stage_path(path)?)?;
            // What the stage is born with, and what is copied onto it before the rename.
            let (private, keep, attributes) = match (mode, existing) {
                (WriteMode::Private, _) => (
                    Some(Descriptor::private(false)?),
                    None,
                    FILE_ATTRIBUTE_NORMAL.0,
                ),
                (WriteMode::KeepTargetSecurity, Some(attributes)) => {
                    match security_to_keep(&target) {
                        Ok(keep) => (None, keep, kept_attributes(attributes)),
                        // Gone since the check above: the last check says what that means.
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            (None, None, FILE_ATTRIBUTE_NORMAL.0)
                        }
                        Err(error) => return Err(error),
                    }
                }
                // A new file inherits its folder's security.
                (WriteMode::KeepTargetSecurity, None) => (None, None, FILE_ATTRIBUTE_NORMAL.0),
            };
            let staging = Staging {
                bytes,
                born_with: private.as_ref().or(keep.as_ref()),
                keep: keep.as_ref(),
                attributes,
            };
            match stage_and_replace(&stage, &target, &staging, expect) {
                Ok(None) => Ok(WriteResult::Written),
                Ok(Some(result)) => {
                    delete_quietly(&stage);
                    Ok(result)
                }
                Err(error) => {
                    delete_quietly(&stage);
                    Err(error)
                }
            }
        }

        fn create_exclusive(&self, path: &Path, bytes: &[u8]) -> io::Result<bool> {
            let target = wide(path)?;
            if attributes_of(&target)?.is_some() {
                return Ok(false);
            }
            // Staged like a replace, so the name never shows a half-written file; the rename
            // refuses to replace, so exactly one writer wins.
            let stage = wide(&stage_path(path)?)?;
            let security = Descriptor::private(false)?;
            let result = (|| {
                let file = create_stage(&stage, Some(&security), FILE_ATTRIBUTE_NORMAL.0)?;
                write_all(&file, bytes)?;
                flush(&file)?;
                rename_onto(file, &stage, &target, false)
            })();
            match result {
                Ok(()) => Ok(true),
                Err(error) => {
                    delete_quietly(&stage);
                    if code_in(&error, already_exists) {
                        Ok(false)
                    } else {
                        Err(error)
                    }
                }
            }
        }

        fn identity(&self, path: &Path) -> io::Result<FileIdentity> {
            identity_of(&open_existing(&wide(path)?)?)
        }

        fn is_reparse(&self, path: &Path) -> io::Result<bool> {
            match attributes_of(&wide(path)?)? {
                Some(attributes) => Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0),
                None => Err(io::ErrorKind::NotFound.into()),
            }
        }

        fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
            let file = open_existing(&wide(path)?)?;
            // FILE_NAME_NORMALIZED is also VOLUME_NAME_DOS (both 0): a drive-letter path.
            let resolved = path_from(|buffer| {
                // SAFETY: the handle is open; the buffer is a valid slice for the call.
                unsafe { GetFinalPathNameByHandleW(file.0, buffer, FILE_NAME_NORMALIZED) }
            })?;
            Ok(match resolved.to_str() {
                Some(text) => PathBuf::from(strip_verbatim(text)),
                None => PathBuf::from(resolved),
            })
        }

        fn is_private(&self, path: &Path) -> io::Result<bool> {
            let security = Descriptor::of_path(&wide(path)?)?;
            let Some(allowed) = security.allowed_sids()? else {
                return Ok(false);
            };
            Ok(only_user_and_system(
                security.protected()?,
                &allowed,
                &user_sid()?,
            ))
        }

        fn short_path(&self, path: &Path) -> Option<PathBuf> {
            let long = wide(path).ok()?;
            path_from(|buffer| {
                // SAFETY: `long` is NUL-terminated; the buffer is a valid slice for the call.
                unsafe { GetShortPathNameW(PCWSTR(long.as_ptr()), Some(buffer)) }
            })
            .ok()
            .map(PathBuf::from)
        }

        fn long_path(&self, path: &Path) -> Option<PathBuf> {
            let short = wide(path).ok()?;
            path_from(|buffer| {
                // SAFETY: `short` is NUL-terminated; the buffer is a valid slice for the call.
                unsafe { GetLongPathNameW(PCWSTR(short.as_ptr()), Some(buffer)) }
            })
            .ok()
            .map(PathBuf::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_private_sddl_names_the_user_and_system_and_is_protected() {
        let sid = "S-1-5-21-1004336348-1177238915-682003330-1001";
        assert_eq!(
            private_sddl(sid, true),
            "D:P(A;OICI;FA;;;S-1-5-21-1004336348-1177238915-682003330-1001)(A;OICI;FA;;;SY)"
        );
        // A file's entries have nothing to hand down.
        assert_eq!(
            private_sddl(sid, false),
            "D:P(A;;FA;;;S-1-5-21-1004336348-1177238915-682003330-1001)(A;;FA;;;SY)"
        );
    }

    #[test]
    fn only_a_sid_goes_into_the_sddl() {
        assert!(is_sid("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(is_sid(SYSTEM_SID));
        for bad in [
            "",
            "S",
            "S-",
            "S-1",
            "S-1-",
            "s-1-5-18",
            "S-1-5-18)(A;;FA;;;WD",
            "S-1-5-x",
            "S-1--18",
            "WD",
        ] {
            assert!(!is_sid(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_refused_rename_is_retried_five_times_over_half_a_second() {
        let delays = retry_delays();
        assert_eq!(delays.len(), 5);
        assert_eq!(RENAME_RETRIES, 5);
        assert_eq!(delays.iter().sum::<Duration>(), Duration::from_millis(500));
        assert!(delays.windows(2).all(|pair| pair[0] < pair[1]));
        // A holder that lets go after 300 ms is outlasted before the last try.
        let before_last: Duration = delays[..4].iter().sum();
        assert!(before_last > Duration::from_millis(300));
    }

    #[test]
    fn only_sharing_access_and_lock_errors_are_retried() {
        for code in [5, 32, 33] {
            assert!(is_transient(code), "{code}");
        }
        // Not found, exists, disk full, invalid parameter, unsupported: never.
        for code in [0, 2, 3, 80, 183, 112, 87, 50, 1] {
            assert!(!is_transient(code), "{code}");
        }
    }

    #[test]
    fn error_codes_are_told_apart() {
        for code in [1, 50, 87] {
            assert!(rename_class_unsupported(code), "{code}");
        }
        for code in [5, 32, 33, 2, 183] {
            assert!(!rename_class_unsupported(code), "{code}");
        }
        assert!(no_acl_support(1) && no_acl_support(50));
        assert!(!no_acl_support(87) && !no_acl_support(5));
        assert!(already_exists(80) && already_exists(183));
        assert!(!already_exists(5) && !already_exists(2));
    }

    #[test]
    fn a_win32_code_is_read_out_of_its_hresult() {
        assert_eq!(win32_code(0x8007_0020_u32 as i32), Some(32));
        assert_eq!(win32_code(0x8007_0005_u32 as i32), Some(5));
        assert_eq!(win32_code(0x8007_00B7_u32 as i32), Some(183));
        // E_FAIL and success wrap no Win32 code.
        assert_eq!(win32_code(0x8000_4005_u32 as i32), None);
        assert_eq!(win32_code(0), None);
    }

    #[test]
    fn the_verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Users\me\.claude\settings.json"),
            r"C:\Users\me\.claude\settings.json"
        );
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\dir\settings.json"),
            r"\\server\share\dir\settings.json"
        );
        assert_eq!(strip_verbatim(r"\\?\unc\server\share"), r"\\server\share");
        // Nothing to strip.
        assert_eq!(strip_verbatim(r"C:\Users\me"), r"C:\Users\me");
        assert_eq!(strip_verbatim(r"\\server\share\x"), r"\\server\share\x");
        assert_eq!(strip_verbatim(""), "");
        // Short and non-ASCII rests are left whole.
        assert_eq!(strip_verbatim(r"\\?\C:"), "C:");
        assert_eq!(strip_verbatim(r"\\?\D:\Büro\é"), r"D:\Büro\é");
        assert_eq!(strip_verbatim(r"\\?\UNé\x"), r"UNé\x");
    }

    #[test]
    fn a_filetime_becomes_nanoseconds_since_1970() {
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_000), 0);
        // One tick is 100 ns.
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_001), 100);
        // 2026-09-30T00:00:00Z = 1790726400 s.
        assert_eq!(
            filetime_to_unix_ns(116_444_736_000_000_000 + 1_790_726_400 * 10_000_000),
            1_790_726_400_000_000_000
        );
        // Before 1970, and the zero FILETIME (1601).
        assert_eq!(filetime_to_unix_ns(116_444_735_990_000_000), -1_000_000_000);
        assert_eq!(filetime_to_unix_ns(0), -11_644_473_600_000_000_000);
        // The largest tick count doesn't overflow.
        assert!(filetime_to_unix_ns(i64::MAX) > 0);
    }

    #[test]
    fn a_replacement_keeps_the_attributes_that_can_be_kept() {
        // Archive only (what a plain file has).
        assert_eq!(kept_attributes(0x20), 0x20);
        // Hidden + system + archive + not content indexed.
        assert_eq!(kept_attributes(0x2 | 0x4 | 0x20 | 0x2000), 0x2026);
        // Normal, and attributes that can't be set (compressed, reparse point), are "normal".
        assert_eq!(kept_attributes(0x80), 0x80);
        assert_eq!(kept_attributes(0x800 | 0x400), 0x80);
        // Read-only is never carried over (such a target is refused before).
        assert_eq!(kept_attributes(0x1 | 0x2), 0x2);
    }

    #[test]
    fn private_means_protected_and_only_the_user_and_system() {
        let user = "S-1-5-21-1-2-3-1001";
        let sids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(only_user_and_system(true, &sids(&[user, "S-1-5-18"]), user));
        assert!(only_user_and_system(true, &sids(&[user]), user));
        // Nobody allowed at all is not readable by others either.
        assert!(only_user_and_system(true, &[], user));
        // Not protected: the folder above can hand down anything.
        assert!(!only_user_and_system(
            false,
            &sids(&[user, "S-1-5-18"]),
            user
        ));
        // Administrators, Users, Everyone, another user.
        for other in [
            "S-1-5-32-544",
            "S-1-5-32-545",
            "S-1-1-0",
            "S-1-5-21-1-2-3-1002",
        ] {
            assert!(
                !only_user_and_system(true, &sids(&[user, other]), user),
                "{other}"
            );
        }
    }
}
