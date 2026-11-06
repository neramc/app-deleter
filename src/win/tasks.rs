//! 작업 스케줄러 작업 조회/삭제.
//!
//! `%SystemRoot%\System32\Tasks` 아래 XML 정의를 직접 읽어(관리자 권한) 실행 명령을 얻고,
//! 삭제는 `schtasks.exe /Delete`로 수행한다(스케줄러 서비스의 캐시까지 일관되게 정리됨).

use crate::win::{expand_env, fsops, run_hidden};

#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// `\Folder\Name` 형식 작업 경로.
    pub path: String,
    /// 작업 정의 파일(백업용).
    pub file: String,
    /// `Command + Arguments` (환경 변수 확장됨).
    pub actions: Vec<String>,
}

fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let words: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&words);
    }
    let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    // BOM 없는 UTF-16LE 추정: 두 번째 바이트가 0이면.
    if rest.len() >= 4 && rest[1] == 0 && rest[3] == 0 {
        let words: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&words);
    }
    String::from_utf8_lossy(rest).into_owned()
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find(&open) {
        let after = &rest[s + open.len()..];
        let Some(e) = after.find(&close) else { break };
        out.push(&after[..e]);
        rest = &after[e + close.len()..];
    }
    out
}

/// 모든 예약 작업과 실행 명령을 읽는다.
pub fn list(tasks_root: &str) -> Vec<TaskInfo> {
    fsops::find_files(tasks_root, &[""], 8)
        .into_iter()
        .filter_map(|file| {
            let bytes = std::fs::read(&file).ok()?;
            if bytes.len() > 1024 * 1024 {
                return None;
            }
            let xml = decode(&bytes);
            if !xml.contains("<Task") {
                return None;
            }
            let commands = elements(&xml, "Command");
            if commands.is_empty() {
                return None;
            }
            let args = elements(&xml, "Arguments");
            let actions = commands
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let a = args.get(i).map(|a| unescape(a)).unwrap_or_default();
                    expand_env(&format!("{} {a}", unescape(c).trim()))
                })
                .collect();
            let rel = file
                .get(tasks_root.trim_end_matches('\\').len()..)?
                .to_string();
            Some(TaskInfo {
                path: rel,
                file,
                actions,
            })
        })
        .collect()
}

pub fn delete(task_path: &str) -> Result<(), String> {
    run_hidden("schtasks.exe", &["/Delete", "/TN", task_path, "/F"])
        .map(|_| ())
        .map_err(|e| format!("예약 작업 삭제 실패: {e}"))
}
