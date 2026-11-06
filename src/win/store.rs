//! Microsoft Store(MSIX/AppX) 앱 조회·제거 (WinRT `PackageManager`).

use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

use crate::format;
use crate::win::icons;

#[derive(Debug, Clone)]
pub struct StoreApp {
    pub full_name: String,
    pub family_name: String,
    pub display_name: String,
    pub publisher: String,
    pub version: String,
    pub install_path: String,
    pub install_date: String,
    pub logo: Option<String>,
}

fn read(p: &Package) -> Option<StoreApp> {
    // 프레임워크·리소스·번들·시스템 구성 요소는 사용자가 제거할 앱이 아니다.
    if p.IsFramework().unwrap_or(true)
        || p.IsResourcePackage().unwrap_or(true)
        || p.IsBundle().unwrap_or(false)
        || p.IsStub().unwrap_or(false)
    {
        return None;
    }
    if matches!(p.SignatureKind(), Ok(k) if k == PackageSignatureKind::System) {
        return None;
    }
    let id = p.Id().ok()?;
    let full_name = id.FullName().ok()?.to_string();
    let family_name = id.FamilyName().ok()?.to_string();
    let display_name = p
        .DisplayName()
        .ok()
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty() && !s.starts_with("ms-resource:"))
        .or_else(|| id.Name().ok().map(|s| s.to_string()))?;
    let publisher = p
        .PublisherDisplayName()
        .ok()
        .map(|s| s.to_string())
        .filter(|s| !s.starts_with("ms-resource:"))
        .unwrap_or_default();
    let version = id
        .Version()
        .map(|v| format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision))
        .unwrap_or_default();
    let install_path = p.InstalledPath().map(|s| s.to_string()).unwrap_or_default();
    let install_date = p
        .InstalledDate()
        .ok()
        .and_then(|d| format::filetime_to_date(d.UniversalTime))
        .unwrap_or_default();
    let logo = if install_path.is_empty() {
        None
    } else {
        icons::resolve_store_logo(&install_path)
    };
    Some(StoreApp {
        full_name,
        family_name,
        display_name,
        publisher,
        version,
        install_path,
        install_date,
        logo,
    })
}

/// 현재 사용자에게 설치된 Store 앱 목록.
pub fn list() -> Vec<StoreApp> {
    let Ok(pm) = PackageManager::new() else {
        return Vec::new();
    };
    let Ok(packages) = pm.FindPackagesByUserSecurityId(&HSTRING::new()) else {
        return Vec::new();
    };
    packages.into_iter().filter_map(|p| read(&p)).collect()
}

/// 현재 사용자에게서 패키지를 제거하고 완료될 때까지 기다린다.
pub fn remove(full_name: &str) -> Result<(), String> {
    let pm = PackageManager::new().map_err(|e| e.message())?;
    let op = pm
        .RemovePackageAsync(&HSTRING::from(full_name))
        .map_err(|e| format!("제거 요청 실패: {}", e.message()))?;
    let result = op
        .join()
        .map_err(|e| format!("Store 앱 제거 실패: {}", e.message()))?;
    match result.ExtendedErrorCode() {
        Ok(code) if code.is_err() => {
            let text = result
                .ErrorText()
                .map(|t| t.to_string())
                .unwrap_or_default();
            Err(format!("Store 앱 제거 실패: {text}"))
        }
        _ => Ok(()),
    }
}

/// 패키지 패밀리가 아직 설치되어 있는지.
pub fn is_installed(family_name: &str) -> bool {
    let Ok(pm) = PackageManager::new() else {
        return false;
    };
    pm.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family_name))
        .map(|it| it.into_iter().next().is_some())
        .unwrap_or(false)
}
