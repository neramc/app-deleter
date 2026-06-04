# 앱 삭제 (app-deleter)

Windows에서 설치된 앱을 **실제 전체 데이터 용량 기준으로 정렬**해 보여주고,
버튼 한 번으로 **시스템 언인스톨러를 실행**하는 데스크톱 애플리케이션입니다.

> `.exe` 실행 파일 크기만이 아니라 **설치 폴더 전체를 재귀 순회**하여 실제 점유 용량을 계산합니다.

## 기술 스택

- **Tauri v2** (Rust 백엔드)
- **React 18 + TypeScript + Vite** (프론트엔드)
- 색상: 메인 배경 `#FDFFFC`, 버튼/강조 `#41EAD4`

## 주요 기능

- HKLM/HKCU 레지스트리의 Uninstall 항목을 열거(32/64비트 모두)하고 시스템 컴포넌트·업데이트 항목을 필터링
- **설치 폴더 + AppData 데이터까지 합산**하여 앱의 실제 점유 용량 계산:
  - 설치 폴더(`InstallLocation`)
  - `%APPDATA%`(Roaming) · `%LOCALAPPDATA%`(Local) · `LocalLow` · `%ProgramData%` 내
    해당 앱의 데이터 폴더(이름 매칭). 서로 포함 관계인 경로는 중복 합산하지 않음
- **고성능 폴더 스캐너**(`src-tauri/src/fastsize.rs`): Win32 `FindFirstFileExW` +
  `FindExInfoBasic`(8.3 단축명 생략) + `FIND_FIRST_EX_LARGE_FETCH`(배치 읽기)로
  추가 stat 호출 없이 크기를 취득하고, 최상위 하위 폴더를 `rayon`으로 병렬 스캔
- 용량은 백그라운드에서 계산되며 `app-size-updated` 이벤트로 스트리밍 → 즉시 목록 표시 후 점진적 갱신/재정렬
- **용량 큰 순 기본 정렬**, 이름/게시자 검색, 새로고침
- 삭제 버튼 → 확인 후 `UninstallString`을 파싱해 OS 언인스톨러 실행
- **권한은 한 번만**: 앱은 일반 권한으로 실행(시작 시 UAC 없음)되며, **최초 삭제 시도
  시에만** 관리자 권한으로 1회 재실행한다(`elevation.rs`). 이후 같은 세션의 삭제는
  추가 프롬프트 없이 진행된다. 목록/용량 스캔은 일반 권한으로 동작.

## 커스텀 설치 마법사 (`installer/`)

NSIS 등 외부 번들러에 **의존하지 않는** 자체 제작 설치 마법사입니다. 메인 앱과
동일하게 **Tauri v2 + React 플랫 디자인**(`#FDFFFC` / `#41EAD4`)으로 만들었으며,
메인 앱 실행 파일을 컴파일 타임에 **임베드**한 단독 `.exe`입니다.

설치 마법사가 하는 일(관리자 권한 불필요, 현재 사용자 계정에 설치):

1. `%LOCALAPPDATA%\AppDeleter`(ASCII 경로)에 `app-deleter.exe` + `uninstall.exe` 복사
2. 시작 메뉴 · 바탕화면 바로가기(.lnk) 생성
3. 제어판 "프로그램 제거" 목록에 제거 정보 등록 (`UninstallString` → `uninstall.exe`)

플랫 디자인 UI(기능 소개 · 단계 체크리스트 · 진행률 · 완료 후 실행)로 구성됩니다.

## 커스텀 제거 마법사 (`uninstaller/`)

설치 마법사가 함께 설치하는 별도의 제거 마법사(`uninstall.exe`)입니다. 제어판
"프로그램 제거" 또는 직접 실행 시 동작하며, 레지스트리 항목·바로가기를 지우고,
자기 자신이 종료된 뒤 설치 폴더 전체를 삭제하도록 예약합니다(실행 중인 exe
자기 삭제 문제 회피).

> 파일/폴더·바로가기 이름은 모두 ASCII(`App Deleter`, `app-deleter.exe`)이며,
> 한글은 창 제목 등 표시용에만 사용합니다.

## 개발

```bash
# 메인 앱
npm install
npm run tauri dev      # 개발 모드 (Windows에서 실행)
npm run tauri build -- --no-bundle   # 단독 exe 생성(번들 없이)

# 설치 마법사 (메인 exe를 installer/src-tauri/payload/app-deleter.exe 로 먼저 복사)
cd installer && npm install && npm run tauri build -- --no-bundle
```

> Tauri 앱의 실제 동작은 Windows에서만 완전합니다. 비-Windows에서는
> 백엔드 커맨드가 빈 목록/에러를 반환하는 스텁으로 컴파일만 가능합니다.

## 릴리스 (자동 빌드)

GitHub Actions(`.github/workflows/release.yml`)가 `windows-latest`에서
**메인 앱(`AppDeleter-<tag>.exe`)** 과 **설치 마법사(`AppDeleter-Setup-<tag>.exe`)** 를
빌드해 **GitHub Release에 자동 업로드**합니다. 두 가지 방식이 있습니다:

- **수동 실행(권장, `workflow_dispatch`)** — Actions → release → Run workflow.
  빌드가 시작되면 최신 `v*` 태그에서 다음 버전(vX.Y.Z)을 계산하고, 그 버전을
  **모든 매니페스트(`package.json`/`Cargo.toml`/`tauri.conf.json`)에 동기화**
  (`scripts/set-version.mjs`)하여 `main`에 커밋·태그·푸시한 뒤 그 태그로 릴리스합니다.
  증가 단위는 입력값 `bump`(`patch`/`minor`/`major`, 기본 `patch`)로 선택.
- **태그 푸시(`v*`)** — 직접 푸시한 태그로 그대로 릴리스합니다(버전 동기화·커밋 없음).

> 자동 태그/커밋 푸시는 `GITHUB_TOKEN`으로 이루어지므로 워크플로가 재귀적으로 다시
> 트리거되지 않습니다(무한 루프 없음).

### 코드 서명 / SmartScreen

서명되지 않은 실행 파일은 첫 실행 시 **Windows SmartScreen 경고**가 뜹니다(이는 게시자
평판/서명이 없어서이며, "추가 정보 → 실행"으로 진행 가능). 이를 없애려면 **코드 서명
인증서**가 필요합니다(무료로는 불가). 인증서를 마련했다면 저장소 Secrets에
`WINDOWS_CERT_BASE64`(PFX의 base64), `WINDOWS_CERT_PASSWORD`를 등록하세요. 워크플로가
자동으로 `signtool`로 산출물을 서명합니다.

### 잠금 파일

`Cargo.lock` / `package-lock.json`은 혼동 방지를 위해 **저장소에 올리지 않으며**(.gitignore),
CI는 `npm install`로 매번 의존성을 해석합니다.
