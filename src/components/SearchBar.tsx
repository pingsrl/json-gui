import { type FC } from "react";
import { useI18n } from "../i18n";
import { useJsonStore } from "../store";
import { ObjectSearchSection } from "./search-bar/ObjectSearchSection";
import { SearchScopeInput } from "./search-bar/SearchScopeInput";
import { SegmentedControl } from "./search-bar/SegmentedControl";
import { TextSearchSection } from "./search-bar/TextSearchSection";
import { MODE_OPTIONS } from "./search-bar/shared";
import { useSearchBarState } from "./search-bar/useSearchBarState";

export const SearchBar: FC = () => {
  const {
    filePath,
    nodeCount,
    searchMode,
    activeSearchMode,
    searchScopePath,
    searchSort,
    searching,
    setSearchMode,
    setSearchScopePath,
    setSearchSort,
    search,
    searchObjects,
    clearSearch
  } = useJsonStore();
  const { t } = useI18n();

  const state = useSearchBarState({
    filePath,
    searchMode,
    activeSearchMode,
    searchScopePath,
    searchSort,
    setSearchMode,
    setSearchScopePath,
    setSearchSort,
    search,
    searchObjects,
    clearSearch
  });

  return (
    <div className="border-b border-gray-200 bg-gray-50 dark:border-gray-700 dark:bg-gray-900">
      <div className="flex flex-col gap-3 px-3 py-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <SegmentedControl
            options={MODE_OPTIONS.map((option) => ({
              value: option.value,
              label: t[option.labelKey] as string
            }))}
            selected={searchMode}
            onChange={(mode) => setSearchMode(mode as "text" | "object")}
            name="search-mode"
          />
          <div className="flex flex-wrap items-center gap-2">
            <SearchScopeInput
              value={searchScopePath}
              placeholder={t.searchPathPlaceholder}
              clearLabel={t.clearSearchScope}
              disabled={nodeCount === 0}
              onChange={setSearchScopePath}
              onClear={state.handleClearScope}
            />
          </div>
        </div>

        {searchMode === "text" ? (
          <TextSearchSection
            nodeCount={nodeCount}
            searchQuery={state.searchQuery}
            searchTarget={state.searchTarget}
            useRegex={state.useRegex}
            caseSensitive={state.caseSensitive}
            exactMatch={state.exactMatch}
            regexFlags={state.regexFlags}
            filterState={state.filterState}
            filterSetters={state.filterSetters}
            onSearchQueryChange={state.handleSearchQueryChange}
            onSearchTargetChange={state.setSearchTarget}
            onRegexFlagsChange={state.setRegexFlags}
            onClearTextSearch={state.handleClearTextSearch}
            onSubmitTextSearch={state.submitTextSearch}
          />
        ) : (
          <ObjectSearchSection
            nodeCount={nodeCount}
            searching={searching}
            activeSearchMode={activeSearchMode}
            searchScopePath={searchScopePath}
            objectKeyCaseSensitive={state.objectKeyCaseSensitive}
            objectValueCaseSensitive={state.objectValueCaseSensitive}
            objectRows={state.objectRows}
            executableRowCount={state.executableRowCount}
            appliedFingerprints={state.appliedFingerprints}
            appliedObjectKeyCaseSensitive={
              state.appliedObjectKeyCaseSensitive
            }
            appliedObjectValueCaseSensitive={
              state.appliedObjectValueCaseSensitive
            }
            appliedScopePath={state.appliedScopePath}
            primaryObjectRowId={state.primaryObjectRowId}
            autocompleteRowId={state.autocompleteRowId}
            autocompleteSuggestions={state.autocompleteSuggestions}
            autocompleteIndex={state.autocompleteIndex}
            autocompleteDropdownStyle={state.autocompleteDropdownStyle}
            getActiveSuggestionForRow={state.getActiveSuggestionForRow}
            registerPathInput={state.registerPathInput}
            onObjectKeyCaseSensitiveChange={state.setObjectKeyCaseSensitive}
            onObjectValueCaseSensitiveChange={state.setObjectValueCaseSensitive}
            onReset={state.handleResetObjectFilters}
            onApplyAll={() => {
              void state.handleApplyAll();
            }}
            onToggleRowEnabled={state.handleToggleObjectRowEnabled}
            onPathFocus={state.handleObjectPathFocus}
            onPathBlur={state.handleObjectPathBlur}
            onPathKeyDown={state.handleObjectPathKeyDown}
            onPathChange={state.handleObjectPathChange}
            onOperatorChange={state.handleObjectOperatorChange}
            onValueChange={state.handleObjectValueChange}
            onRegexFlagsChange={state.handleObjectRegexFlagsChange}
            onApplyRow={(rowId) => {
              void state.handleApplyRow(rowId);
            }}
            onRemoveRow={state.handleRemoveRow}
            onInsertRowAfter={state.handleInsertRowAfter}
            onAutocompleteSelect={state.applyAutocompleteSuggestion}
            onAutocompleteHover={state.setAutocompleteIndex}
          />
        )}
      </div>
    </div>
  );
};
