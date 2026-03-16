import { useState, useCallback, useEffect, useRef, type FC } from "react";
import { Search, X } from "lucide-react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

export const SearchPanel: FC = () => {
  const {
    nodeCount,
    searching,
    searchResults,
    selectedNode,
    search,
    clearSearch,
    navigateToNode
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

  const handleSearch = useCallback(
    (q: string) => {
      setSearchQuery(q);
      clearTimeout(searchTimer.current);
      searchTimer.current = setTimeout(() => {
        search(q, searchTarget, caseSensitive, useRegex, exactMatch);
      }, 150);
    },
    [search, searchTarget, caseSensitive, useRegex, exactMatch]
  );

  const handleClear = () => {
    setSearchQuery("");
    clearSearch();
  };

  useEffect(() => {
    if (searchQuery) {
      search(searchQuery, searchTarget, caseSensitive, useRegex, exactMatch);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchTarget, caseSensitive, useRegex, exactMatch]);

  return (
    <div className="w-72 flex flex-col border-r border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 flex-shrink-0">
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

        <div className="mt-2 flex gap-3 flex-wrap">
          {(["both", "keys", "values"] as const).map((opt) => (
            <label
              key={opt}
              className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400 cursor-pointer"
            >
              <input
                type="radio"
                name="target"
                value={opt}
                checked={searchTarget === opt}
                onChange={() => setSearchTarget(opt)}
                className="accent-blue-500"
              />
              {opt === "both"
                ? t.searchBoth
                : opt === "keys"
                  ? t.searchKeys
                  : t.searchValues}
            </label>
          ))}
        </div>

        <div className="mt-1.5 flex gap-3 flex-wrap">
          <label className="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              checked={caseSensitive}
              onChange={(e) => setCaseSensitive(e.target.checked)}
              className="accent-blue-500"
            />
            {t.caseSensitive}
          </label>
          <label className="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              checked={useRegex}
              onChange={(e) => setUseRegex(e.target.checked)}
              className="accent-blue-500"
            />
            {t.regex}
          </label>
          <label className="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              checked={exactMatch}
              onChange={(e) => setExactMatch(e.target.checked)}
              className="accent-blue-500"
            />
            {t.exactMatch}
          </label>
        </div>
      </div>

      <div className="flex-1 overflow-auto">
        {searching && (
          <div className="p-3 text-gray-400 dark:text-gray-500 text-xs">
            {t.searching}
          </div>
        )}
        {!searching && !selectedNode && searchResults.length > 0 && (
          <div>
            <div className="px-3 py-1.5 text-xs text-gray-400 dark:text-gray-500 border-b border-gray-200 dark:border-gray-700 sticky top-0 bg-gray-50 dark:bg-gray-900">
              {t.results(searchResults.length)}
              {searchResults.length === 500 && (
                <span className="text-yellow-600 ml-1">{t.limitReached}</span>
              )}
            </div>
            {searchResults.map((r) => (
              <div
                key={r.node_id}
                onClick={() => navigateToNode(r.node_id)}
                className="px-3 py-2 hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer border-b border-gray-100 dark:border-gray-800"
              >
                <div className="text-xs text-blue-600 dark:text-blue-400 font-mono truncate">
                  {r.path}
                </div>
                <div className="text-xs text-gray-700 dark:text-gray-300 font-mono truncate mt-0.5">
                  {r.value_preview}
                </div>
              </div>
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
  );
};
