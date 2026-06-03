// 백엔드 `InstalledApp`(src-tauri/src/apps.rs)와 1:1 대응.
// serde가 snake_case 그대로 직렬화하므로 필드명을 동일하게 유지한다.
export interface InstalledApp {
  id: string;
  name: string;
  publisher: string | null;
  version: string | null;
  install_location: string | null;
  size_bytes: number;
  size_estimated: boolean;
  uninstall_string: string | null;
  icon_path: string | null;
}

// `app-size-updated` 이벤트 payload (백엔드에서 emit).
export interface AppSizeUpdate {
  id: string;
  size_bytes: number;
  size_estimated: boolean;
}

export type SortKey = "size" | "name";
