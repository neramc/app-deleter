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
- 설치 폴더(`InstallLocation`)를 `walkdir` + `rayon`으로 병렬 재귀 순회하여 **실제 전체 용량** 계산
- 용량은 백그라운드에서 계산되며 `app-size-updated` 이벤트로 스트리밍 → 즉시 목록 표시 후 점진적 갱신/재정렬
- **용량 큰 순 기본 정렬**, 이름/게시자 검색, 새로고침
- 삭제 버튼 → 확인 후 `UninstallString`을 파싱해 OS 언인스톨러 실행
- 앱 시작 시 관리자 권한(UAC) 상승 요청 (레지스트리/폴더 스캔 안정화)

## 개발

```bash
npm install
npm run tauri dev      # 개발 모드 (Windows에서 실행)
npm run tauri build    # 로컬 번들(NSIS .exe) 생성
```

> Tauri 앱의 실제 동작은 Windows에서만 완전합니다. 비-Windows에서는
> 백엔드 커맨드가 빈 목록/에러를 반환하는 스텁으로 컴파일만 가능합니다.

## 릴리스 (자동 빌드)

GitHub Actions(`.github/workflows/release.yml`)가 `windows-latest`에서 빌드하여
NSIS 산출물을 **GitHub Release에 자동 업로드**합니다. 두 가지 방식이 있습니다:

- **수동 실행(권장, `workflow_dispatch`)** — Actions → release → Run workflow.
  빌드가 시작되면 최신 `v*` 태그에서 **다음 버전 태그(vX.Y.Z)를 자동으로 계산·푸시**하고
  그 태그로 릴리스합니다. 증가 단위는 입력값 `bump`(`patch`/`minor`/`major`, 기본 `patch`)로 선택.
  기존 태그가 없으면 `src-tauri/tauri.conf.json`의 버전을 시작점으로 사용합니다.
- **태그 푸시(`v*`)** — 직접 푸시한 태그로 그대로 릴리스합니다(추가 증가 없음).

  ```bash
  git tag v0.1.0
  git push origin v0.1.0
  ```

> 자동 태그 푸시는 `GITHUB_TOKEN`으로 이루어지므로 워크플로가 재귀적으로 다시
> 트리거되지 않습니다(무한 루프 없음).

## 아이콘

`app-icon.png`(1024×1024)가 소스 아이콘입니다. 아이콘 세트를 다시 생성하려면:

```bash
npx tauri icon app-icon.png
```
