import { type FC } from "react";
import { useJsonStore } from "../store";
import { useI18n } from "../i18n";

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
    navigateToNode,
    toggleNode,
    expandedNodes,
    hasActiveSearch,
    searchMode,
    searchSort,
    setSearchSort
  } = useJsonStore();
  const { t } = useI18n();

  const handleResultClick = async (result: (typeof searchResults)[number]) => {
    if (result.kind === "object") {
      const alreadyExpanded = expandedNodes.has(result.node_id);
      await navigateToNode(result.node_id);
      if (!alreadyExpanded) {
        await toggleNode(result.node_id);
      }
      // Seleziona la prima proprietà dell'oggetto invece del nodo oggetto stesso
      const children = useJsonStore.getState().expandedNodes.get(result.node_id);
      if (children && children.length > 0) {
        await navigateToNode(children[0].id);
      }
      return;
    }

    const shouldExpandOneLevel =
      result.value_preview === "[object]" || result.value_preview === "[array]";
    const alreadyExpanded = expandedNodes.has(result.node_id);
    await navigateToNode(result.node_id);
    if (shouldExpandOneLevel && !alreadyExpanded) {
      await toggleNode(result.node_id);
    }
  };

  return (
    <div className="flex h-full flex-col border-r border-gray-200 bg-gray-50 dark:border-gray-700 dark:bg-gray-900">
      {!searching && hasActiveSearch && (
        <div className="flex items-center justify-between border-b border-gray-200 bg-gray-50 px-3 py-1.5 dark:border-gray-700 dark:bg-gray-900">
          <span className="text-xs text-gray-400 dark:text-gray-500">
            {searchResults.length > 0 && (
              <>
                {t.results(searchResults.length)}
                {searchResults.length === 500 && (
                  <span className="ml-1 text-yellow-600">{t.limitReached}</span>
                )}
              </>
            )}
          </span>
          <div className="inline-flex rounded-lg border border-gray-200 bg-gray-100 p-0.5 dark:border-gray-700 dark:bg-gray-800">
            {(["relevance", "file"] as const).map((value) => (
              <button
                key={value}
                type="button"
                onClick={() => setSearchSort(value)}
                className={`rounded-md px-2 py-1 text-[11px] font-medium transition-all ${
                  searchSort === value
                    ? "bg-white text-gray-800 shadow-sm dark:bg-gray-700 dark:text-gray-100"
                    : "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                }`}
              >
                {value === "relevance" ? t.searchSortRelevance : t.searchSortFileOrder}
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="app-scrollbar flex-1 overflow-auto">
        {searching && (
          <div className="p-3 text-xs text-gray-400 dark:text-gray-500">
            {t.searching}
          </div>
        )}

        {!searching && searchResults.length > 0 && (
          <div>
            {searchResults.map((result) => {
              const { parentPath, leafPath } = splitSearchPath(result.path);
              return (
                <div
                  key={`${result.kind}-${result.node_id}`}
                  onClick={() => {
                    void handleResultClick(result);
                  }}
                  className="cursor-pointer border-b border-gray-100 px-3 py-2 hover:bg-gray-100 dark:border-gray-800 dark:hover:bg-gray-800"
                  title={result.path}
                >
                  <div className="space-y-0.5">
                    <div className="font-mono text-[10px] leading-4 whitespace-normal break-all text-gray-400 dark:text-gray-500">
                      {parentPath}
                    </div>
                    <div className="font-mono text-xs leading-4 whitespace-normal break-all text-blue-600 dark:text-blue-400">
                      {leafPath}
                    </div>
                  </div>

                  {result.kind === "object" ? (
                    <div className="mt-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="rounded-md border border-indigo-500/30 bg-indigo-500/10 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-[0.12em] text-indigo-700 dark:text-indigo-200">
                          {result.value_preview}
                        </span>
                      </div>
                      {result.match_preview && (
                        <div className="mt-1 font-mono text-xs leading-4 whitespace-normal break-all text-gray-600 dark:text-gray-300">
                          {result.match_preview}
                        </div>
                      )}
                    </div>
                  ) : (
                    <div
                      className="mt-1 font-mono text-xs leading-4 whitespace-normal break-all text-gray-700 dark:text-gray-300"
                      title={result.value_preview}
                    >
                      {result.value_preview}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {!searching && hasActiveSearch && searchResults.length === 0 && (
          <div className="p-3 text-xs text-gray-400 dark:text-gray-500">
            {t.noResults}
          </div>
        )}

        {!searching && !hasActiveSearch && nodeCount > 0 && (
          <div className="p-3 text-xs text-gray-400 dark:text-gray-600">
            {searchMode === "object"
              ? t.objectSearchHint
              : t.searchHint(nodeCount.toLocaleString())}
          </div>
        )}
      </div>
    </div>
  );
};
