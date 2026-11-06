# App Deleter (앱 삭제)

**Windows 11 전용** 고성능 앱 완전 삭제 도구입니다. **Rust + [Slint](https://slint.dev)** 로 만든 단일 실행 파일(설치 불필요)이며,
앱의 제거 프로그램을 실행한 뒤 **남은 파일·폴더·레지스트리·바로가기·서비스·예약 작업·시작 프로그램·방화벽 규칙**까지
찾아 깨끗하게 정리합니다.

| 기능 | 설명 |
| --- | --- |
| 앱 목록 | 레지스트리 Uninstall 키(HKLM 64/32비트, HKCU) + **Microsoft Store(MSIX/AppX) 앱** |
| 실제 용량 | 설치 폴더를 직접 순회해 크기순 정렬(레지스트리 추정치가 아닌 실측). 계산 중에도 목록은 즉시 표시 |
| 제거 | 앱 고유 제거 마법사 실행 / **자동(무인) 제거**(MSI·Inno Setup·QuietUninstallString) / Store 앱은 Windows 패키지 관리자 |
| 강제 제거 | 제거 프로그램이 없거나 고장 난 앱도 파일·레지스트리를 직접 정리 |
| 잔여물 검색 | 설치 폴더, AppData(Roaming/Local/LocalLow), ProgramData, Program Files, 시작 메뉴·바탕 화면·작업 표시줄 바로가기, 레지스트리(Software·App Paths·Run·MSI 등록 정보), 서비스, 작업 스케줄러, 방화벽 규칙 |
| 안전장치 | 신뢰도(확실/높음/낮음) 표시 후 사용자가 선택, 시스템 폴더/키 보호 가드, 레지스트리 `.reg` 백업, 휴지통 이동, 시스템 복원 지점, 사용 중 파일은 재부팅 시 삭제 예약 |
| 디자인 | Windows 11 Fluent(DESIGN.md 토큰) · 라이트/다크 모드 · OS 강조색 자동 반영 · PerMonitorV2 고DPI |

## 사용 방법

1. [Releases](../../releases)에서 `AppDeleter.exe`를 받아 실행합니다(관리자 권한 요청 1회).
2. 목록에서 앱의 **제거** 버튼(또는 행 더블클릭)을 누릅니다. `Ctrl+F` 검색, `F5` 새로 고침.
3. 옵션을 고르고 **제거 시작** → 앱의 제거 마법사를 마치면 자동으로 **남은 항목 검색**으로 넘어갑니다.
4. 검색된 항목을 확인하고 **선택 항목 삭제**. 기본으로 `확실`·`높음` 항목만 선택되어 있습니다.

삭제한 레지스트리와 예약 작업은 `%LOCALAPPDATA%\AppDeleter\Backups\<날짜> <앱 이름>\`에
`registry.reg`(더블클릭으로 복원), `tasks\*.xml`, `log.txt`(작업 기록)로 남습니다.

## 고성능 설계

- **폴더 크기**: `FindFirstFileExW` + `FindExInfoBasic`(8.3 이름 생략) + `FIND_FIRST_EX_LARGE_FETCH`(배치 읽기)로
  파일마다 별도 stat 없이 크기를 얻고, 하위 폴더는 **rayon 작업 훔치기로 병렬** 순회합니다.
  같은 폴더를 가리키는 앱은 한 번만 계산하고, 큰 앱부터 시작해 코어 부하를 고르게 나눕니다.
- **UI 스트리밍**: 크기·아이콘 결과를 약 80ms 단위로 묶어 UI 스레드에 반영합니다(수백 개 앱에서도 끊김 없음).
- **삭제**: 하위 폴더 병렬 삭제, `\\?\` 긴 경로 지원, 읽기 전용 속성 자동 해제.
- **제거 완료 대기**: 제거 프로그램을 일시 정지 상태로 만들어 **Job Object**에 넣고, Job 안의 **모든 자식 프로세스**가
  끝날 때까지 기다립니다(임시 폴더로 자신을 복사해 다시 실행하는 Inno Setup/NSIS 제거 프로그램 대응).
  제거 후 설문 페이지용 브라우저만 남으면 완료로 판단합니다.
- 릴리스 빌드: `opt-level=3`, fat LTO, `codegen-units=1`, `panic=abort`, Skia 렌더러(소프트웨어 렌더러 폴백).

## 안전 설계

"완벽하게" 지우는 것만큼 **엉뚱한 것을 지우지 않는 것**이 중요합니다.

- **보호 가드**(`src/safety.rs`): `C:\Windows`, `Program Files`/`ProgramData`/`AppData` 루트, 사용자 프로필 폴더,
  `WindowsApps`, `Package Cache`, 공용 런타임(`dotnet`, `Windows Kits` 등)은 절대 삭제하지 않습니다.
  레지스트리는 `Software\<앱>` 계열과 정해진 위치(Uninstall·App Paths·MSI 등록 정보·Run·방화벽 규칙)만 허용합니다.
  모든 삭제 직전에 가드를 **다시** 확인합니다.
- **신뢰도**
  - `확실`: 설치 폴더(실행 파일이 그 안에 있음을 확인), 설치 폴더를 가리키는 바로가기·서비스·예약 작업·Run 값·방화벽 규칙
  - `높음`: 이름이 앱과 정확히 일치하는 데이터 폴더·레지스트리 키(게시자 폴더는 이 앱 데이터만 있을 때만)
  - `낮음`: 부분 일치, 문서 폴더 — **기본 선택하지 않음**
- **다른 앱 보호**: 아직 설치된 다른 앱의 설치 폴더·실행 파일을 포함하거나, 다른 앱 이름과 일치하는 폴더/키는 제외합니다.
- **정션/심볼릭 링크를 따라가지 않음**: 링크 너머의 데이터를 계산하거나 지우지 않습니다.
- 정리 중에는 창을 닫을 수 없게 막아 작업이 중간에 끊기지 않습니다.

## 빌드

Windows 11 + [Rust](https://rustup.rs) 1.88 이상(MSVC 툴체인)이 필요합니다.

```bash
cargo build --release
```

결과: `target\release\app-deleter.exe` (아이콘·버전 정보·관리자 권한 매니페스트 포함 단일 파일)

```bash
cargo test                       # 순수 로직 단위 테스트(모든 OS)
cargo test -- --include-ignored  # + Windows 통합/종단 간 테스트(HKCU·임시 폴더에 가짜 앱을 만들었다 지움)
```

순수 로직(경로·이름 매칭·명령줄·`.reg` 포맷·안전 가드)은 Linux/macOS에서도 컴파일·테스트되며,
Win32/WinRT와 UI는 Windows에서만 빌드됩니다.

## 구조

```
ui/                 Slint UI (Fluent 토큰 theme.slint, 메인 창 app.slint, 제거 마법사 wizard.slint)
src/app.rs          UI 컨트롤러(백그라운드 스캔, 마법사 흐름)
src/inventory.rs    설치된 앱 수집(레지스트리 + Store), 설치 폴더 검증
src/leftovers.rs    잔여물 검색 엔진(신뢰도 판정)
src/cleaner.rs      삭제 실행 + 백업 + 기록
src/safety.rs       삭제 보호 가드
src/textmatch.rs    앱 이름 ↔ 폴더/키 이름 매칭
src/installdir.rs   설치 폴더 추정
src/win/            Win32/WinRT 계층(파일 시스템, 레지스트리, Job Object, 아이콘, 서비스, 예약 작업, 복원 지점, Store)
```

## 릴리스 (GitHub Actions)

`.github/workflows/build.yml`이 모든 푸시에서 Windows 러너로 포맷·Clippy·테스트(통합 테스트 포함)와 릴리스 빌드를
실행하고, `v*` 태그를 푸시하면 `AppDeleter.exe`(+ SHA-256)를 GitHub Release에 올립니다.

```bash
git tag v2.0.0
```

```bash
git push origin v2.0.0
```

서명되지 않은 실행 파일은 첫 실행 시 SmartScreen 경고가 뜰 수 있습니다. 코드 서명 인증서가 있다면 저장소 Secrets에
`WINDOWS_CERT_BASE64`(PFX의 base64)와 `WINDOWS_CERT_PASSWORD`를 등록하면 자동으로 서명합니다.

## 참고 / 한계

- Windows 11(빌드 22000 이상) 전용입니다. 그 이하 버전에서는 안내 후 종료합니다.
- 창 배경 Mica 재질은 Slint 렌더러가 투명 창 합성을 지원하지 않아 Fluent 솔리드 배경색(`SolidBackgroundFillColorBase`)으로 대체했습니다.
- 아이콘 글꼴은 Windows 11에 기본 설치된 *Segoe Fluent Icons*를 사용하며 앱에 동봉하지 않습니다.
- 관리자 계정이 아닌 표준 사용자가 다른 관리자 자격 증명으로 실행하면 `HKCU`·`AppData`는 그 관리자 계정 기준으로 검색됩니다.
- UI는 Slint Royalty-free 라이선스로 사용하며, 앱의 정보 화면에 Slint 표기를 포함합니다.

## 라이선스

[Apache-2.0](LICENSE)
