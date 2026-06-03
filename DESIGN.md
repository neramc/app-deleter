# DESIGN.md — Windows 11 / WinUI 3 (Fluent Design) 구현 가이드

> **이 문서의 독자: Claude Code**
> 어떤 앱 플랫폼(WinUI 3 · Avalonia · Tauri 등)에서든 **Windows 11의 WinUI 3 / Fluent Design 룩앤필**을 정확히 재현하기 위한 단일 기준 문서다.
> 구현 우선순위는 **WinUI 3(네이티브 기준) → Avalonia(.NET 크로스플랫폼) → Tauri(웹 기반)** 순서다.

---

## 0. 핵심 작업 원칙 (먼저 읽을 것)

1. **토큰 우선(Token-first).** 색·타이포·간격·모서리·재질·아이콘·모션을 먼저 "디자인 토큰"으로 정의하고, 플랫폼별로 그 토큰을 매핑한다. UI 코드에 raw hex/px를 흩뿌리지 말 것.
2. **소스 오브 트루스 참조, 추측 금지.** 정확한 색 값(특히 alpha가 섞인 fill 색)은 각 플랫폼의 테마 리소스(WinUI 테마 딕셔너리, Avalonia `FluentTheme`)에서 가져온다. 기억에 의존해 hex를 지어내지 말 것. 본 문서가 제시하는 hex 중 "참조값"이라고 표시된 것은 근사치이며, 정확한 값은 반드시 테마 딕셔너리로 검증한다.
3. **Light / Dark 양쪽 지원.** Fluent의 모든 색·재질 토큰은 mode-aware다. 한쪽만 구현하지 않는다.
4. **Accent는 OS에서.** 강조색은 하드코딩하지 말고 시스템 강조색을 읽어 쓰는 것을 기본으로 한다(폴백만 정의).
5. **플랫폼 한계를 명시.** Mica/Acrylic의 진짜 OS 합성은 Windows에서만 가능하다. 비-Windows나 웹에서는 근사치임을 코드 주석/문서에 남긴다.

---

## 1. Fluent Design 핵심 시각 언어

Fluent Design System은 **Light · Depth · Motion · Material · Scale** 다섯 축으로 구성된다. Windows 11에서는 과거의 reveal/light 강조 효과는 축소되고, **재질(Material)과 모션(Motion)**으로 상호작용을 표현하는 방향으로 진화했다.

Windows 11 화면을 "WinUI 3답게" 만드는 식별 특징:

- **둥근 모서리**: 창·카드·flyout은 8px, 일반 컨트롤(버튼·입력창 등)은 4px.
- **Mica 배경**: 창 전체 배경이 데스크톱 배경색으로 은은하게 tint되고, 창 포커스(active/inactive)에 따라 톤이 바뀐다.
- **계층적 깊이감(layering)**: 솔리드 표면 위에 카드/레이어가 살짝 떠 있는 느낌. 강한 그림자보다 미묘한 elevation.
- **명확한 타이포 위계**: Segoe UI Variable 기반의 type ramp.
- **시스템 강조색(accent)**: 선택·강조·기본 버튼 등에 일관 사용.
- **부드러운 모션**: 빠른 hover 피드백 + 부드러운 진입 애니메이션.

---

## 2. 디자인 토큰 (Source of Truth)

### 2.1 색상 토큰

Fluent의 색은 "역할(role)" 기반 계층으로 나뉜다. 아래 표의 **WinUI 브러시 키**가 정식 이름이며, 모든 플랫폼은 이 역할 구조를 따라야 한다.

| 토큰 역할 | WinUI 브러시 키(예) | 용도 |
|---|---|---|
| Accent | `AccentFillColorDefaultBrush`, `…Secondary`, `…Tertiary`, `…Disabled` | 기본 버튼, 토글 on, 선택 상태, 링크 |
| Accent 텍스트 | `AccentTextFillColorPrimaryBrush` 등 | 강조색 위/강조 텍스트 |
| 텍스트 | `TextFillColorPrimaryBrush`, `…Secondary`, `…Tertiary`, `…Disabled` | 본문 / 보조 / 비활성 텍스트 |
| 컨트롤 채움 | `ControlFillColorDefaultBrush`, `…Secondary`, `…Tertiary`, `…Disabled` | 버튼·입력창 등 컨트롤 배경 |
| 미묘한 채움 | `SubtleFillColorTransparentBrush`, `…Secondary`, `…Tertiary` | hover/pressed 시 은은한 배경(예: 리스트 아이템) |
| 카드 | `CardBackgroundFillColorDefaultBrush`, `…Secondary` | 카드/패널 표면 |
| 레이어 | `LayerFillColorDefaultBrush`, `…Alt` | 컨텐츠 위 떠 있는 레이어 |
| 솔리드 배경 | `SolidBackgroundFillColorBaseBrush`, `…Secondary`, `…Tertiary`, `…Quarternary` | 페이지/영역 기본 배경 |
| 스트로크 | `ControlStrokeColorDefaultBrush`, `CardStrokeColorDefaultBrush`, `DividerStrokeColorDefaultBrush` | 테두리·구분선 |
| 시스템 의미색 | `SystemFillColorSuccessBrush`, `…Caution`, `…Critical`, `…Attention` | 성공/경고/오류/주의 |

**Accent:**
- 기본은 **OS 시스템 강조색**을 읽어 쓴다. 폴백 참조값은 Microsoft 블루 계열(약 `#0078D4`). 정확한 셰이드는 OS/테마에서 가져온다.

**Light / Dark 기준 배경(참조값 — 정확한 값은 테마 딕셔너리로 검증):**

| | Light | Dark |
|---|---|---|
| 앱/페이지 기본 배경 (`SolidBackgroundFillColorBase`) | ≈ `#F3F3F3` | ≈ `#202020` |
| 본문 텍스트 (`TextFillColorPrimary`) | 거의 검정(약 89% 불투명) | 흰색 |

> ⚠️ 카드/컨트롤/subtle fill 색들은 대부분 **alpha가 섞인 ARGB**다(예: 흰색 5~70% 위에 깔리는 식). 이 값들은 추측하지 말고 WinUI 테마 딕셔너리(`generic.xaml`) 또는 WinUI Gallery에서 정확한 `#AARRGGBB`를 확인해 사용한다.

### 2.2 타이포그래피

- **기본 폰트: Segoe UI Variable** (Windows 11 시스템 폰트). 가변 폰트로 weight 축(`wght`, 100~700)과 optical size 축(`opsz`, 자동)을 가진다.
- **weight 매핑**: Light 300 · Semilight 350 · Regular 400 · Semibold 600 · Bold 700.
- **규칙**: 본문은 Regular, 제목은 Semibold. **최소 크기는 14px Semibold / 12px Regular**(이보다 작으면 일부 언어에서 가독성 붕괴). 케이싱은 **Sentence case**(제목 포함). 기본 좌측 정렬, 중앙 정렬은 아이콘 아래 텍스트 같은 드문 경우만.
- **폴백 체인**: `Segoe UI Variable` → `Segoe UI` → 플랫폼 시스템 폰트.

**Type ramp** (effective px, `크기 / 행간`):

| 스타일 이름 | 크기 / 행간 | weight | WinUI 스타일 키 |
|---|---|---|---|
| Caption | 12 / 16 | Regular | `CaptionTextBlockStyle` |
| Body | 14 / 20 | Regular | `BodyTextBlockStyle` |
| Body Strong | 14 / 20 | Semibold | `BodyStrongTextBlockStyle` |
| Body Large | 18 / 24 | Regular | (Fluent 램프) |
| Subtitle | 20 / 28 | Semibold | `SubtitleTextBlockStyle` |
| Title | 28 / 36 | Semibold | `TitleTextBlockStyle` |
| Title Large | 40 / 52 | Semibold | `TitleLargeTextBlockStyle` |
| Display | 68 / 92 | Semibold | `DisplayTextBlockStyle` |

> Caption(12/16), Body(14/20), Body Strong(14/20 Semibold), Body Large(18/24)는 MS Learn 타이포 문서로 확인된 값이다. Subtitle 이상은 WinUI 표준 type ramp 값이며, 정확 검증이 필요하면 WinUI Gallery의 Typography 페이지를 본다.

### 2.3 간격 & 레이아웃

- **4px 그리드**를 기준으로 모든 간격을 4의 배수로 설계(4 · 8 · 12 · 16 · 20 · 24 · 32 · 40).
- 권장 패턴:
  - 컨트롤 내부 패딩: 가로 11~12px, 세로 5~6px 수준(버튼 기준).
  - 관련 요소 간 간격: 8px, 영역 간: 16~24px.
  - 페이지 콘텐츠 좌우 여백: 보통 16~24px.
  - 카드 내부 패딩: 16px 안팎.

### 2.4 모서리 (Corner Radius)

| 토큰 | 값 | 적용 대상 |
|---|---|---|
| `ControlCornerRadius` | **4px** | 버튼, 입력창, 체크박스 컨테이너, 작은 컨트롤 |
| `OverlayCornerRadius` | **8px** | 창, 카드, flyout, 메뉴, 다이얼로그, 팝업 |

### 2.5 깊이 / 그림자 (Elevation)

- Fluent의 깊이는 **강한 drop shadow가 아니라 미묘한 elevation + ThemeShadow**로 표현한다.
- transient 표면(flyout/메뉴/툴팁)은 본문 위로 떠 보이도록 부드러운 그림자(작은 key shadow + 넓고 옅은 ambient shadow)를 쓴다.
- 카드는 그림자보다 **테두리(stroke) + 약간 밝은 표면색**으로 분리하는 경우가 많다.

### 2.6 재질 (Material) — Mica / Acrylic / Smoke

| 재질 | 성격 | 쓰임 | mode-aware |
|---|---|---|---|
| **Mica** | 불투명. 데스크톱 배경색으로 은은히 tint. 창 active/inactive로 톤 변화 | **창 전체 배경** | O |
| **Acrylic** | 반투명 "frosted glass". Win11에서 더 밝고 투명 | **transient·light-dismiss 표면**(flyout, 컨텍스트 메뉴, 팝오버) | O |
| **Smoke** | 그 아래를 어둡게 dim | 모달(다이얼로그) 뒤 배경 차단 | X (항상 translucent black) |

> 원칙: **창 배경 = Mica**, **잠깐 떴다 사라지는 표면 = Acrylic**, **모달 뒤 = Smoke**. Acrylic을 창 전체에 남발하지 않는다.

### 2.7 아이콘

- **Segoe Fluent Icons** (Windows 11 기본 아이콘 폰트, WinUI의 `FontIcon` 기본). 레거시는 Segoe MDL2 Assets.
- ⚠️ **라이선스 주의**: Segoe Fluent Icons 폰트는 Windows 11에 기본 설치되어 있으나, **다른 플랫폼으로 동봉/배포 불가**다. 따라서 **크로스플랫폼/웹에서는 Fluent UI System Icons(SVG, MIT 라이선스)**를 사용한다.

### 2.8 모션

- **빠른 피드백**: pointer-over/pressed 같은 즉각 상태 변화는 짧게(대략 100~150ms), 감속(ease-out) 계열.
- **진입/전환**: 페이지·표면 진입은 조금 더 길게(대략 200~300ms) 부드럽게.
- 과한 바운스/오버슈트는 지양. Fluent는 "절제된 자연스러움"을 지향한다.

---

## 3. 플랫폼별 구현

### 3.1 WinUI 3 — 기준 구현 (C# / XAML, Windows App SDK)

WinUI 3는 Windows App SDK로 제공되는 Microsoft의 네이티브 UI 프레임워크다. 둥근 모서리·type ramp·테마 브러시가 **기본 제공**되므로, 우리가 할 일은 "직접 그리는 것"이 아니라 **올바른 리소스 키를 쓰는 것**이다.

**테마 브러시는 `{ThemeResource ...}`로 참조(절대 hex 하드코딩 금지):**
```xml
<Grid Background="{ThemeResource SolidBackgroundFillColorBaseBrush}">
    <Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}"
            BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}"
            BorderThickness="1"
            CornerRadius="{StaticResource OverlayCornerRadius}"
            Padding="16">

        <StackPanel Spacing="8">
            <TextBlock Text="설정" Style="{StaticResource TitleTextBlockStyle}" />
            <TextBlock Text="본문 텍스트입니다."
                       Style="{StaticResource BodyTextBlockStyle}"
                       Foreground="{ThemeResource TextFillColorSecondaryBrush}" />

            <Button Content="저장"
                    Style="{StaticResource AccentButtonStyle}"
                    CornerRadius="{StaticResource ControlCornerRadius}" />

            <FontIcon Glyph="&#xE74E;" />  <!-- Segoe Fluent Icons (Save) -->
        </StackPanel>
    </Border>
</Grid>
```

**창 배경에 Mica 적용:**
```csharp
// MainWindow.xaml.cs (Windows App SDK)
this.SystemBackdrop = new MicaBackdrop();
// transient 표면이라면: new DesktopAcrylicBackdrop();
```

**체크리스트(WinUI 3):**
- 색은 `ThemeResource` 브러시 키로만. type은 `…TextBlockStyle`로.
- 강조 버튼은 `AccentButtonStyle`.
- 창 배경 `MicaBackdrop`, flyout/메뉴는 Acrylic 계열.
- Light/Dark는 시스템 테마 자동 추종(필요 시 `RequestedTheme`로 강제).

### 3.2 Avalonia — .NET 크로스플랫폼 (XAML / C#)

Avalonia에는 Fluent에서 영감받은 **`FluentTheme`**가 내장되어 있다(NuGet `Avalonia.Themes.Fluent`, 11.x/12.x 계열). 더 WinUI에 가깝게 가려면 **FluentAvalonia(amwx)**로 WinUI 컨트롤·리소스를 보강한다.

**기본 FluentTheme 적용 (App.axaml):**
```xml
<Application xmlns="https://github.com/avaloniaui"
             xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <Application.Styles>
        <FluentTheme DensityStyle="Compact" />
    </Application.Styles>
</Application>
```

**Light/Dark 팔레트·강조색 override:**
```xml
<FluentTheme>
    <FluentTheme.Palettes>
        <ColorPaletteResources x:Key="Light" Accent="#0078D4" RegionColor="#F3F3F3" />
        <ColorPaletteResources x:Key="Dark"  Accent="#4CC2FF" RegionColor="#202020" />
    </FluentTheme.Palettes>
</FluentTheme>
```
- `Accent`를 override하지 않으면 Avalonia가 **OS 강조색**을 사용한다(가능한 경우). 런타임 변경은 `Accent`만 가능.
- Avalonia는 Windows에서 시스템 테마/강조색을 `PlatformSettings`로 감지하고, Win11에서는 네이티브 타이틀바를 `RequestedThemeVariant`에 맞춰 갱신한다.

**더 WinUI스럽게 — FluentAvalonia:**
- `FluentAvalonia`(amwx)는 WinUI 컨트롤(`NavigationView`, `InfoBar`, `TabView` 등)과 Fluent 리소스를 가져온다. WinUI 3 룩을 목표로 한다면 권장.

**Mica(Windows 11):**
- `Window`의 `TransparencyLevelHint`에 `Mica`(또는 `AcrylicBlur`)를 주거나, FluentAvalonia/창 합성 기법으로 Win11 Mica를 흉내낸다. 비-Windows에서는 솔리드 배경으로 폴백.

**주의:** Segoe UI Variable은 Windows에만 존재한다. macOS/Linux에서는 시스템 폰트로 폴백되도록 FontFamily 폴백 체인을 명시한다.

### 3.3 Tauri — 웹 기반 (HTML/CSS/JS + Rust)

Tauri는 프론트가 **웹 기술(HTML/CSS/JS, React 등)**이고 창은 OS WebView다. 두 갈래로 접근한다:
1. **창 재질**: Rust 쪽에서 `window-vibrancy` 플러그인으로 OS 합성 재질 적용.
2. **UI 토큰**: CSS 변수로 Fluent 토큰을 재현.

**(1) 창 재질 — `window-vibrancy` (Rust, Windows):**
```rust
use window_vibrancy::{apply_mica, apply_acrylic, apply_blur};

#[cfg(target_os = "windows")]
{
    let v = windows_version::OsVersion::current();
    if v.major > 10 || (v.major == 10 && v.build >= 22000) {
        apply_mica(&window, None).expect("Mica 적용 실패");   // Windows 11
    } else {
        apply_acrylic(&window, None).ok();                    // Windows 10 폴백
    }
}
```
- Mica가 비치려면 창/웹 루트 배경을 (반)투명하게 둬야 한다.
- WebView2 최신 버전에서는 Fluent 스타일 오버레이 스크롤바도 지원한다.

**(2) UI 토큰 — CSS 변수(근사 Fluent, light/dark):**
```css
:root {
  --accent: #0078D4;                 /* 가능하면 OS 강조색을 JS로 주입 */
  --bg-base: #f3f3f3;
  --card-bg: rgba(255,255,255,0.70);
  --card-stroke: rgba(0,0,0,0.06);
  --text-primary: rgba(0,0,0,0.89);
  --text-secondary: rgba(0,0,0,0.60);

  --radius-control: 4px;
  --radius-overlay: 8px;

  --font: "Segoe UI Variable", "Segoe UI", system-ui, sans-serif;
  --font-body: 14px/20px var(--font);
  --font-title: 600 28px/36px var(--font);
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg-base: #202020;
    --card-bg: rgba(255,255,255,0.05);
    --card-stroke: rgba(255,255,255,0.08);
    --text-primary: #ffffff;
    --text-secondary: rgba(255,255,255,0.78);
  }
}

/* Acrylic 흉내(웹 근사치): transient 표면에만 */
.flyout {
  background: var(--card-bg);
  backdrop-filter: blur(30px) saturate(125%);
  border: 1px solid var(--card-stroke);
  border-radius: var(--radius-overlay);
}
```

- 더 정식으로 가려면 **Fluent UI Web Components(`@fluentui/web-components`)** 또는 Fluent 토큰 패키지를 사용한다.
- 아이콘은 **Fluent UI System Icons(SVG, MIT)**.
- ⚠️ `backdrop-filter` blur는 진짜 Mica/Acrylic의 OS 합성과 다른 **근사치**다. 진짜 재질이 필요하면 `window-vibrancy` 경로를 함께 쓴다.

### 3.4 토큰 → 플랫폼 매핑 한눈에 보기

| 토큰 | WinUI 3 | Avalonia | Tauri (웹) |
|---|---|---|---|
| 색(역할) | `{ThemeResource …Brush}` | `FluentTheme` 리소스 / `ColorPaletteResources` | CSS 변수(`--…`) |
| 강조색 | `AccentFillColorDefaultBrush`(OS) | `Accent`(미지정 시 OS) | OS 값 JS 주입 → `--accent` |
| 타이포 | `…TextBlockStyle` | FluentTheme 텍스트 스타일 | `--font-*` (Segoe UI Variable 폴백) |
| 모서리 | `ControlCornerRadius`/`OverlayCornerRadius` | 동일 키(Fluent 리소스) | `--radius-control`/`--radius-overlay` |
| 창 재질 | `MicaBackdrop`/`DesktopAcrylicBackdrop` | `TransparencyLevelHint=Mica` / FluentAvalonia | `window-vibrancy`(`apply_mica`/`apply_acrylic`) |
| 아이콘 | Segoe Fluent Icons (`FontIcon`) | Segoe Fluent Icons(Win) / SVG 폴백 | Fluent UI System Icons (SVG) |

---

## 4. Claude Code 구현 체크리스트

생성/수정한 UI가 아래를 모두 만족하는지 확인한다.

- [ ] 색을 raw hex로 박지 않고 **역할 토큰/테마 리소스**로 참조했다.
- [ ] **Light/Dark 둘 다** 정상 동작한다.
- [ ] 강조색은 **OS 시스템 강조색** 기반(폴백만 정의).
- [ ] 폰트는 **Segoe UI Variable**(+폴백 체인), 최소 **14px Semibold / 12px Regular** 준수, **Sentence case**.
- [ ] type ramp 스타일을 일관되게 사용(제목 Semibold).
- [ ] 모서리: 일반 컨트롤 **4px**, 창/카드/flyout **8px**.
- [ ] 재질: **창 = Mica**, **transient = Acrylic**, **모달 뒤 = Smoke**. Acrylic 남용 금지.
- [ ] 간격은 **4px 그리드**.
- [ ] 아이콘 라이선스: 비-Windows/웹에서는 Segoe Fluent Icons를 동봉하지 말고 **Fluent UI System Icons(SVG)** 사용.
- [ ] 접근성: 텍스트 대비 충분, **포커스 비주얼**, 키보드 내비게이션, 터치 타깃 크기.
- [ ] 플랫폼 한계를 코드 주석/문서에 명시(예: "Mica는 Windows 11 전용, 그 외엔 솔리드 폴백").

---

## 5. 소스 오브 트루스 (참고 링크)

정확한 값/구현이 필요할 때 본 문서 대신 아래 1차 소스를 본다.

- WinUI 3 개요 — Microsoft Learn: <https://learn.microsoft.com/en-us/windows/apps/winui/winui3/>
- Fluent 2 (Windows) — <https://fluent2.microsoft.design/components/windows>
- 타이포그래피(Type ramp) — <https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography>
- 재질(Mica/Acrylic/Smoke) — <https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/materials>
- Segoe Fluent Icons — <https://learn.microsoft.com/en-us/windows/apps/design/iconography/segoe-fluent-icons-font>
- 디자인 리소스(폰트·토큰·Figma) — <https://learn.microsoft.com/en-us/windows/apps/design/downloads/>
- Avalonia FluentTheme — <https://docs.avaloniaui.net/docs/basics/user-interface/styling/themes/fluent>
- Avalonia Windows(테마/강조색 감지) — <https://docs.avaloniaui.net/docs/platform-specific-guides/windows>
- FluentAvalonia(amwx) — <https://github.com/amwx/FluentAvalonia>
- Tauri `window-vibrancy` — <https://github.com/tauri-apps/window-vibrancy>
- Fluent UI System Icons(SVG, MIT) — <https://github.com/microsoft/fluentui-system-icons>
