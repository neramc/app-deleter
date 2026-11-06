//! 표시용 포맷터 (순수 함수).

/// 바이트 수를 사람이 읽기 쉬운 단위로 표시한다 (1 KB = 1024 B, Windows 탐색기와 동일).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// 레지스트리 `InstallDate`(`YYYYMMDD`)를 `YYYY-MM-DD`로. 형식이 다르면 `None`.
pub fn install_date(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.len() != 8 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, m, d) = (&s[0..4], &s[4..6], &s[6..8]);
    let (mi, di): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&mi) || !(1..=31).contains(&di) || y < "1990" {
        return None;
    }
    Some(format!("{y}-{m}-{d}"))
}

/// FILETIME 틱(1601-01-01 UTC 기준 100ns 단위)을 `YYYY-MM-DD`로.
pub fn filetime_to_date(ticks: i64) -> Option<String> {
    if ticks <= 0 {
        return None;
    }
    let days_since_1601 = ticks / 10_000_000 / 86_400;
    // 1601-01-01 → 1970-01-01 은 134,774일.
    let z = days_since_1601 - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (1990..=2200)
        .contains(&y)
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// 정렬 키로 쓰는 날짜 숫자(YYYYMMDD). 알 수 없으면 0.
pub fn date_sort_key(date: &str) -> u32 {
    date.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// 파일명에 쓸 수 없는 문자를 `_`로 바꾼다(백업 폴더 이름용).
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    let limited: String = trimmed.chars().take(60).collect();
    if limited.is_empty() {
        "app".to_string()
    } else {
        limited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KB");
        assert_eq!(bytes(1536), "1.50 KB");
        assert_eq!(bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(bytes(250 * 1024 * 1024 * 1024), "250 GB");
    }

    #[test]
    fn parses_install_date() {
        assert_eq!(install_date("20240315").as_deref(), Some("2024-03-15"));
        assert_eq!(install_date("2024315"), None);
        assert_eq!(install_date("20241315"), None);
        assert_eq!(install_date("abc"), None);
        assert_eq!(date_sort_key("2024-03-15"), 20240315);
        assert_eq!(date_sort_key(""), 0);
    }

    #[test]
    fn converts_filetime() {
        // 2024-03-15T00:00:00Z
        let unix = 1_710_460_800i64;
        let ticks = (unix + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_to_date(ticks).as_deref(), Some("2024-03-15"));
        assert_eq!(filetime_to_date(0), None);
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(
            sanitize_file_name(r#"A<B>:C"D/E\F|G?H*"#),
            "A_B__C_D_E_F_G_H_"
        );
        assert_eq!(sanitize_file_name("  ...  "), "app");
        assert_eq!(sanitize_file_name("카카오톡."), "카카오톡");
    }
}
