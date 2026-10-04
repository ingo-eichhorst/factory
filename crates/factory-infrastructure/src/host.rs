//! The facts L1 Infrastructure shows about the machine the daemon runs on.
//!
//! Read live on every request and never cached: they are a handful of
//! `sysctl`/`libc` calls, cheaper than keeping a copy honest. No subprocess
//! per field -- `sw_vers`, `uname` and friends are all answered by a sysctl
//! directly.
//!
//! Every fact is its own fallible read, and a failure is `None` -- never an
//! error, never a panic. A host that will not say what chip it has is still
//! a page worth drawing, and nothing in here may take the daemon down with
//! it: each `unsafe` block checks its return code before it trusts a byte.
//!
//! ## The platform seam
//!
//! The same shape as `crate::power`: macOS is the one platform with a real
//! body, behind `#[cfg(target_os = "macos")]`. Everywhere else gets what the
//! standard library and POSIX answer alike -- the architecture, the core
//! count, the hostname, the load average and the root filesystem -- and
//! `None` for the rest.

use serde::{Deserialize, Serialize};

/// The machine the daemon runs on, read live on every request and never
/// cached. Every field is its own fallible read: one that fails is `null` on
/// the wire -- deliberately no `skip_serializing_if` anywhere here, so the
/// page can tell "unreadable" from a field this daemon never heard of.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostFacts {
    pub hostname: Option<String>,
    /// The hardware model identifier, e.g. `Mac17,7`.
    pub model: Option<String>,
    /// The CPU brand string, e.g. `Apple M5 Max`.
    pub chip: Option<String>,
    /// Physical cores.
    pub cores: Option<u32>,
    pub memory_bytes: Option<u64>,
    /// Product name and version, e.g. `macOS 26.6.1`.
    pub os: Option<String>,
    /// What this daemon was built for, e.g. `aarch64`.
    pub arch: Option<String>,
    /// Since boot, wall clock -- time asleep included.
    pub uptime_seconds: Option<u64>,
    /// The 1, 5 and 15 minute load averages.
    pub load: Option<[f64; 3]>,
    /// The filesystem mounted at `/`, or `null` when it could not be read.
    pub disk: Option<DiskFacts>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskFacts {
    pub mount: String,
    pub total_bytes: u64,
    /// Available to an unprivileged user, as `df` reports it.
    pub free_bytes: u64,
}

/// Everything this platform will say about the host, right now.
pub fn collect() -> HostFacts {
    HostFacts {
        hostname: hostname(),
        model: platform::model(),
        chip: platform::chip(),
        cores: platform::cores().or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .and_then(|n| u32::try_from(n.get()).ok())
        }),
        memory_bytes: platform::memory_bytes(),
        os: platform::os(),
        arch: Some(std::env::consts::ARCH.to_string()),
        uptime_seconds: platform::uptime_seconds(),
        load: load(),
        disk: disk("/"),
    }
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; gethostname writes at
    // most that many and we never read past the first NUL below.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    c_string(&buf)
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
}

#[cfg(unix)]
fn load() -> Option<[f64; 3]> {
    let mut averages = [0f64; 3];
    // SAFETY: `averages` holds exactly the three samples asked for.
    let got = unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };
    (got == 3 && averages.iter().all(|v| v.is_finite())).then_some(averages)
}

#[cfg(not(unix))]
fn load() -> Option<[f64; 3]> {
    None
}

#[cfg(unix)]
pub fn disk(mount: &str) -> Option<DiskFacts> {
    let path = std::ffi::CString::new(mount).ok()?;
    // SAFETY: a zeroed statvfs is a valid out-parameter, `path` is a valid
    // NUL-terminated string, and the struct is only read when the call
    // reports success.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // The field widths differ by platform; widen them all the same way.
    #[allow(clippy::unnecessary_cast)]
    let (fragment, blocks, available) = (
        stat.f_frsize as u64,
        stat.f_blocks as u64,
        stat.f_bavail as u64,
    );
    Some(DiskFacts {
        mount: mount.to_string(),
        total_bytes: blocks.checked_mul(fragment)?,
        free_bytes: available.checked_mul(fragment)?,
    })
}

#[cfg(not(unix))]
pub fn disk(_mount: &str) -> Option<DiskFacts> {
    None
}

/// The text before the first NUL, if there is any.
#[cfg(unix)]
fn c_string(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::CString;

    /// A string sysctl: asked once for its size, then read into a buffer of
    /// exactly that size.
    pub(super) fn string(name: &str) -> Option<String> {
        let key = CString::new(name).ok()?;
        let mut len: libc::size_t = 0;
        // SAFETY: a null out-buffer with a valid length pointer asks only
        // for the size.
        let rc = unsafe {
            libc::sysctlbyname(
                key.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || len == 0 || len > 4096 {
            return None;
        }
        let mut buf = vec![0u8; len];
        // SAFETY: `buf` is valid for `len` bytes, and the call writes at
        // most `len`, updating it to the count actually written.
        let rc = unsafe {
            libc::sysctlbyname(
                key.as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return None;
        }
        buf.truncate(len.min(buf.len()));
        super::c_string(&buf)
    }

    /// A fixed-size sysctl value, read only when the kernel hands back
    /// exactly `size_of::<T>()` bytes -- a size mismatch means this is not
    /// the value we think it is, and is treated as unreadable.
    fn value<T: Copy>(name: &str) -> Option<T> {
        let key = CString::new(name).ok()?;
        let mut out = std::mem::MaybeUninit::<T>::zeroed();
        let mut len: libc::size_t = std::mem::size_of::<T>();
        // SAFETY: `out` is valid for `size_of::<T>()` bytes and `len` says
        // so; it is only assumed initialised when the call succeeded and
        // filled all of it.
        let rc = unsafe {
            libc::sysctlbyname(
                key.as_ptr(),
                out.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && len == std::mem::size_of::<T>()).then(|| unsafe { out.assume_init() })
    }

    pub fn model() -> Option<String> {
        string("hw.model")
    }

    pub fn chip() -> Option<String> {
        string("machdep.cpu.brand_string")
    }

    pub fn cores() -> Option<u32> {
        value::<i32>("hw.physicalcpu")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    }

    pub fn memory_bytes() -> Option<u64> {
        value::<u64>("hw.memsize").filter(|n| *n > 0)
    }

    pub fn os() -> Option<String> {
        string("kern.osproductversion").map(|version| format!("macOS {version}"))
    }

    /// Wall clock since boot, from `kern.boottime` -- not a monotonic clock,
    /// which stops while the machine sleeps and would undercount a laptop.
    pub fn uptime_seconds() -> Option<u64> {
        let boot = value::<libc::timeval>("kern.boottime")?;
        let booted = u64::try_from(boot.tv_sec).ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        now.checked_sub(booted)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn model() -> Option<String> {
        None
    }
    pub fn chip() -> Option<String> {
        None
    }
    pub fn cores() -> Option<u32> {
        None
    }
    pub fn memory_bytes() -> Option<u64> {
        None
    }
    pub fn os() -> Option<String> {
        None
    }
    pub fn uptime_seconds() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a check of any particular machine -- CI and a laptop differ --
    /// only that collecting never panics and that what every platform can
    /// answer, it does.
    #[test]
    fn collecting_host_facts_never_fails_and_answers_the_portable_ones() {
        let facts = collect();
        assert_eq!(facts.arch.as_deref(), Some(std::env::consts::ARCH));
        assert!(facts.cores.is_some_and(|n| n > 0), "{facts:?}");
        #[cfg(unix)]
        {
            let disk = facts
                .disk
                .as_ref()
                .expect("the root filesystem is readable");
            assert_eq!(disk.mount, "/");
            assert!(
                disk.total_bytes > 0 && disk.free_bytes <= disk.total_bytes,
                "{disk:?}"
            );
            assert!(facts.hostname.is_some(), "{facts:?}");
            assert!(facts.load.is_some(), "{facts:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_answers_every_fact() {
        let facts = collect();
        assert!(facts.model.is_some(), "{facts:?}");
        assert!(facts.chip.is_some(), "{facts:?}");
        assert!(facts.memory_bytes.is_some_and(|m| m > 0), "{facts:?}");
        assert!(
            facts.os.as_deref().is_some_and(|o| o.starts_with("macOS ")),
            "{facts:?}"
        );
        assert!(facts.uptime_seconds.is_some(), "{facts:?}");
    }

    #[test]
    fn a_missing_mount_is_none_not_an_error() {
        assert!(disk("/no/such/mount/anywhere").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unknown_sysctl_is_none_not_an_error() {
        assert!(platform::string("factory.no.such.key").is_none());
    }
}
