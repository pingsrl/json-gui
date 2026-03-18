import { type FC } from "react";
import { X } from "lucide-react";

interface SearchScopeInputProps {
  value: string;
  placeholder: string;
  clearLabel: string;
  disabled: boolean;
  onChange: (value: string) => void;
  onClear: () => void;
}

export const SearchScopeInput: FC<SearchScopeInputProps> = ({
  value,
  placeholder,
  clearLabel,
  disabled,
  onChange,
  onClear
}) => (
  <div className="relative">
    <input
      id="search-path-input"
      type="text"
      placeholder={placeholder}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      disabled={disabled}
      className="w-[240px] rounded-lg border border-gray-200 bg-white px-3 py-2 pr-8 text-xs font-mono text-gray-700 shadow-sm outline-none transition-colors placeholder:text-gray-400 focus:border-blue-500 disabled:cursor-not-allowed disabled:opacity-40 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-200 dark:placeholder:text-gray-500"
    />
    {value && (
      <button
        type="button"
        onClick={onClear}
        className="absolute right-2.5 top-1/2 -translate-y-1/2 text-gray-400 transition-colors hover:text-gray-600 dark:hover:text-gray-300"
        title={clearLabel}
      >
        <X size={12} />
      </button>
    )}
  </div>
);
