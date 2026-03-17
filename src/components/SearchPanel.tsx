import { useState, useCallback, useEffect, useRef, type FC } from "react";
import { Check, Search, X } from "lucide-react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

const SEARCH_TARGETS = [
  { value: "both", labelKey: "searchBoth" },
  { value: "keys", labelKey: "searchKeys" },
  { value: "values", labelKey: "searchValues" }
] as const;

const SEARCH_SORT_OPTIONS = [
  { value: "relevance", labelKey: "searchSortRelevance" },
  { value: "file", labelKey: "searchSortFileOrder" }
] as const;

const SEARCH_FILTERS = [
  {
    key: "caseSensitive",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.caseSensitive,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setCaseSensitive(checked),
    labelKey: "caseSensitive"
  },
  {
    key: "regex",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.useRegex,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setUseRegex(checked),
    labelKey: "regex"
  },
  {
    key: "exactMatch",
    getChecked: (state: {
      caseSensitive: boolean;
      useRegex: boolean;
      exactMatch: boolean;
    }) => state.exactMatch,
    onChange: (
      checked: boolean,
      setters: {
        setCaseSensitive: (value: boolean) => void;
        setUseRegex: (value: boolean) => void;
        setExactMatch: (value: boolean) => void;
      }
    ) => setters.setExactMatch(checked),
    labelKey: "exactMatch"
  }
] as const;

function splitSearchPath(path: string) {
  const lastDot = path.lastIndexOf(".");
  if (lastDot <= 1) {
    return { parentPath: "$", leafPath: path.replace(/^\$\./, "") || "$" };
  }
  return {
    parentPath: path.slice(0, lastDot),
    leafPath: path.slice(lastDot + 1)
  };
}

export const SearchPanel: FC = () => {
  const {
    nodeCount,
    searching,
    searchResults,
    search,
    clearSearch,
    navigateToNode,
    searchScopePath,
    setSearchScopePath,
    searchSort,
    setSearchSort
  } = useJsonStore();
  const { t } = useI18n();

  const [searchQuery, setSearchQuery] = useState("");
  const [searchTarget, setSearchTarget] = useState("both");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [exactMatch, setExactMatch] = useState(false);
  const searchTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );
  const filterState = { caseSensitive, useRegex, exactMatch };
  const filterSetters = { setCaseSensitive, setUseRegex, setExactMatch };

  const scheduleSearch = useCallback(
    (q: string, path: string) => {
      clearTimeout(searchTimer.current);
      if (!q.trim()) {
        clearSearch();
        return;
      }
      searchTimer.current = setTimeout(() => {
        search(q, searchTarget, caseSensitive, useRegex, exactMatch, path);
      }, 150);
    },
    [
      caseSensitive,
      clearSearch,
      exactMatch,
      search,
      searchTarget,
      useRegex
    ]
  );

  const handleSearch = useCallback(
    (q: string) => {
      setSearchQuery(q);
      scheduleSearch(q, searchScopePath);
    },
    [scheduleSearch, searchScopePath]
  );

  const handleClear = () => {
    clearTimeout(searchTimer.current);
    setSearchQuery("");
    clearSearch();
  };

  const handleScopeChange = (path: string) => {
    setSearchScopePath(path);
  };

  const handleClearScope = () => {
    setSearchScopePath("");
  };

  useEffect(() => {
    return () => clearTimeout(searchTimer.current);
  }, []);

  useEffect(() => {
    if (searchQuery) {
      search(
        searchQuery,
        searchTarget,
        caseSensitive,
        useRegex,
        exactMatch,
        searchScopePath
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchTarget, caseSensitive, useRegex, exactMatch]);

  useEffect(() => {
    if (searchQuery) {
      scheduleSearch(searchQuery, searchScopePath);
    }
  }, [scheduleSearch, searchQuery, searchScopePath]);

  return (
    <div className="flex flex-col border-r border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 h-full overflow-hidden">
      <div className="p-3 border-b border-gray-200 dark:border-gray-700">
        <div className="relative">
          <Search
            size={14}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400 dark:text-gray-500"
          />
          <input
            id="search-input"
            type="text"
            placeholder={t.searchPlaceholder}
            value={searchQuery}
            onChange={(e) => handleSearch(e.target.value)}
            disabled={nodeCount === 0}
            className="w-full pl-8 pr-8 py-1.5 bg-white dark:bg-gray-700 border border-gray-300 dark:border-gray-600 rounded text-sm placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-blue-500 disabled:opacity-40 disabled:cursor-not-allowed text-gray-900 dark:text-gray-100"
          />
          {searchQuery && (
            <button
              onClick={handleClear}
              className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            >
              <X size={12} />
            </button>
          )}
        </div>

        <div className="mt-3">
          <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchScope}
          </div>
          <div className="inline-flex w-full rounded-xl border border-gray-200 bg-gray-100 p-1 shadow-sm dark:border-gray-700 dark:bg-gray-800/80">
            {SEARCH_TARGETS.map((opt) => {
              const checked = searchTarget === opt.value;
              const label = t[opt.labelKey];
              return (
                <label key={opt.value} className="min-w-0 flex-1 cursor-pointer">
                  <input
                    type="radio"
                    name="target"
                    value={opt.value}
                    checked={checked}
                    onChange={() => setSearchTarget(opt.value)}
                    className="peer sr-only"
                  />
                  <span
                    className={`flex items-center justify-center rounded-lg px-3 py-2 text-xs font-semibold transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                      checked
                        ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-gray-100"
                        : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                    }`}
                  >
                    {label}
                  </span>
                </label>
              );
            })}
          </div>
        </div>

        <div className="mt-3">
          <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchFilters}
          </div>
          <div className="flex gap-2 flex-wrap">
            {SEARCH_FILTERS.map((filter) => {
              const checked = filter.getChecked(filterState);
              const label = t[filter.labelKey];
              return (
                <label key={filter.key} className="cursor-pointer">
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={(e) =>
                      filter.onChange(e.target.checked, filterSetters)
                    }
                    className="peer sr-only"
                  />
                  <span
                    className={`inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-xs font-medium shadow-sm transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                      checked
                        ? "border-blue-500 bg-blue-50 text-blue-700 dark:border-blue-400/80 dark:bg-blue-500/15 dark:text-blue-200"
                        : "border-gray-200 bg-white text-gray-600 hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
                    }`}
                  >
                    <span
                      className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded border ${
                        checked
                          ? "border-blue-600 bg-blue-600 text-white dark:border-blue-300 dark:bg-blue-400 dark:text-gray-950"
                          : "border-gray-300 bg-white text-transparent dark:border-gray-600 dark:bg-gray-800"
                      }`}
                    >
                      <Check size={11} strokeWidth={3} />
                    </span>
                    {label}
                  </span>
                </label>
              );
            })}
          </div>
        </div>

        <div className="mt-3">
          <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchPath}
          </div>
          <div className="relative">
            <input
              id="search-path-input"
              type="text"
              placeholder={t.searchPathPlaceholder}
              value={searchScopePath}
              onChange={(e) => handleScopeChange(e.target.value)}
              disabled={nodeCount === 0}
              className="w-full rounded-lg border border-gray-200 bg-white px-3 py-2 pr-8 text-xs font-mono text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 disabled:cursor-not-allowed disabled:opacity-40 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-200 dark:placeholder:text-gray-500"
            />
            {searchScopePath && (
              <button
                onClick={handleClearScope}
                className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 transition-colors hover:text-gray-600 dark:hover:text-gray-300"
                title={t.clearSearchScope}
              >
                <X size={12} />
              </button>
            )}
          </div>
        </div>

        <div className="mt-3">
          <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.searchSort}
          </div>
          <div className="inline-flex w-full rounded-xl border border-gray-200 bg-gray-100 p-1 shadow-sm dark:border-gray-700 dark:bg-gray-800/80">
            {SEARCH_SORT_OPTIONS.map((opt) => {
              const checked = searchSort === opt.value;
              const label = t[opt.labelKey];
              return (
                <label key={opt.value} className="min-w-0 flex-1 cursor-pointer">
                  <input
                    type="radio"
                    name="search-sort"
                    value={opt.value}
                    checked={checked}
                    onChange={() => setSearchSort(opt.value)}
                    className="peer sr-only"
                  />
                  <span
                    className={`flex items-center justify-center rounded-lg px-3 py-2 text-xs font-semibold transition-all peer-focus-visible:ring-2 peer-focus-visible:ring-blue-500/50 ${
                      checked
                        ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-gray-100"
                        : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                    }`}
                  >
                    {label}
                  </span>
                </label>
              );
            })}
          </div>
        </div>
      </div>

      <div className="flex-1 min-h-0 flex flex-col">
        {!searching && searchResults.length > 0 && (
          <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 flex-shrink-0">
            {t.results(searchResults.length)}
            {searchResults.length === 500 && (
              <span className="text-yellow-600 ml-1">{t.limitReached}</span>
            )}
          </div>
        )}
        <div className="flex-1 overflow-auto">
          {searching && (
            <div className="p-3 text-gray-400 dark:text-gray-500 text-xs">
              {t.searching}
            </div>
          )}
          {!searching && searchResults.length > 0 && (
            <div>
              {searchResults.map((r) => (
              (() => {
                const { parentPath, leafPath } = splitSearchPath(r.path);
                return (
                  <div
                    key={r.node_id}
                    onClick={() => navigateToNode(r.node_id)}
                    className="px-3 py-2 hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer border-b border-gray-100 dark:border-gray-800"
                    title={r.path}
                  >
                    <div className="space-y-0.5">
                      <div className="text-[10px] text-gray-400 dark:text-gray-500 font-mono whitespace-normal break-all leading-4">
                        {parentPath}
                      </div>
                      <div className="text-xs text-blue-600 dark:text-blue-400 font-mono whitespace-normal break-all leading-4">
                        {leafPath}
                      </div>
                    </div>
                    <div
                      className="mt-1 text-xs text-gray-700 dark:text-gray-300 font-mono whitespace-normal break-all leading-4"
                      title={r.value_preview}
                    >
                      {r.value_preview}
                    </div>
                  </div>
                );
              })()
              ))}
            </div>
          )}
          {!searching && searchQuery && searchResults.length === 0 && (
            <div className="p-3 text-gray-400 dark:text-gray-500 text-xs">
              {t.noResults}
            </div>
          )}
          {!searching && !searchQuery && nodeCount > 0 && (
            <div className="p-3 text-gray-400 dark:text-gray-600 text-xs">
              {t.searchHint(nodeCount.toLocaleString())}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
