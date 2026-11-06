//! 알려진 폴더(Known Folders)와 삭제 가드 구성.

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads,
    FOLDERID_LocalAppData, FOLDERID_LocalAppDataLow, FOLDERID_Profile, FOLDERID_ProgramData,
    FOLDERID_ProgramFilesCommonX64, FOLDERID_ProgramFilesCommonX86, FOLDERID_ProgramFilesX64,
    FOLDERID_ProgramFilesX86, FOLDERID_Programs, FOLDERID_Public, FOLDERID_PublicDesktop,
    FOLDERID_PublicDocuments, FOLDERID_RoamingAppData, FOLDERID_UserProfiles,
    FOLDERID_UserProgramFiles, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

use crate::pathutil::join;
use crate::safety::PathGuard;

/// 알려진 폴더 하나의 경로를 얻는다.
fn known(id: &GUID) -> Option<String> {
    // SAFETY: 반환된 PWSTR은 CoTaskMemFree로 해제해야 한다.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// 잔여물 검색/삭제에 쓰는 시스템 경로 모음.
#[derive(Debug, Clone, Default)]
pub struct Folders {
    pub windows: String,
    pub profile: String,
    pub users: String,
    pub roaming: String,
    pub local: String,
    pub local_low: String,
    pub local_programs: String,
    pub program_data: String,
    pub program_files: String,
    pub program_files_x86: String,
    pub common_files: String,
    pub common_files_x86: String,
    pub documents: String,
    pub downloads: String,
    pub desktop: String,
    pub public: String,
    pub public_desktop: String,
    pub public_documents: String,
    pub programs: String,
    pub common_programs: String,
    pub quick_launch: String,
    pub packages: String,
    pub tasks: String,
}

impl Folders {
    pub fn load() -> Self {
        let get = |id: &GUID| known(id).unwrap_or_default();
        let windows = known(&FOLDERID_Windows).unwrap_or_else(|| r"C:\Windows".into());
        let local = get(&FOLDERID_LocalAppData);
        let roaming = get(&FOLDERID_RoamingAppData);
        let local_programs = known(&FOLDERID_UserProgramFiles).unwrap_or_else(|| {
            if local.is_empty() {
                String::new()
            } else {
                join(&local, "Programs")
            }
        });
        Self {
            tasks: join(&windows, r"System32\Tasks"),
            profile: get(&FOLDERID_Profile),
            users: get(&FOLDERID_UserProfiles),
            local_low: get(&FOLDERID_LocalAppDataLow),
            program_data: get(&FOLDERID_ProgramData),
            program_files: get(&FOLDERID_ProgramFilesX64),
            program_files_x86: get(&FOLDERID_ProgramFilesX86),
            common_files: get(&FOLDERID_ProgramFilesCommonX64),
            common_files_x86: get(&FOLDERID_ProgramFilesCommonX86),
            documents: get(&FOLDERID_Documents),
            downloads: get(&FOLDERID_Downloads),
            desktop: get(&FOLDERID_Desktop),
            public: get(&FOLDERID_Public),
            public_desktop: get(&FOLDERID_PublicDesktop),
            public_documents: get(&FOLDERID_PublicDocuments),
            programs: get(&FOLDERID_Programs),
            common_programs: get(&FOLDERID_CommonPrograms),
            quick_launch: if roaming.is_empty() {
                String::new()
            } else {
                join(&roaming, r"Microsoft\Internet Explorer\Quick Launch")
            },
            packages: if local.is_empty() {
                String::new()
            } else {
                join(&local, "Packages")
            },
            local_programs,
            windows,
            local,
            roaming,
        }
    }

    /// 설치 폴더 추정에 쓰는 "설치 루트"들(이 바로 아래가 보통 앱 폴더).
    pub fn install_roots(&self) -> Vec<String> {
        [
            &self.program_files,
            &self.program_files_x86,
            &self.common_files,
            &self.common_files_x86,
            &self.local_programs,
            &self.local,
            &self.roaming,
            &self.program_data,
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect()
    }

    /// 삭제 가드를 구성한다. 시스템·사용자 핵심 폴더는 절대 지우지 않는다.
    pub fn guard(&self) -> PathGuard {
        let mut g = PathGuard::new();
        let all = [
            &self.windows,
            &self.profile,
            &self.users,
            &self.roaming,
            &self.local,
            &self.local_low,
            &self.local_programs,
            &self.program_data,
            &self.program_files,
            &self.program_files_x86,
            &self.common_files,
            &self.common_files_x86,
            &self.documents,
            &self.downloads,
            &self.desktop,
            &self.public,
            &self.public_desktop,
            &self.public_documents,
            &self.programs,
            &self.common_programs,
            &self.quick_launch,
            &self.packages,
        ];
        for p in all {
            g.protect(p);
        }
        for sub in [
            "Temp",
            "Microsoft",
            r"Microsoft\Windows",
            "Programs",
            "Packages",
            "VirtualStore",
            "CrashDumps",
            "D3DSCache",
            "ConnectedDevicesPlatform",
            "Comms",
            "PlaceholderTileLogoFolder",
            "Publishers",
        ] {
            if !self.local.is_empty() {
                g.protect(&join(&self.local, sub));
            }
        }
        for sub in [
            "Microsoft",
            r"Microsoft\Windows",
            r"Microsoft\Internet Explorer",
        ] {
            if !self.roaming.is_empty() {
                g.protect(&join(&self.roaming, sub));
            }
        }
        for profile_child in [
            "Pictures",
            "Music",
            "Videos",
            "OneDrive",
            "Favorites",
            "Contacts",
            "Links",
            "Saved Games",
            "Searches",
            "3D Objects",
        ] {
            if !self.profile.is_empty() {
                g.protect(&join(&self.profile, profile_child));
            }
        }
        if !self.users.is_empty() {
            g.protect(&join(&self.users, "Default"));
            g.deny_tree(&join(&self.users, "Default"));
        }

        // 하위 트리 전체 금지.
        g.deny_tree(&self.windows);
        let drive = self.windows.get(..3).unwrap_or(r"C:\").to_string();
        for d in [
            "Windows.old",
            "$Recycle.Bin",
            "System Volume Information",
            "Recovery",
            "Boot",
            "$WinREAgent",
            "PerfLogs",
        ] {
            g.deny_tree(&join(&drive, d));
        }
        if !self.program_data.is_empty() {
            for d in [
                "Microsoft",
                "Package Cache",
                "Packages",
                "regid.1991-06.com.microsoft",
                "USOShared",
                "USOPrivate",
                "ssh",
            ] {
                g.deny_tree(&join(&self.program_data, d));
            }
        }
        for pf in [&self.program_files, &self.program_files_x86] {
            if pf.is_empty() {
                continue;
            }
            for d in [
                "WindowsApps",
                "ModifiableWindowsApps",
                "Windows Defender",
                "Windows Defender Advanced Threat Protection",
                "Windows Mail",
                "Windows Media Player",
                "Windows NT",
                "Windows Photo Viewer",
                "Windows Portable Devices",
                "Windows Security",
                "Windows Sidebar",
                "WindowsPowerShell",
                "Internet Explorer",
                "Common Files\\microsoft shared",
                "Common Files\\System",
                "Common Files\\Services",
                "Common Files\\SpeechEngines",
                "Reference Assemblies",
                "MSBuild",
                "dotnet",
                "Microsoft SDKs",
                "Windows Kits",
                "Microsoft Update Health Tools",
                "Uninstall Information",
            ] {
                g.deny_tree(&join(pf, d));
            }
        }
        if !self.local.is_empty() {
            for d in [
                r"Microsoft\Windows",
                r"Microsoft\WindowsApps",
                r"Microsoft\Credentials",
                r"Microsoft\Vault",
                r"Microsoft\Edge",
                r"Microsoft\EdgeWebView",
                "Temp",
            ] {
                g.deny_tree(&join(&self.local, d));
            }
        }
        if !self.roaming.is_empty() {
            for d in [
                r"Microsoft\Windows",
                r"Microsoft\Credentials",
                r"Microsoft\Crypto",
                r"Microsoft\Protect",
                r"Microsoft\SystemCertificates",
                r"Microsoft\Vault",
            ] {
                g.deny_tree(&join(&self.roaming, d));
            }
        }
        // 시작 메뉴·빠른 실행 바로가기는 거부 트리 안에 있지만 정리 대상이다.
        for allow in [&self.programs, &self.common_programs, &self.quick_launch] {
            if !allow.is_empty() {
                g.allow_tree(allow);
            }
        }
        // 시작 프로그램 폴더 자체는 보존.
        for p in [&self.programs, &self.common_programs] {
            if !p.is_empty() {
                g.protect(&join(p, "Startup"));
                g.protect(&join(p, "Administrative Tools"));
                g.protect(&join(p, "Accessibility"));
                g.protect(&join(p, "Accessories"));
                g.protect(&join(p, "Maintenance"));
                g.protect(&join(p, "System Tools"));
                g.protect(&join(p, "Windows PowerShell"));
                g.protect(&join(p, "Windows Tools"));
            }
        }
        if !self.quick_launch.is_empty() {
            g.protect(&join(&self.quick_launch, "User Pinned"));
            g.protect(&join(&self.quick_launch, r"User Pinned\TaskBar"));
            g.protect(&join(&self.quick_launch, r"User Pinned\StartMenu"));
            g.protect(&join(
                &self.quick_launch,
                r"User Pinned\ImplicitAppShortcuts",
            ));
        }
        g
    }
}
