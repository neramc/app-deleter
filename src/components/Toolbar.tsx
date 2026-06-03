import type { SortKey } from "../types";

interface ToolbarProps {
  query: string;
  onQueryChange: (value: string) => void;
  sortKey: SortKey;
  onSortChange: (key: SortKey) => void;
  onRefresh: () => void;
  loading: boolean;
}

export function Toolbar({
  query,
  onQueryChange,
  sortKey,
  onSortChange,
  onRefresh,
  loading,
}: ToolbarProps) {
  return (
    <div className="toolbar">
      <div className="search">
        <span className="search-icon" aria-hidden>
          🔍
        </span>
        <input
          className="search-input"
          type="search"
          placeholder="앱 이름 또는 게시자 검색"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          aria-label="앱 검색"
        />
      </div>

      <div className="sort">
        <label htmlFor="sort-select" className="sort-label">
          정렬
        </label>
        <select
          id="sort-select"
          className="sort-select"
          value={sortKey}
          onChange={(e) => onSortChange(e.target.value as SortKey)}
        >
          <option value="size">용량 큰 순</option>
          <option value="name">이름순</option>
        </select>
      </div>

      <button
        className="btn btn-ghost refresh"
        onClick={onRefresh}
        disabled={loading}
        aria-label="목록 새로고침"
      >
        <span className={loading ? "spin" : ""} aria-hidden>
          ⟳
        </span>
        새로고침
      </button>
    </div>
  );
}
