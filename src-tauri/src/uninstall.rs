/// 앱의 UninstallString을 실행하여 시스템 언인스톨러를 띄운다.
///
/// 언인스톨러 자체 UI는 OS/설치 프로그램에 위임하므로, 프로세스 종료를
/// 기다리지 않고 spawn 성공 여부만 반환한다.
#[tauri::command]
pub fn uninstall_app(uninstall_string: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::run(&uninstall_string)
    }
    #[cfg(not(windows))]
    {
        let _ = uninstall_string;
        Err("언인스톨은 Windows에서만 지원됩니다.".into())
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::process::Command;

    pub fn run(uninstall_string: &str) -> Result<(), String> {
        let trimmed = uninstall_string.trim();
        if trimmed.is_empty() {
            return Err("언인스톨 정보가 없습니다.".into());
        }

        let (program, args) = parse_command_line(trimmed);
        Command::new(&program)
            .args(&args)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("언인스톨러 실행 실패 ({program}): {e}"))
    }

    /// Windows 명령줄 문자열을 (실행 파일, 인자 목록)으로 분해한다.
    ///
    /// 따옴표로 묶인 토큰을 인식하여 공백이 포함된 경로를 올바르게 처리한다.
    /// 예) `"C:\Program Files\App\unins000.exe" /SILENT` 또는
    ///     `MsiExec.exe /X{GUID}`
    fn parse_command_line(input: &str) -> (String, Vec<String>) {
        let mut tokens: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;

        for ch in input.chars() {
            match ch {
                '"' => in_quotes = !in_quotes,
                c if c.is_whitespace() && !in_quotes => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        let mut iter = tokens.into_iter();
        let program = iter.next().unwrap_or_default();
        let args = iter.collect();
        (program, args)
    }

    #[cfg(test)]
    mod tests {
        use super::parse_command_line;

        #[test]
        fn parses_quoted_path_with_args() {
            let (prog, args) =
                parse_command_line("\"C:\\Program Files\\App\\unins000.exe\" /SILENT /x");
            assert_eq!(prog, "C:\\Program Files\\App\\unins000.exe");
            assert_eq!(args, vec!["/SILENT", "/x"]);
        }

        #[test]
        fn parses_msiexec() {
            let (prog, args) = parse_command_line("MsiExec.exe /X{12345-ABCDE}");
            assert_eq!(prog, "MsiExec.exe");
            assert_eq!(args, vec!["/X{12345-ABCDE}"]);
        }
    }
}
