#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        credential_type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            credential_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, credential_type: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    fn target(reference: &str) -> String {
        format!("InsituTranslate/{reference}")
    }

    pub fn write(reference: &str, secret: &str) -> Result<(), String> {
        let mut target_name = wide(&target(reference));
        let mut user_name = wide("InsituTranslate");
        let mut blob: Vec<u8> = secret
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let credential = CredentialW {
            flags: 0,
            credential_type: CRED_TYPE_GENERIC,
            target_name: target_name.as_mut_ptr(),
            comment: null_mut(),
            last_written: [0, 0],
            credential_blob_size: blob.len() as u32,
            credential_blob: blob.as_mut_ptr(),
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: null_mut(),
            target_alias: null_mut(),
            user_name: user_name.as_mut_ptr(),
        };
        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok == 0 {
            return Err(format!(
                "Windows Credential Manager write failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn read(reference: &str) -> Result<Option<String>, String> {
        let target_name = wide(&target(reference));
        let mut credential: *mut CredentialW = null_mut();
        let ok = unsafe { CredReadW(target_name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1168) {
                return Ok(None);
            }
            return Err(format!("Windows Credential Manager read failed: {error}"));
        }
        if credential.is_null() {
            return Ok(None);
        }
        let value = unsafe {
            let item = &*credential;
            let bytes = std::slice::from_raw_parts(
                item.credential_blob,
                item.credential_blob_size as usize,
            );
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            let decoded = String::from_utf16(&units)
                .map_err(|error| format!("Credential is not valid UTF-16: {error}"))?;
            CredFree(credential.cast());
            decoded
        };
        Ok(Some(value))
    }

    pub fn delete(reference: &str) -> Result<(), String> {
        let target_name = wide(&target(reference));
        let ok = unsafe { CredDeleteW(target_name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(1168) {
                return Err(format!("Windows Credential Manager delete failed: {error}"));
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    pub fn write(_reference: &str, _secret: &str) -> Result<(), String> {
        Err("System credential storage is currently implemented for Windows only".into())
    }
    pub fn read(_reference: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn delete(_reference: &str) -> Result<(), String> {
        Ok(())
    }
}

pub fn write(reference: &str, secret: &str) -> Result<(), String> {
    platform::write(reference, secret)
}

pub fn read(reference: &str) -> Result<Option<String>, String> {
    platform::read(reference)
}

pub fn delete(reference: &str) -> Result<(), String> {
    platform::delete(reference)
}

pub fn mask(_secret: &str) -> String {
    "••••••••••••".into()
}
