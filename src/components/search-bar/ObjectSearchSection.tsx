import { type ChangeEvent, type FC, type KeyboardEvent } from "react";
import { type SearchMode } from "../../store";
import { type RegexFlags } from "../RegexInput";
import { AutocompletePortal } from "./AutocompletePortal";
import { ObjectSearchRow } from "./ObjectSearchRow";
import { ToggleChip } from "./ToggleChip";
import {
  getObjectFilterFingerprint,
  type AutocompleteDropdownStyle,
  type ObjectFilterRow
} from "./shared";
import { useI18n } from "../../i18n";

interface ObjectSearchSectionProps {
  nodeCount: number;
  searching: boolean;
  activeSearchMode: SearchMode | null;
  searchScopePath: string;
  objectKeyCaseSensitive: boolean;
  objectValueCaseSensitive: boolean;
  objectRows: ObjectFilterRow[];
  executableRowCount: number;
  appliedFingerprints: Map<number, string>;
  appliedObjectKeyCaseSensitive: boolean | null;
  appliedObjectValueCaseSensitive: boolean | null;
  appliedScopePath: string;
  primaryObjectRowId: number | null;
  autocompleteRowId: number | null;
  autocompleteSuggestions: string[];
  autocompleteIndex: number;
  autocompleteDropdownStyle: AutocompleteDropdownStyle | null;
  getActiveSuggestionForRow: (
    rowId: number,
    inputValue: string
  ) => string | null;
  registerPathInput: (rowId: number, element: HTMLInputElement | null) => void;
  onObjectKeyCaseSensitiveChange: (checked: boolean) => void;
  onObjectValueCaseSensitiveChange: (checked: boolean) => void;
  onReset: () => void;
  onApplyAll: () => void;
  onToggleRowEnabled: (rowId: number, enabled: boolean) => void;
  onPathFocus: (rowId: number, path: string) => void;
  onPathBlur: () => void;
  onPathKeyDown: (
    event: KeyboardEvent<HTMLInputElement>,
    rowId: number
  ) => void;
  onPathChange: (rowId: number, event: ChangeEvent<HTMLInputElement>) => void;
  onOperatorChange: (
    rowId: number,
    operator: ObjectFilterRow["operator"]
  ) => void;
  onValueChange: (rowId: number, value: string) => void;
  onRegexFlagsChange: (rowId: number, flags: RegexFlags) => void;
  onApplyRow: (rowId: number) => void;
  onRemoveRow: (rowId: number) => void;
  onInsertRowAfter: (rowId: number) => void;
  onAutocompleteSelect: (rowId: number, suggestion: string) => void;
  onAutocompleteHover: (index: number) => void;
}

export const ObjectSearchSection: FC<ObjectSearchSectionProps> = ({
  nodeCount,
  searching,
  activeSearchMode,
  searchScopePath,
  objectKeyCaseSensitive,
  objectValueCaseSensitive,
  objectRows,
  executableRowCount,
  appliedFingerprints,
  appliedObjectKeyCaseSensitive,
  appliedObjectValueCaseSensitive,
  appliedScopePath,
  primaryObjectRowId,
  autocompleteRowId,
  autocompleteSuggestions,
  autocompleteIndex,
  autocompleteDropdownStyle,
  getActiveSuggestionForRow,
  registerPathInput,
  onObjectKeyCaseSensitiveChange,
  onObjectValueCaseSensitiveChange,
  onReset,
  onApplyAll,
  onToggleRowEnabled,
  onPathFocus,
  onPathBlur,
  onPathKeyDown,
  onPathChange,
  onOperatorChange,
  onValueChange,
  onRegexFlagsChange,
  onApplyRow,
  onRemoveRow,
  onInsertRowAfter,
  onAutocompleteSelect,
  onAutocompleteHover
}) => {
  const { t } = useI18n();

  return (
    <>
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <ToggleChip
              checked={objectKeyCaseSensitive}
              label={t.caseSensitiveKey}
              onChange={onObjectKeyCaseSensitiveChange}
            />
            <ToggleChip
              checked={objectValueCaseSensitive}
              label={t.caseSensitiveValue}
              onChange={onObjectValueCaseSensitiveChange}
            />
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onReset}
              className="rounded-lg border border-gray-200 bg-white px-3 py-2 text-xs font-medium text-gray-600 shadow-sm transition-colors hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300 dark:hover:border-gray-600 dark:hover:bg-gray-700/80"
            >
              {t.resetFilters}
            </button>
            <button
              type="button"
              onClick={onApplyAll}
              disabled={
                nodeCount === 0 || searching || executableRowCount === 0
              }
              className="rounded-lg bg-blue-600 px-3 py-2 text-xs font-semibold text-white shadow-sm transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {t.applyAll}
            </button>
          </div>
        </div>

        <div className="rounded-xl border border-gray-200 bg-white p-2 shadow-sm dark:border-gray-700 dark:bg-gray-800/70">
          <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            {t.objectFilters}
          </div>
          <div className="app-scrollbar max-h-44 overflow-auto pr-1">
            <div className="flex flex-col gap-2">
              {objectRows.map((row) => {
                const isApplied =
                  activeSearchMode === "object" &&
                  appliedObjectKeyCaseSensitive === objectKeyCaseSensitive &&
                  appliedObjectValueCaseSensitive ===
                    objectValueCaseSensitive &&
                  appliedScopePath === searchScopePath.trim() &&
                  appliedFingerprints.get(row.id) ===
                    getObjectFilterFingerprint(row);
                const activeSuggestions =
                  autocompleteRowId === row.id ? autocompleteSuggestions : [];

                return (
                  <ObjectSearchRow
                    key={row.id}
                    row={row}
                    nodeCount={nodeCount}
                    searching={searching}
                    isApplied={isApplied}
                    primary={row.id === primaryObjectRowId}
                    activeSuggestions={activeSuggestions}
                    autocompleteIndex={autocompleteIndex}
                    inlineSuggestion={getActiveSuggestionForRow(
                      row.id,
                      row.path
                    )}
                    registerPathInput={registerPathInput}
                    onToggleEnabled={onToggleRowEnabled}
                    onPathFocus={onPathFocus}
                    onPathBlur={onPathBlur}
                    onPathKeyDown={onPathKeyDown}
                    onPathChange={onPathChange}
                    onOperatorChange={onOperatorChange}
                    onValueChange={onValueChange}
                    onRegexFlagsChange={onRegexFlagsChange}
                    onApply={onApplyRow}
                    onRemove={onRemoveRow}
                    onInsertAfter={onInsertRowAfter}
                  />
                );
              })}
            </div>
          </div>
        </div>
      </div>

      <AutocompletePortal
        rowId={autocompleteRowId}
        suggestions={autocompleteSuggestions}
        selectedIndex={autocompleteIndex}
        style={autocompleteDropdownStyle}
        onSelect={onAutocompleteSelect}
        onHover={onAutocompleteHover}
      />
    </>
  );
};
