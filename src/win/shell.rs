//! 바로가기(.lnk) 대상 해석.

use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink};
use windows::core::{Interface, PCWSTR};

use crate::win::{expand_env, from_wide, wide};

/// 한 스레드에서 재사용하는 바로가기 해석기. (호출 스레드에 COM이 초기화되어 있어야 한다.)
pub struct ShortcutResolver {
    link: IShellLinkW,
    file: IPersistFile,
}

impl ShortcutResolver {
    pub fn new() -> Option<Self> {
        // SAFETY: 표준 셸 COM 객체 생성.
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let file: IPersistFile = link.cast().ok()?;
        Some(Self { link, file })
    }

    /// 바로가기의 대상 경로와 인자. 대상이 없는(고급 MSI) 바로가기는 빈 문자열일 수 있다.
    pub fn target(&self, lnk_path: &str) -> Option<(String, String)> {
        let w = wide(lnk_path);
        // SAFETY: NUL 종료 경로, 출력 버퍼 길이는 API가 슬라이스에서 얻는다.
        unsafe {
            self.file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
            let mut buf = vec![0u16; 32768];
            let mut fd = WIN32_FIND_DATAW::default();
            let path = if self
                .link
                .GetPath(&mut buf, &mut fd, SLGP_RAWPATH.0 as u32)
                .is_ok()
            {
                expand_env(&from_wide(&buf))
            } else {
                String::new()
            };
            let mut args = vec![0u16; 4096];
            let args = if self.link.GetArguments(&mut args).is_ok() {
                from_wide(&args)
            } else {
                String::new()
            };
            Some((path, args))
        }
    }
}
