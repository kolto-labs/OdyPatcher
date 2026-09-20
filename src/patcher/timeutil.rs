//! Local wall-clock time, for log stamps and backup folder names.
//!
//! Python formats `datetime.now(tz=timezone.utc).astimezone()`, i.e. local
//! time. The standard library only knows UTC, so the platform's C routine is
//! asked for the local broken-down time.

/// Local date and time: `(year, month, day, hour, minute, second, microsecond)`.
pub fn local_datetime() -> (i32, u32, u32, u32, u32, u32, u32) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let micros = now.subsec_micros();
    let (year, month, day, hour, minute, second) = platform::local_parts(now.as_secs() as i64);
    (year, month, day, hour, minute, second, micros)
}

/// Local `(hour, minute, second)`.
pub fn local_time_of_day() -> (u32, u32, u32) {
    let (_, _, _, hour, minute, second, _) = local_datetime();
    (hour, minute, second)
}

/// `strftime("%Y-%m-%d_%H.%M.%S")` in local time.
pub fn backup_timestamp() -> String {
    let (y, mo, d, h, mi, s, _) = local_datetime();
    format!("{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}")
}

/// `strftime("%Y-%m-%d_%H.%M.%S.%f")` in local time.
pub fn backup_timestamp_micros() -> String {
    let (y, mo, d, h, mi, s, us) = local_datetime();
    format!("{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}.{us:06}")
}

/// Convert days since the Unix epoch into a calendar date (UTC fallback).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((if m <= 2 { y + 1 } else { y }) as i32, m as u32, d as u32)
}

#[allow(dead_code)]
fn utc_parts(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(unix)]
mod platform {
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }

    extern "C" {
        fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
    }

    pub fn local_parts(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
        let mut tm = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        // SAFETY: `localtime_r` writes into the caller-provided struct and
        // reads only the time value; both pointers are valid for the call.
        let ok = unsafe { !localtime_r(&secs, &mut tm).is_null() };
        if !ok {
            return super::utc_parts(secs);
        }
        (
            tm.tm_year + 1900,
            (tm.tm_mon + 1) as u32,
            tm.tm_mday as u32,
            tm.tm_hour as u32,
            tm.tm_min as u32,
            tm.tm_sec as u32,
        )
    }
}

#[cfg(windows)]
mod platform {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }

    pub fn local_parts(_secs: i64) -> (i32, u32, u32, u32, u32, u32) {
        let mut st = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: the struct is a valid, writable SYSTEMTIME.
        unsafe { GetLocalTime(&mut st) };
        (
            st.year as i32,
            st.month as u32,
            st.day as u32,
            st.hour as u32,
            st.minute as u32,
            st.second as u32,
        )
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    pub fn local_parts(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
        super::utc_parts(secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_have_the_python_shape() {
        let stamp = backup_timestamp();
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert_eq!(&stamp[10..11], "_");
        assert_eq!(backup_timestamp_micros().len(), 26);
    }

    #[test]
    fn civil_conversion_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
    }
}
