import { type FC } from "react";
import { createPortal } from "react-dom";
import type { AutocompleteDropdownStyle } from "./shared";

interface AutocompletePortalProps {
  rowId: number | null;
  suggestions: string[];
  selectedIndex: number;
  style: AutocompleteDropdownStyle | null;
  onSelect: (rowId: number, suggestion: string) => void;
  onHover: (index: number) => void;
}

export const AutocompletePortal: FC<AutocompletePortalProps> = ({
  rowId,
  suggestions,
  selectedIndex,
  style,
  onSelect,
  onHover
}) => {
  if (rowId === null || suggestions.length === 0 || !style) {
    return null;
  }

  return createPortal(
    <div
      id={`object-path-suggestions-${rowId}`}
      role="listbox"
      className="z-[1200] overflow-hidden rounded-xl border border-gray-200 bg-white shadow-[0_12px_32px_rgba(15,23,42,0.2)] dark:border-gray-700 dark:bg-gray-900"
      style={{
        position: "absolute",
        top: style.top,
        left: style.left,
        width: style.width
      }}
    >
      {suggestions.map((suggestion, suggestionIndex) => (
        <button
          id={`object-path-suggestion-${rowId}-${suggestionIndex}`}
          key={suggestion}
          type="button"
          role="option"
          aria-selected={suggestionIndex === selectedIndex}
          tabIndex={-1}
          onMouseDown={(event) => {
            event.preventDefault();
            onSelect(rowId, suggestion);
          }}
          onMouseEnter={() => onHover(suggestionIndex)}
          className={`flex w-full items-center justify-between border-b border-gray-100 px-3 py-2.5 text-left text-xs font-mono transition-colors last:border-b-0 dark:border-gray-800 ${
            suggestionIndex === selectedIndex
              ? "bg-blue-50 text-blue-700 dark:bg-blue-500/15 dark:text-blue-200"
              : "text-gray-600 hover:bg-gray-50 dark:text-gray-300 dark:hover:bg-gray-800"
          }`}
        >
          {suggestion}
        </button>
      ))}
    </div>,
    document.body
  );
};
