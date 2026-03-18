import {
  useMemo,
  useRef,
  useEffect,
  useState,
  type FC,
  type KeyboardEvent
} from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { useI18n } from "../i18n";

// ── Tokenizer ─────────────────────────────────────────────────────────────────

type RegexTokenType =
  | "literal"
  | "escape"
  | "quantifier"
  | "charclass"
  | "group"
  | "anchor"
  | "alternation";

const TOKEN_COLORS: Record<RegexTokenType, { light: string; dark: string }> = {
  literal: { light: "inherit", dark: "inherit" },
  escape: { light: "#16a34a", dark: "#4ade80" },
  quantifier: { light: "#ea580c", dark: "#fb923c" },
  charclass: { light: "#b45309", dark: "#fbbf24" },
  group: { light: "#7c3aed", dark: "#c084fc" },
  anchor: { light: "#dc2626", dark: "#f87171" },
  alternation: { light: "#c026d3", dark: "#e879f9" }
};

function tokenize(
  pattern: string
): Array<{ text: string; type: RegexTokenType }> {
  const tokens: Array<{ text: string; type: RegexTokenType }> = [];
  let i = 0;
  while (i < pattern.length) {
    const ch = pattern[i];

    if (ch === "\\") {
      const next = pattern[i + 1];
      if (next !== undefined) {
        tokens.push({
          text: ch + next,
          type: next === "b" || next === "B" ? "anchor" : "escape"
        });
        i += 2;
        continue;
      }
      tokens.push({ text: ch, type: "literal" });
      i++;
      continue;
    }
    if (ch === "[") {
      let j = i + 1;
      if (pattern[j] === "^") j++;
      if (pattern[j] === "]") j++;
      while (j < pattern.length && pattern[j] !== "]") {
        if (pattern[j] === "\\") j++;
        j++;
      }
      tokens.push({ text: pattern.slice(i, j + 1), type: "charclass" });
      i = j + 1;
      continue;
    }
    if (ch === "(" || ch === ")") {
      tokens.push({ text: ch, type: "group" });
      i++;
      continue;
    }
    if (ch === "*" || ch === "+" || ch === "?") {
      let q = ch;
      if (pattern[i + 1] === "?") {
        q += "?";
        i++;
      }
      tokens.push({ text: q, type: "quantifier" });
      i++;
      continue;
    }
    if (ch === "{") {
      const end = pattern.indexOf("}", i);
      if (end !== -1 && /^\{\d+(,\d*)?\}/.test(pattern.slice(i, end + 1))) {
        tokens.push({ text: pattern.slice(i, end + 1), type: "quantifier" });
        i = end + 1;
        continue;
      }
    }
    if (ch === "^" || ch === "$") {
      tokens.push({ text: ch, type: "anchor" });
      i++;
      continue;
    }
    if (ch === ".") {
      tokens.push({ text: ch, type: "charclass" });
      i++;
      continue;
    }
    if (ch === "|") {
      tokens.push({ text: ch, type: "alternation" });
      i++;
      continue;
    }
    tokens.push({ text: ch, type: "literal" });
    i++;
  }
  return tokens;
}

// ── Component ─────────────────────────────────────────────────────────────────

export interface RegexFlags {
  caseInsensitive: boolean;
  multiline: boolean;
  dotAll: boolean;
}

interface Props {
  id?: string;
  value: string;
  flags: RegexFlags;
  placeholder?: string;
  disabled?: boolean;
  /**
   * compact=true  → usato nelle righe filtro oggetto (sfondo gray-50/900,
   *                   errore come tooltip sul bordo, nessun testo aggiuntivo)
   * compact=false → usato nella barra testo (sfondo white/gray-700,
   *                   errore testuale sotto il campo)
   */
  compact?: boolean;
  onChange: (value: string) => void;
  onFlagsChange: (flags: RegexFlags) => void;
  onClear?: () => void;
  onEnter?: () => void;
}

const FLAG_DEFS = [
  { key: "caseInsensitive" as const, label: "i", title: "Case insensitive" },
  {
    key: "multiline" as const,
    label: "m",
    title: "Multiline (^ $ match line boundaries)"
  },
  { key: "dotAll" as const, label: "s", title: "Dot matches newline" }
] as const;

export const RegexInput: FC<Props> = ({
  id,
  value,
  flags,
  placeholder,
  disabled = false,
  compact = false,
  onChange,
  onFlagsChange,
  onClear,
  onEnter
}) => {
  const overlayRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const { t } = useI18n();

  const tokens = useMemo(() => tokenize(value), [value]);

  const rawError = useMemo(() => {
    if (!value) return null;
    try {
      new RegExp(value);
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : "Invalid regex";
    }
  }, [value]);

  const errorMsg = rawError ? t.regexError(rawError) : null;
  const isDark = document.documentElement.classList.contains("dark");

  // Posizione fixed calcolata dalla bounding rect — non è clippata da overflow hidden/auto
  const [errorStyle, setErrorStyle] = useState<{
    top: number;
    left: number;
  } | null>(null);

  useEffect(() => {
    if (!errorMsg || !containerRef.current) {
      setErrorStyle(null);
      return;
    }
    const rect = containerRef.current.getBoundingClientRect();
    setErrorStyle({ top: rect.bottom + 2, left: rect.left });
  }, [errorMsg, value]);

  // Stili condizionali: compact (riga filtro) vs default (barra testo)
  const bgCls = compact
    ? "bg-gray-50 dark:bg-gray-900"
    : "bg-white dark:bg-gray-700";
  const borderCls = compact
    ? "border-gray-200 dark:border-gray-700"
    : "border-gray-300 dark:border-gray-600";
  const separatorCls = compact
    ? "border-gray-200 dark:border-gray-700"
    : "border-gray-200 dark:border-gray-600";
  const radiusCls = compact ? "rounded-lg" : "rounded";
  const fontCls = ""; // mai font-mono nell'input: deve matchare il font del campo normale
  const textSizeCls = compact ? "text-xs" : "text-sm";

  return (
    <div ref={containerRef} className="w-full">
      <div
        className={`flex items-stretch border transition-colors focus-within:border-blue-500 ${radiusCls} ${bgCls} ${compact ? "shadow-sm" : ""} ${
          errorMsg ? "border-red-500 dark:border-red-500" : borderCls
        } ${disabled ? "opacity-40" : ""}`}
      >
        {/* prefix / — in non-compact occupa w-8 (32px) come il pl-8 del normale input */}
        <span
          className={`flex items-center justify-center font-mono ${textSizeCls} text-gray-400 dark:text-gray-500 ${compact ? "pl-2.5 pr-0.5" : "w-8"}`}
        >
          /
        </span>

        {/* input + highlight overlay */}
        <div className="relative min-w-0 flex-1 overflow-hidden">
          <div
            ref={overlayRef}
            aria-hidden
            className={`pointer-events-none absolute inset-0 overflow-hidden whitespace-pre ${fontCls} ${textSizeCls} py-2`}
          >
            {tokens.map((token, idx) => (
              <span
                key={idx}
                style={{
                  color: isDark
                    ? TOKEN_COLORS[token.type].dark
                    : TOKEN_COLORS[token.type].light
                }}
              >
                {token.text}
              </span>
            ))}
          </div>
          <input
            id={id}
            type="text"
            placeholder={placeholder}
            value={value}
            disabled={disabled}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
              if (e.key === "Enter") {
                e.preventDefault();
                onEnter?.();
              }
            }}
            onScroll={(e) => {
              if (overlayRef.current) {
                overlayRef.current.scrollLeft = e.currentTarget.scrollLeft;
              }
            }}
            className={`relative w-full bg-transparent ${fontCls} ${textSizeCls} py-2 caret-gray-900 placeholder-gray-400 focus:outline-none disabled:cursor-not-allowed dark:caret-gray-100 dark:placeholder-gray-500`}
            style={{ color: "transparent" }}
          />
        </div>

        {/* separator + suffix / + flags + clear */}
        <div className={`flex items-center border-l ${separatorCls}`}>
          <span
            className={`flex items-center pl-1.5 pr-0.5 select-none font-mono ${textSizeCls === "text-sm" ? "text-sm" : "text-xs"} text-gray-400 dark:text-gray-500`}
          >
            /
          </span>

          {FLAG_DEFS.map(({ key, label, title }) => (
            <button
              key={key}
              type="button"
              title={title}
              disabled={disabled}
              onClick={() => onFlagsChange({ ...flags, [key]: !flags[key] })}
              className={`px-0.5 font-mono text-[11px] font-semibold transition-colors disabled:cursor-not-allowed ${
                flags[key]
                  ? "text-blue-500 dark:text-blue-400"
                  : "text-gray-400 hover:text-gray-600 dark:text-gray-500 dark:hover:text-gray-300"
              }`}
            >
              {label}
            </button>
          ))}

          {value && onClear && (
            <button
              type="button"
              onClick={onClear}
              className="px-1.5 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            >
              <X size={12} />
            </button>
          )}
        </div>
      </div>

      {/* errore via portal con position:fixed — non clippato da overflow:hidden/auto */}
      {errorStyle &&
        errorMsg &&
        createPortal(
          <div
            style={{
              position: "fixed",
              top: errorStyle.top,
              left: errorStyle.left,
              zIndex: 9999
            }}
            className="whitespace-nowrap rounded bg-red-50 px-1.5 py-0.5 font-mono text-[10px] leading-none text-red-600 shadow ring-1 ring-red-200 dark:bg-red-950 dark:text-red-400 dark:ring-red-800"
          >
            {errorMsg}
          </div>,
          document.body
        )}
    </div>
  );
};
