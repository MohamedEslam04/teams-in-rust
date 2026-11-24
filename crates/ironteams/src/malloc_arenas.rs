//! glibc's malloc gives every thread that allocates its own arena (up to eight per core), and
//! memory freed in an arena mostly stays with the process. IronTeams' threads come and go
//! (the blocking pool decoding pictures, tokio, the call and notification threads), and each left
//! an arena behind: measured on 0.1.9, 35 arenas held 152 MB, and the same session with
//! `MALLOC_ARENA_MAX=2` used 31 MB less (190 MB of anonymous memory against 159 MB).
//!
//! [`limit`] caps the arenas at [`ARENAS`] before any thread starts. A `MALLOC_ARENA_MAX` set by
//! the user wins: glibc has already applied it, and it is left alone.

/// Arenas glibc may use: one for the main thread and one shared by all the others.
pub const ARENAS: i32 = 2;

/// Whether the cap is applied: not when the user set `MALLOC_ARENA_MAX` themselves.
pub fn applies(user_setting: Option<&std::ffi::OsStr>) -> bool {
    user_setting.is_none()
}

/// Caps glibc's malloc arenas (Linux with glibc only; elsewhere nothing happens). Called first
/// thing in `main`, before any other thread exists. True when the cap was applied.
pub fn limit() -> bool {
    if !applies(std::env::var_os("MALLOC_ARENA_MAX").as_deref()) {
        return false;
    }
    set_arena_max(ARENAS)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn set_arena_max(arenas: i32) -> bool {
    use std::ffi::c_int;
    // <malloc.h>: `#define M_ARENA_MAX -8`.
    const M_ARENA_MAX: c_int = -8;
    unsafe extern "C" {
        fn mallopt(param: c_int, value: c_int) -> c_int;
    }
    // SAFETY: mallopt only changes malloc's settings; M_ARENA_MAX takes any positive count.
    unsafe { mallopt(M_ARENA_MAX, arenas) == 1 }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn set_arena_max(_arenas: i32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_users_own_setting_is_left_alone() {
        assert!(applies(None));
        assert!(!applies(Some(std::ffi::OsStr::new("4"))));
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_takes_the_arena_cap() {
        assert!(set_arena_max(ARENAS));
    }
}
