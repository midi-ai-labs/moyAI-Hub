//! Management credentials are readable only by their local operating-system owner.
use std::{fs::File, io};

#[cfg(unix)]
pub fn protect(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(unix)]
pub fn verify(file: &File) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { geteuid() } || metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "management credential permissions are not private",
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{ffi::c_void, os::windows::io::AsRawHandle, ptr};

    type Handle = *mut c_void;
    #[repr(C)]
    struct AclSize {
        count: u32,
        used: u32,
        free: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: Handle) -> Handle;
        fn ReOpenFile(file: Handle, access: u32, share: u32, flags: u32) -> Handle;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(
            token: Handle,
            class: i32,
            buffer: Handle,
            size: u32,
            required: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: Handle, output: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            input: *const u16,
            revision: u32,
            output: *mut Handle,
            size: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorDacl(
            descriptor: Handle,
            present: *mut i32,
            acl: *mut Handle,
            defaulted: *mut i32,
        ) -> i32;
        fn SetSecurityInfo(
            object: Handle,
            kind: i32,
            information: u32,
            owner: Handle,
            group: Handle,
            dacl: Handle,
            sacl: Handle,
        ) -> u32;
        fn GetSecurityInfo(
            object: Handle,
            kind: i32,
            information: u32,
            owner: *mut Handle,
            group: *mut Handle,
            dacl: *mut Handle,
            sacl: *mut Handle,
            descriptor: *mut Handle,
        ) -> u32;
        fn GetAclInformation(acl: Handle, information: Handle, length: u32, class: i32) -> i32;
        fn GetAce(acl: Handle, index: u32, ace: *mut Handle) -> i32;
    }
    struct Local(Handle);
    impl Drop for Local {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    struct Token(Handle);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn sid_string(sid: Handle) -> io::Result<String> {
        let mut output = ptr::null_mut();
        if sid.is_null() || unsafe { ConvertSidToStringSidW(sid, &mut output) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _allocation = Local(output.cast());
        let mut length = 0;
        while length < 256 && unsafe { *output.add(length) } != 0 {
            length += 1;
        }
        if length == 256 {
            return Err(io::Error::other("invalid owner SID"));
        }
        String::from_utf16(unsafe { std::slice::from_raw_parts(output, length) })
            .map_err(|_| io::Error::other("invalid owner SID"))
    }
    fn current_user() -> io::Result<String> {
        let mut raw = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), 0x0008, &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(raw);
        let mut required = 0;
        unsafe {
            GetTokenInformation(token.0, 1, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 65536 {
            return Err(io::Error::other("cannot inspect process owner"));
        }
        let mut bytes = vec![0u8; required as usize];
        if unsafe {
            GetTokenInformation(
                token.0,
                1,
                bytes.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let sid = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<Handle>()) };
        sid_string(sid)
    }
    pub fn protect(file: &File) -> io::Result<()> {
        let security_handle = unsafe { ReOpenFile(file.as_raw_handle(), 0x0006_0000, 7, 0) };
        if security_handle == -1isize as Handle {
            return Err(io::Error::last_os_error());
        }
        let security_handle = Token(security_handle);
        let user = current_user()?;
        let sddl: Vec<u16> = format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)\0")
            .encode_utf16()
            .collect();
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _allocation = Local(descriptor);
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            == 0
            || present == 0
            || acl.is_null()
        {
            return Err(io::Error::other("cannot create private management ACL"));
        }
        let result = unsafe {
            SetSecurityInfo(
                security_handle.0,
                1,
                0x8000_0004,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        verify(file)
    }
    pub fn verify(file: &File) -> io::Result<()> {
        let user = current_user()?;
        let mut descriptor = ptr::null_mut();
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                1,
                0x0000_0005,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let _allocation = Local(descriptor);
        if acl.is_null() || sid_string(owner)? != user {
            return Err(io::Error::other(
                "management credential belongs to another owner",
            ));
        }
        let mut size = AclSize {
            count: 0,
            used: 0,
            free: 0,
        };
        if unsafe {
            GetAclInformation(
                acl,
                (&mut size as *mut AclSize).cast(),
                std::mem::size_of::<AclSize>() as u32,
                2,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut user_allowed = false;
        for index in 0..size.count {
            let mut ace = ptr::null_mut();
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // Accept only the exact explicit access-allowed ACEs we create.
            let bytes = ace.cast::<u8>();
            if unsafe { *bytes } != 0 || unsafe { *bytes.add(1) } != 0 {
                return Err(io::Error::other("management credential ACL is not private"));
            }
            let sid = sid_string(unsafe { bytes.add(8) }.cast())?;
            if sid != user && sid != "S-1-5-18" {
                return Err(io::Error::other(
                    "management credential is readable by another principal",
                ));
            }
            user_allowed |= sid == user;
        }
        if !user_allowed {
            return Err(io::Error::other(
                "management credential owner has no access",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows::{protect, verify};
